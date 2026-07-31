//! Collecting, downloading, and rewriting a post's media (images, native
//! video, native audio) into the archive's shared `media/` store, plus the
//! opt-in yt-dlp bridge for externally hosted video embeds.

use std::borrow::Cow;
use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use crabrave::handlers::blog::Post;
use crabrave::npf::{ContentBlock, MediaObject};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

/// Maximum number of concurrent media downloads.
pub const MAX_CONCURRENT_DOWNLOADS: usize = 6;

/// Which media kinds a run downloads, driven by `--save-images`,
/// `--save-video`, and `--save-audio`, plus the `--wayback` download policy.
#[derive(Clone, Copy, Default)]
pub struct MediaSelection {
    pub images: bool,
    pub video: bool,
    pub audio: bool,
    /// Retry dead media URLs (403/404) through the Internet Archive's
    /// Wayback Machine.
    pub wayback: bool,
}

impl MediaSelection {
    pub fn any(self) -> bool {
        self.images || self.video || self.audio
    }
}

/// Total per-request deadline (connect through body). Mirrors tumblr-utils'
/// 90-second HTTP timeout; without one a stalled CDN connection hangs the
/// backup forever.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Retries after the initial attempt, on transient failures only.
const MAX_DOWNLOAD_RETRIES: u32 = 3;

/// Collects all image URLs from a post's content and trail.
pub fn collect_image_urls(post: &Post) -> Vec<String> {
    let mut urls = Vec::new();
    collect_from_blocks(&post.content, &mut urls);
    for trail_item in &post.trail {
        collect_from_blocks(&trail_item.content, &mut urls);
    }
    urls
}

fn collect_from_blocks(blocks: &[ContentBlock], urls: &mut Vec<String>) {
    for block in blocks {
        if let ContentBlock::Image { media, .. } = block {
            // Prefer the original-dimensions image, fall back to the first available
            let best = media
                .iter()
                .find(|m| matches!(m.has_original_dimensions, Some(true)))
                .or_else(|| media.first());
            if let Some(m) = best
                && !m.url.is_empty()
            {
                urls.push(m.url.clone());
            }
        }
    }
}

/// Applies `f` to the post's own content blocks and every trail item's.
fn for_all_blocks(post: &Post, mut f: impl FnMut(&[ContentBlock])) {
    f(&post.content);
    for trail_item in &post.trail {
        f(&trail_item.content);
    }
}

/// Collects native Tumblr video file URLs from a post's content and trail.
/// External embeds (YouTube, Vimeo, ...) carry no `media` entries and are
/// deliberately excluded — see [`collect_external_video_urls`].
pub fn collect_video_urls(post: &Post) -> Vec<String> {
    let mut urls = Vec::new();
    for_all_blocks(post, |blocks| {
        for block in blocks {
            if let ContentBlock::Video { media: Some(media), .. } = block
                && !media.url.is_empty()
            {
                urls.push(media.url.clone());
            }
        }
    });
    urls
}

/// Collects native Tumblr audio file URLs from a post's content and trail.
pub fn collect_audio_urls(post: &Post) -> Vec<String> {
    let mut urls = Vec::new();
    for_all_blocks(post, |blocks| {
        for block in blocks {
            if let ContentBlock::Audio { media: Some(media), .. } = block
                && !media.url.is_empty()
            {
                urls.push(media.url.clone());
            }
        }
    });
    urls
}

/// Collects externally hosted video embed URLs (blocks with a source URL but
/// no native media file) for the yt-dlp bridge.
pub fn collect_external_video_urls(post: &Post) -> Vec<String> {
    let mut urls = Vec::new();
    for_all_blocks(post, |blocks| {
        for block in blocks {
            if let ContentBlock::Video { media, url: Some(url), .. } = block
                && media.as_ref().is_none_or(|m| m.url.is_empty())
                && !url.is_empty()
            {
                urls.push(url.clone());
            }
        }
    });
    urls
}

/// Downloads a batch of posts' media into the shared per-kind stores under
/// `{output_dir}/media/`, returning the merged URL → relative-path map.
///
/// `depth` is how many directory levels below `output_dir` the referencing
/// files live (0 for flat post files, 1 inside a post directory or a special
/// collection, 2 for both), which fixes up the relative prefix.
pub async fn download_batch_media(
    client: &reqwest::Client,
    posts: &[&Post],
    output_dir: &Utf8Path,
    depth: usize,
    selection: MediaSelection,
    semaphore: &std::sync::Arc<Semaphore>,
) -> anyhow::Result<HashMap<String, String>> {
    type Collector = fn(&Post) -> Vec<String>;
    let kinds: [(bool, Collector, &str); 3] = [
        (selection.images, collect_image_urls, "images"),
        (selection.video, collect_video_urls, "videos"),
        (selection.audio, collect_audio_urls, "audio"),
    ];

    let mut url_map = HashMap::new();
    for (enabled, collect, subdir) in kinds {
        if !enabled {
            continue;
        }
        let urls: Vec<String> = posts.iter().flat_map(|p| collect(p)).collect();
        if urls.is_empty() {
            continue;
        }
        let media_dir = output_dir.join("media").join(subdir);
        let relative_prefix = format!("{}media/{subdir}/", "../".repeat(depth));
        url_map.extend(
            download_media(
                client,
                &urls,
                &media_dir,
                &relative_prefix,
                semaphore,
                selection.wayback,
            )
            .await?,
        );
    }
    Ok(url_map)
}

/// Produces a deterministic local filename for a URL.
///
/// Format: `{sha256_prefix_16hex}_{original_filename}`
pub fn local_filename_for_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let hash = hasher.finalize();
    let hash_prefix = hex::encode(&hash[..8]); // 16 hex chars

    let filename = url
        .rsplit('/')
        .next()
        .and_then(|segment| segment.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("image");

    format!("{hash_prefix}_{filename}")
}

/// Downloads media files concurrently and returns a mapping of original URL to local relative path.
///
/// Only successful downloads appear in the returned map. Individual failed
/// downloads are logged and omitted so that original CDN URLs are preserved in
/// the output. Failure to create the media directory is a hard error: it would
/// silently turn an explicit `--save-*` flag into a CDN-linked archive.
pub async fn download_media(
    client: &reqwest::Client,
    urls: &[String],
    media_dir: &Utf8Path,
    relative_prefix: &str,
    semaphore: &std::sync::Arc<Semaphore>,
    wayback: bool,
) -> anyhow::Result<HashMap<String, String>> {
    // Deduplicate URLs
    let unique_urls: Vec<&String> = {
        let mut seen = std::collections::HashSet::new();
        urls.iter().filter(|u| seen.insert(u.as_str())).collect()
    };

    if unique_urls.is_empty() {
        return Ok(HashMap::new());
    }

    if !fs_err::exists(media_dir).unwrap_or(false) {
        fs_err::create_dir_all(media_dir).map_err(|e| {
            anyhow::anyhow!("failed to create media directory (required by a --save-* flag): {e}")
        })?;
    }

    let mut join_set = tokio::task::JoinSet::new();

    for url in unique_urls {
        let filename = local_filename_for_url(url);
        let dest = media_dir.join(&filename);

        // Skip already-downloaded files
        if fs_err::exists(&dest).unwrap_or(false) {
            log::debug!("Skipping already-downloaded media file: {url}");
            // We still need to include it in the map
            let url_clone = url.clone();
            let rel_path = format!("{relative_prefix}{filename}");
            join_set.spawn(async move { Some((url_clone, rel_path)) });
            continue;
        }

        let client = client.clone();
        let url_clone = url.clone();
        let rel_path = format!("{relative_prefix}{filename}");
        let dest_clone = dest.clone();
        let permit = semaphore.clone();

        join_set.spawn(async move {
            let _permit = match permit.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    log::warn!("Semaphore closed while downloading {url_clone}");
                    return None;
                }
            };

            match download_one(&client, &url_clone, &dest_clone, wayback).await {
                Ok(()) => {
                    log::debug!("Downloaded media file: {url_clone} -> {dest_clone}");
                    Some((url_clone, rel_path))
                }
                Err(e) => {
                    log::warn!("Failed to download media file {url_clone}: {e}");
                    None
                }
            }
        });
    }

    let mut url_map = HashMap::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Some((url, path))) => {
                url_map.insert(url, path);
            }
            Ok(None) => {}
            // A panicked/cancelled task must not be silent: the post will
            // keep its CDN link and the user should know why.
            Err(e) => log::warn!("media download task failed: {e}"),
        }
    }

    Ok(url_map)
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Utf8Path,
    wayback: bool,
) -> anyhow::Result<()> {
    let failure = match download_with_retries(client, url, dest).await {
        Ok(()) => return Ok(()),
        Err(f) => f,
    };
    // A 403/404 from the CDN usually means the asset is simply gone (old
    // blogs especially); the Wayback Machine often still has a copy. `web/0/`
    // redirects to the closest snapshot.
    if wayback && matches!(failure.status, Some(403 | 404)) {
        let wayback_url = format!("https://web.archive.org/web/0/{url}");
        log::info!("media URL {url} is dead ({}); trying the Wayback Machine", failure.error);
        return download_with_retries(client, &wayback_url, dest)
            .await
            .map_err(|f| {
                f.error.context(format!(
                    "Wayback Machine fallback also failed (original error: {})",
                    failure.error
                ))
            });
    }
    Err(failure.error)
}

/// A download that failed after the retry policy was exhausted, with the last
/// HTTP status (if the failure was an HTTP error response at all).
struct DownloadFailure {
    status: Option<u16>,
    error: anyhow::Error,
}

async fn download_with_retries(
    client: &reqwest::Client,
    url: &str,
    dest: &Utf8Path,
) -> Result<(), DownloadFailure> {
    let mut attempt: u32 = 0;
    loop {
        let (transient, failure) = match try_download(client, url, dest).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        attempt += 1;
        if !transient || attempt > MAX_DOWNLOAD_RETRIES {
            return Err(failure);
        }
        let backoff = 1u64 << (attempt - 1);
        log::debug!(
            "transient failure downloading {url} ({}); retrying in {backoff}s \
             (attempt {attempt}/{MAX_DOWNLOAD_RETRIES})",
            failure.error
        );
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
    }
}

/// One download attempt. The `bool` distinguishes transient failures (worth
/// retrying: transport errors, timeouts, gateway 5xx) from permanent ones
/// (4xx, disk errors).
async fn try_download(
    client: &reqwest::Client,
    url: &str,
    dest: &Utf8Path,
) -> Result<(), (bool, DownloadFailure)> {
    let transport = |e: reqwest::Error| {
        (
            true,
            DownloadFailure {
                status: None,
                error: anyhow::Error::new(e),
            },
        )
    };
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(transport)?;

    let status = response.status();
    if !status.is_success() {
        let transient = matches!(status.as_u16(), 500 | 502 | 503 | 504);
        return Err((
            transient,
            DownloadFailure {
                status: Some(status.as_u16()),
                error: anyhow::anyhow!("HTTP {status} for {url}"),
            },
        ));
    }

    // The request-level timeout above covers the body read as well.
    let bytes = response.bytes().await.map_err(transport)?;
    crate::fsutil::write_atomic(dest, &bytes).map_err(|e| {
        (
            false,
            DownloadFailure {
                status: None,
                error: anyhow::Error::new(e),
            },
        )
    })?;
    Ok(())
}

/// Rewrites downloaded media URLs (images, native video, native audio) in a
/// post, returning a modified clone.
///
/// Only URLs present in `url_map` (successful downloads) are rewritten.
/// URLs not in the map (failed downloads) remain as original CDN URLs.
///
/// Returns `Cow::Borrowed` if no URLs need rewriting, avoiding unnecessary clones.
pub fn rewrite_post_media_urls<'a>(
    post: &'a Post,
    url_map: &HashMap<String, String>,
) -> Cow<'a, Post> {
    if url_map.is_empty() {
        return Cow::Borrowed(post);
    }

    // Check if any URLs in this post actually need rewriting
    let has_rewritable = post
        .content
        .iter()
        .chain(post.trail.iter().flat_map(|t| t.content.iter()))
        .any(|block| block_media_urls(block).any(|url| url_map.contains_key(url)));

    if !has_rewritable {
        return Cow::Borrowed(post);
    }

    let mut post = post.clone();
    rewrite_blocks(&mut post.content, url_map);
    for trail_item in &mut post.trail {
        rewrite_blocks(&mut trail_item.content, url_map);
    }
    Cow::Owned(post)
}

/// The media-file URLs a block carries, across the block kinds we download.
fn block_media_urls(block: &ContentBlock) -> Box<dyn Iterator<Item = &str> + '_> {
    match block {
        ContentBlock::Image { media, .. } => Box::new(media.iter().map(|m| m.url.as_str())),
        ContentBlock::Video { media: Some(media), .. }
        | ContentBlock::Audio { media: Some(media), .. } => {
            Box::new(std::iter::once(media.url.as_str()))
        }
        _ => Box::new(std::iter::empty()),
    }
}

fn rewrite_media_object(media_obj: &mut MediaObject, url_map: &HashMap<String, String>) {
    if let Some(local_path) = url_map.get(&media_obj.url) {
        *media_obj = MediaObject {
            url: local_path.clone(),
            ..media_obj.clone()
        };
    }
}

fn rewrite_blocks(blocks: &mut [ContentBlock], url_map: &HashMap<String, String>) {
    for block in blocks {
        match block {
            ContentBlock::Image { media, .. } => {
                for media_obj in media {
                    rewrite_media_object(media_obj, url_map);
                }
            }
            ContentBlock::Video { media: Some(media), .. }
            | ContentBlock::Audio { media: Some(media), .. } => {
                rewrite_media_object(media, url_map);
            }
            _ => {}
        }
    }
}

/// Ceiling on one yt-dlp invocation. External videos can be long, but a hung
/// extractor must not stall the backup forever.
const EXTERNAL_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// Best-effort bridge to yt-dlp for externally hosted video embeds
/// (`--save-video-external`). Files land in `media/videos/external/`;
/// yt-dlp's own `--download-archive` file dedups across runs. Downloads are
/// not linked back into the rendered HTML — the original embed markup is kept
/// — so failures only cost the local copy, never the archive.
pub struct ExternalVideoDownloader {
    dir: Utf8PathBuf,
    archive_file: Utf8PathBuf,
    cookies_file: Option<Utf8PathBuf>,
    quiet: bool,
}

impl ExternalVideoDownloader {
    /// Probes for yt-dlp; returns `None` (with a printed notice — silently
    /// skipping an explicit flag would misrepresent the archive) when it is
    /// not runnable.
    pub async fn new(
        output_dir: &Utf8Path,
        cookies_file: Option<&Utf8Path>,
        quiet: bool,
    ) -> anyhow::Result<Option<Self>> {
        let probe = tokio::process::Command::new("yt-dlp")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        if !probe.map(|s| s.success()).unwrap_or(false) {
            crate::api::say(format_args!(
                "Warning: yt-dlp was not found on PATH; --save-video-external is \
                 disabled for this run and external video embeds will not be downloaded."
            ))?;
            return Ok(None);
        }

        let dir = output_dir.join("media").join("videos").join("external");
        let archive_file = dir.join(".yt-dlp-archive");
        Ok(Some(Self {
            dir,
            archive_file,
            cookies_file: cookies_file.map(Utf8Path::to_path_buf),
            quiet,
        }))
    }

    /// Downloads one batch of posts' external video embeds, sequentially
    /// (yt-dlp does its own connection management). Failures warn and move
    /// on: one dead YouTube link must not abort a backup.
    pub async fn download_batch(&self, posts: &[&Post]) -> anyhow::Result<()> {
        let urls: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            posts
                .iter()
                .flat_map(|p| collect_external_video_urls(p))
                .filter(|u| seen.insert(u.clone()))
                .collect()
        };
        if urls.is_empty() {
            return Ok(());
        }
        if !fs_err::exists(&self.dir).unwrap_or(false) {
            fs_err::create_dir_all(&self.dir)?;
        }

        for url in urls {
            if let Err(e) = self.download_one(&url).await {
                log::warn!("yt-dlp failed for {url}: {e:#}");
                if !self.quiet {
                    crate::api::say(format_args!(
                        "  Warning: could not download external video {url} ({e:#})"
                    ))?;
                }
            }
        }
        Ok(())
    }

    async fn download_one(&self, url: &str) -> anyhow::Result<()> {
        let mut cmd = tokio::process::Command::new("yt-dlp");
        cmd.arg("--no-progress")
            .arg("--no-playlist")
            .arg("--restrict-filenames")
            .arg("--download-archive")
            .arg(self.archive_file.as_str())
            .arg("--output")
            .arg(self.dir.join("%(extractor)s_%(id)s.%(ext)s").as_str());
        if let Some(ref cookies) = self.cookies_file {
            cmd.arg("--cookies").arg(cookies.as_str());
        }
        cmd.arg("--").arg(url);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());
        // If the timeout drops the future mid-run, take the child down with it.
        cmd.kill_on_drop(true);

        let output = tokio::time::timeout(EXTERNAL_DOWNLOAD_TIMEOUT, cmd.output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "timed out after {}s",
                    EXTERNAL_DOWNLOAD_TIMEOUT.as_secs()
                )
            })??;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last_line = stderr.lines().last().unwrap_or("no error output").trim();
            return Err(anyhow::anyhow!("yt-dlp exited with {}: {last_line}", output.status));
        }
        Ok(())
    }
}

/// Picks a file extension for image bytes by their magic number (the avatar
/// endpoint doesn't say what format it serves).
pub fn image_extension_for_bytes(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        "img"
    }
}

/// Writes `media-list.jsonl` (`--media-list`): one JSON object per archived
/// post mapping its id to every media URL it carries (images, native
/// video/audio, external embeds), before any local-path rewriting. The list
/// describes one run: a fresh run truncates it, a `--resume` appends.
pub struct MediaListWriter {
    file: fs_err::File,
}

pub const MEDIA_LIST_FILE: &str = "media-list.jsonl";

impl MediaListWriter {
    pub fn create(output_dir: &Utf8Path, resume: bool) -> anyhow::Result<Self> {
        let path = output_dir.join(MEDIA_LIST_FILE);
        let mut options = fs_err::OpenOptions::new();
        options.create(true);
        if resume {
            options.append(true);
        } else {
            options.write(true).truncate(true);
        }
        Ok(Self {
            file: options.open(path)?,
        })
    }

    /// Records one post's media URLs; posts without media are omitted.
    pub fn record(&mut self, post: &Post) -> anyhow::Result<()> {
        use std::io::Write as _;

        let mut urls = collect_image_urls(post);
        urls.extend(collect_video_urls(post));
        urls.extend(collect_audio_urls(post));
        urls.extend(collect_external_video_urls(post));
        if urls.is_empty() {
            return Ok(());
        }
        let line = serde_json::json!({ "id": post.id, "urls": urls });
        writeln!(self.file, "{line}")?;
        Ok(())
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crabrave::handlers::blog::{Post, TrailItem};
    use crabrave::npf::MediaObject;

    fn make_media(url: &str, has_original: Option<bool>) -> MediaObject {
        MediaObject {
            url: url.to_owned(),
            media_type: None,
            media_key: None,
            identifier: None,
            width: None,
            height: None,
            original_dimensions_missing: None,
            cropped: None,
            has_original_dimensions: has_original,
            colors: None,
            exif: None,
        }
    }

    fn make_post(content: Vec<ContentBlock>, trail: Vec<TrailItem>) -> Post {
        // Build a minimal Post via JSON deserialization
        let json = serde_json::json!({
            "id_string": "123",
            "blog_name": "test",
            "post_url": "https://test.tumblr.com/post/123",
            "type": "text",
            "timestamp": 1700000000,
            "scheduled_publish_time": 0,
            "note_count": 0,
            "reblog_key": "abc",
            "is_blocks_post_format": true,
            "followed": false,
            "liked": false,
            "can_like": false,
            "can_reblog": false,
            "can_reply": false,
            "can_send_in_message": false,
            "can_mute": false,
            "display_avatar": false,
            "interactability_reblog": "",
            "is_blazed": false,
            "is_blaze_pending": false,
            "can_ignite": false,
            "can_blaze": false,
            "muted": false,
            "mute_end_timestamp": 0,
            "content": [],
            "trail": [],
            "tags": [],
            "layout": []
        });
        let mut post: Post = serde_json::from_value(json).unwrap();
        post.content = content;
        post.trail = trail;
        post
    }

    #[test]
    fn collect_image_urls_from_content() {
        let post = make_post(
            vec![ContentBlock::Image {
                media: vec![
                    make_media("https://cdn.tumblr.com/small.jpg", None),
                    make_media("https://cdn.tumblr.com/original.jpg", Some(true)),
                ],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );

        let urls = collect_image_urls(&post);
        assert_eq!(urls.len(), 1);
        // Should prefer the original-dimensions image
        assert_eq!(urls[0], "https://cdn.tumblr.com/original.jpg");
    }

    #[test]
    fn collect_image_urls_falls_back_to_first() {
        let post = make_post(
            vec![ContentBlock::Image {
                media: vec![make_media("https://cdn.tumblr.com/only.jpg", None)],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );

        let urls = collect_image_urls(&post);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://cdn.tumblr.com/only.jpg");
    }

    #[test]
    fn collect_image_urls_skips_empty_media() {
        let post = make_post(
            vec![ContentBlock::Image {
                media: vec![],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );

        assert!(collect_image_urls(&post).is_empty());
    }

    #[test]
    fn collect_image_urls_includes_trail() {
        let trail = vec![TrailItem {
            content: vec![ContentBlock::Image {
                media: vec![make_media("https://cdn.tumblr.com/trail.jpg", Some(true))],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            content_raw: None,
            layout: vec![],
            post: None,
            blog: None,
            is_root_item: false,
        }];

        let post = make_post(vec![], trail);
        let urls = collect_image_urls(&post);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://cdn.tumblr.com/trail.jpg");
    }

    #[test]
    fn collect_image_urls_ignores_non_image_blocks() {
        let post = make_post(
            vec![ContentBlock::Text {
                text: "hello".to_owned(),
                subtype: None,
                formatting: None,
                indent_level: None,
            }],
            vec![],
        );

        assert!(collect_image_urls(&post).is_empty());
    }

    #[test]
    fn local_filename_deterministic() {
        let url = "https://64.media.tumblr.com/abc123/s540x810/image.jpg";
        let a = local_filename_for_url(url);
        let b = local_filename_for_url(url);
        assert_eq!(a, b);
    }

    #[test]
    fn local_filename_contains_original_name() {
        let filename = local_filename_for_url("https://example.com/path/to/photo.png");
        assert!(filename.ends_with("_photo.png"));
    }

    #[test]
    fn local_filename_strips_query_params() {
        let filename = local_filename_for_url("https://example.com/img.jpg?width=500&quality=80");
        assert!(filename.ends_with("_img.jpg"));
    }

    #[test]
    fn local_filename_different_urls_different_hashes() {
        let a = local_filename_for_url("https://example.com/a.jpg");
        let b = local_filename_for_url("https://example.com/b.jpg");
        assert_ne!(a, b);
    }

    #[test]
    fn rewrite_post_media_urls_no_map() {
        let post = make_post(
            vec![ContentBlock::Image {
                media: vec![make_media("https://cdn.tumblr.com/img.jpg", Some(true))],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );

        let result = rewrite_post_media_urls(&post, &HashMap::new());
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn rewrite_post_media_urls_with_map() {
        let post = make_post(
            vec![ContentBlock::Image {
                media: vec![make_media("https://cdn.tumblr.com/img.jpg", Some(true))],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );

        let mut url_map = HashMap::new();
        url_map.insert(
            "https://cdn.tumblr.com/img.jpg".to_owned(),
            "media/images/local.jpg".to_owned(),
        );

        let result = rewrite_post_media_urls(&post, &url_map);
        assert!(matches!(result, Cow::Owned(_)));

        if let ContentBlock::Image { media, .. } = &result.content[0] {
            assert_eq!(media[0].url, "media/images/local.jpg");
        } else {
            panic!("expected Image block");
        }
    }

    fn video_block(json: serde_json::Value) -> ContentBlock {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn collect_video_urls_takes_native_media_only() {
        let native = video_block(serde_json::json!({
            "type": "video",
            "provider": "tumblr",
            "media": { "url": "https://vt.tumblr.com/clip.mp4" }
        }));
        let external = video_block(serde_json::json!({
            "type": "video",
            "provider": "youtube",
            "url": "https://www.youtube.com/watch?v=abc",
            "embed_html": "<iframe></iframe>"
        }));
        let post = make_post(vec![native, external], vec![]);

        assert_eq!(collect_video_urls(&post), vec!["https://vt.tumblr.com/clip.mp4"]);
        assert_eq!(
            collect_external_video_urls(&post),
            vec!["https://www.youtube.com/watch?v=abc"]
        );
    }

    #[test]
    fn image_extension_sniffing() {
        assert_eq!(image_extension_for_bytes(b"\x89PNG\r\n\x1a\nrest"), "png");
        assert_eq!(image_extension_for_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(image_extension_for_bytes(b"GIF89a"), "gif");
        assert_eq!(image_extension_for_bytes(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "webp");
        assert_eq!(image_extension_for_bytes(b"not an image"), "img");
        assert_eq!(image_extension_for_bytes(b""), "img");
    }

    #[test]
    fn media_list_records_posts_with_media_only() {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        let out = dir.join(format!("archivr-test-medialist-{}", std::process::id()));
        fs_err::create_dir_all(&out).unwrap();

        let with_media = make_post(
            vec![ContentBlock::Image {
                media: vec![make_media("https://cdn.tumblr.com/a.jpg", Some(true))],
                alt_text: None,
                caption: None,
                attribution: None,
                colors: None,
                feedback_token: None,
                poster: None,
            }],
            vec![],
        );
        let without_media = make_post(vec![], vec![]);

        let mut writer = MediaListWriter::create(&out, false).unwrap();
        writer.record(&with_media).unwrap();
        writer.record(&without_media).unwrap();
        drop(writer);

        let content = fs_err::read_to_string(out.join(MEDIA_LIST_FILE)).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["id"], "123");
        assert_eq!(parsed["urls"][0], "https://cdn.tumblr.com/a.jpg");

        // A fresh (non-resume) writer truncates; a resume writer appends.
        let mut writer = MediaListWriter::create(&out, true).unwrap();
        writer.record(&with_media).unwrap();
        drop(writer);
        let content = fs_err::read_to_string(out.join(MEDIA_LIST_FILE)).unwrap();
        assert_eq!(content.lines().count(), 2);
        let writer = MediaListWriter::create(&out, false).unwrap();
        drop(writer);
        let content = fs_err::read_to_string(out.join(MEDIA_LIST_FILE)).unwrap();
        assert_eq!(content.lines().count(), 0);

        fs_err::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn collect_audio_urls_from_content() {
        let audio = video_block(serde_json::json!({
            "type": "audio",
            "media": { "url": "https://a.tumblr.com/track.mp3" }
        }));
        let post = make_post(vec![audio], vec![]);
        assert_eq!(collect_audio_urls(&post), vec!["https://a.tumblr.com/track.mp3"]);
    }

    #[test]
    fn rewrite_covers_video_and_audio_blocks() {
        let video = video_block(serde_json::json!({
            "type": "video",
            "media": { "url": "https://vt.tumblr.com/clip.mp4" }
        }));
        let audio = video_block(serde_json::json!({
            "type": "audio",
            "media": { "url": "https://a.tumblr.com/track.mp3" }
        }));
        let post = make_post(vec![video, audio], vec![]);

        let mut url_map = HashMap::new();
        url_map.insert(
            "https://vt.tumblr.com/clip.mp4".to_owned(),
            "media/videos/local.mp4".to_owned(),
        );
        url_map.insert(
            "https://a.tumblr.com/track.mp3".to_owned(),
            "media/audio/local.mp3".to_owned(),
        );

        let result = rewrite_post_media_urls(&post, &url_map);
        assert!(matches!(result, Cow::Owned(_)));
        match &result.content[0] {
            ContentBlock::Video { media: Some(m), .. } => {
                assert_eq!(m.url, "media/videos/local.mp4");
            }
            _ => panic!("expected Video block"),
        }
        match &result.content[1] {
            ContentBlock::Audio { media: Some(m), .. } => {
                assert_eq!(m.url, "media/audio/local.mp3");
            }
            _ => panic!("expected Audio block"),
        }
    }
}
