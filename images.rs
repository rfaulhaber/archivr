use std::borrow::Cow;
use std::collections::HashMap;

use camino::Utf8Path;
use crabrave::handlers::blog::Post;
use crabrave::npf::{ContentBlock, MediaObject};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

/// Maximum number of concurrent image downloads.
const MAX_CONCURRENT_DOWNLOADS: usize = 6;

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

/// Downloads images concurrently and returns a mapping of original URL to local relative path.
///
/// Only successful downloads appear in the returned map. Failed downloads are logged
/// and omitted so that original CDN URLs are preserved in the output.
pub async fn download_images(
    client: &reqwest::Client,
    urls: &[String],
    media_dir: &Utf8Path,
    relative_prefix: &str,
) -> HashMap<String, String> {
    // Deduplicate URLs
    let unique_urls: Vec<&String> = {
        let mut seen = std::collections::HashSet::new();
        urls.iter().filter(|u| seen.insert(u.as_str())).collect()
    };

    if unique_urls.is_empty() {
        return HashMap::new();
    }

    if !fs_err::exists(media_dir).unwrap_or(false)
        && let Err(e) = fs_err::create_dir_all(media_dir)
    {
        log::warn!("Failed to create media directory {media_dir}: {e}");
        return HashMap::new();
    }

    let semaphore = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let mut join_set = tokio::task::JoinSet::new();

    for url in unique_urls {
        let filename = local_filename_for_url(url);
        let dest = media_dir.join(&filename);

        // Skip already-downloaded images
        if fs_err::exists(&dest).unwrap_or(false) {
            log::debug!("Skipping already-downloaded image: {url}");
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

            match download_one(&client, &url_clone, &dest_clone).await {
                Ok(()) => {
                    log::debug!("Downloaded image: {url_clone} -> {dest_clone}");
                    Some((url_clone, rel_path))
                }
                Err(e) => {
                    log::warn!("Failed to download image {url_clone}: {e}");
                    None
                }
            }
        });
    }

    let mut url_map = HashMap::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(Some((url, path))) = result {
            url_map.insert(url, path);
        }
    }

    url_map
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Utf8Path,
) -> anyhow::Result<()> {
    let response = client.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status} for {url}"));
    }
    let bytes = response.bytes().await?;
    fs_err::write(dest, &bytes)?;
    Ok(())
}

/// Rewrites image URLs in a post, returning a modified clone.
///
/// Only URLs present in `url_map` (successful downloads) are rewritten.
/// URLs not in the map (failed downloads) remain as original CDN URLs.
///
/// Returns `Cow::Borrowed` if no URLs need rewriting, avoiding unnecessary clones.
pub fn rewrite_post_image_urls<'a>(
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
        .any(|block| {
            if let ContentBlock::Image { media, .. } = block {
                media.iter().any(|m| url_map.contains_key(&m.url))
            } else {
                false
            }
        });

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

fn rewrite_blocks(blocks: &mut [ContentBlock], url_map: &HashMap<String, String>) {
    for block in blocks {
        if let ContentBlock::Image { media, .. } = block {
            for media_obj in media {
                if let Some(local_path) = url_map.get(&media_obj.url) {
                    *media_obj = MediaObject {
                        url: local_path.clone(),
                        ..media_obj.clone()
                    };
                }
            }
        }
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
