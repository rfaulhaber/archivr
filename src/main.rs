use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use archivr::{
    Args, BackupState, JobState, PostNotes, PostRenderer, PostTimestamp, ResolvedConfig, api,
    auth::authenticate,
    fsutil::write_atomic,
    images::{
        MAX_CONCURRENT_DOWNLOADS, collect_image_urls, download_images, rewrite_post_image_urls,
    },
    notes::fetch_post_notes,
    template::{OLDER_NAV_PLACEHOLDER, build_older_nav_link},
};
use clap::Parser;
use crabrave::Crabrave;
use crabrave::handlers::blog::{Post, PostsResponse};
use tokio::sync::Semaphore;

const PROJECT_QUALIFIER: &str = "com.ryanfaulhaber";
const PROJECT_NAME: &str = "archivr";

/// Writes a progress line to stdout, treating a closed pipe (e.g. `archivr ... | head`)
/// as success so the backup keeps running even after the reader goes away.
macro_rules! say {
    ($($arg:tt)*) => {
        api::say(format_args!($($arg)*))
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();
    log::debug!("args: {:?}", args);

    let mut config = ResolvedConfig::from_args(args)?;

    let project_dir = directories::ProjectDirs::from(PROJECT_QUALIFIER, "", PROJECT_NAME)
        .ok_or_else(|| anyhow::anyhow!("Could not determine project directory"))?;
    let data_dir = camino::Utf8Path::from_path(project_dir.data_local_dir())
        .ok_or_else(|| anyhow::anyhow!("Data directory path is not valid UTF-8"))?;

    let client = authenticate(
        &config.consumer_key,
        &config.consumer_secret,
        data_dir,
        config.reauth,
        config.cookies_file.as_deref(),
        config.dashboard,
        config.headless,
    )
    .await?;

    if !fs_err::exists(&config.output_dir)? {
        fs_err::create_dir_all(&config.output_dir)?;
    }

    let state_file = BackupState::state_file_path(&config.output_dir);
    let prior_state = BackupState::load(&state_file).ok();

    if config.incremental {
        match prior_state.as_ref() {
            Some(s) if s.blog_name == config.blog_name => {
                // Tumblr's `after` filter is exclusive on the second-granularity
                // timestamp, so the boundary post is not re-fetched. Posts that
                // share that exact second but were not yet archived could be
                // missed; the window is one second and Tumblr timestamps rarely
                // collide, so we accept it rather than re-fetching a full second.
                config.after = Some(s.newest_post_timestamp);
                if !config.quiet {
                    say!(
                        "Incremental backup: fetching posts newer than {} (last archived post)",
                        s.newest_post_timestamp
                    )?;
                }
            }
            Some(s) => {
                return Err(anyhow::anyhow!(
                    "state file at {} is for blog `{}`, but `{}` was requested",
                    state_file,
                    s.blog_name,
                    config.blog_name
                ));
            }
            None => {
                if !config.quiet {
                    say!(
                        "Incremental backup: no prior state found, performing a full backup to establish baseline"
                    )?;
                }
            }
        }
    }

    let job_file = JobState::job_file_path(&config.output_dir);
    let mut loaded_job: Option<JobState> = None;
    if config.resume && fs_err::exists(&job_file).unwrap_or(false) {
        match JobState::load(&job_file) {
            Ok(loaded) => {
                if loaded.blog_name != config.blog_name {
                    return Err(anyhow::anyhow!(
                        "job file at {} is for blog `{}`, but `{}` was requested. \
                         Resume from the matching output directory or omit --resume to start fresh.",
                        job_file,
                        loaded.blog_name,
                        config.blog_name
                    ));
                }
                loaded_job = Some(loaded);
            }
            Err(e) => {
                // Not gated on --quiet: silently restarting from the beginning
                // when the user explicitly asked to resume is the kind of
                // surprise worth a line of output.
                say!(
                    "Warning: could not read the saved job file at {job_file} ({e}); \
                     starting a new backup from the beginning."
                )?;
            }
        }
    }

    let resuming_existing_job = loaded_job.is_some();
    let mut job = loaded_job.unwrap_or_else(|| JobState::new(&config));

    if resuming_existing_job {
        config.apply_job_state(&job);
        let mut params = Vec::new();
        if job.json {
            params.push("--json".to_owned());
        }
        if job.directories {
            params.push("--directories".to_owned());
        }
        if job.save_images {
            params.push("--save-images".to_owned());
        }
        if job.save_notes {
            params.push("--save-notes".to_owned());
        }
        if let Some(n) = job.notes_limit {
            params.push(format!("--notes-limit {n}"));
        }
        if job.drafts {
            params.push("--drafts".to_owned());
        }
        if job.queue {
            params.push("--queue".to_owned());
        }
        if job.submissions {
            params.push("--submissions".to_owned());
        }
        if let Some(before) = job.before {
            params.push(format!("--before {before}"));
        }
        if let Some(after) = job.after {
            params.push(format!("--after {after}"));
        }
        if let Some(ref tpl) = job.template_path {
            params.push(format!("--template {tpl}"));
        }
        if !config.quiet {
            if params.is_empty() {
                say!("Resuming with default parameters")?;
            } else {
                say!("Resuming with saved parameters: {}", params.join(" "))?;
            }
        }

        if job.before_cursor.is_none() {
            say!(
                "Note: this job file was written by an older archivr; re-scanning the \
                 backup window from the top (already-saved posts are reused)."
            )?;
        }
    }

    let renderer = if config.json {
        None
    } else if let Some(ref template_path) = config.template_path {
        Some(PostRenderer::from_file(template_path)?)
    } else {
        Some(PostRenderer::new()?)
    };

    if !config.quiet {
        if resuming_existing_job {
            say!("Backing up {} (resuming previous job)...", config.blog_name)?;
        } else if config.incremental && config.after.is_some() {
            say!("Backing up {} (incremental)...", config.blog_name)?;
        } else {
            say!("Backing up {}...", config.blog_name)?;
        }
    }

    if config.directories && config.save_images {
        let migrated = migrate_directories_media_layout(&config.output_dir)?;
        if migrated > 0 && !config.quiet {
            say!(
                "Linked {migrated} previously downloaded images from per-post media \
                 directories into the shared media store (media/images/)."
            )?;
        }
    }

    let newest_observed =
        run_backup(&client, &config, renderer.as_ref(), &mut job, &job_file).await?;

    if let Some(newest_ts) = newest_observed {
        let now = chrono::Utc::now().timestamp();
        // Only floor against prior state from the same blog: a non-incremental
        // run into another blog's directory must not inherit that blog's mark.
        let prior_high_water = prior_state
            .as_ref()
            .filter(|s| s.blog_name == config.blog_name)
            .map(|s| s.newest_post_timestamp)
            .unwrap_or(0);
        // The mark asserts "everything up to this timestamp is archived", so
        // it may only advance when this run's coverage is contiguous with the
        // archive below it. A run floored by --after above the prior mark (or
        // into a fresh directory) leaves a gap the mark must not paper over.
        // Incremental runs pass trivially: their floor IS the prior mark.
        if config.after.is_none_or(|floor| floor <= prior_high_water) {
            let new_state = BackupState {
                blog_name: config.blog_name.clone(),
                last_run_at: now,
                // Tumblr allows arbitrary post dates; cap the mark at the
                // present so a single future-dated post can't push the
                // incremental boundary past posts not yet written.
                newest_post_timestamp: newest_ts.max(prior_high_water).min(now),
            };
            new_state.save(&state_file)?;
        } else {
            log::info!(
                "not updating incremental state: this run's --after floor is above \
                 the last archived post, so posts below it remain unarchived"
            );
        }
    }

    // Special collections run after the main backup but before the job file
    // is deleted: if one fails, --resume finishes the (already-complete) main
    // window quickly and re-runs the specials wholesale.
    for kind in SpecialKind::ALL {
        if kind.enabled(&config) {
            backup_special(&client, &config, renderer.as_ref(), kind).await?;
        }
    }

    if fs_err::exists(&job_file)? {
        JobState::delete(&job_file)?;
    }

    if !config.quiet {
        say!("Backup complete.")?;
    }

    Ok(())
}

/// An HTML post already on disk with [`OLDER_NAV_PLACEHOLDER`] still in place,
/// kept in memory so the file can be rewritten with the real "older" nav link
/// once its older neighbor is known.
struct BufferedHtmlPost {
    content: String,
    output_file: camino::Utf8PathBuf,
    /// Relative href for *this* post (used as `newer_href` by the next post).
    relative_href: String,
}

/// Compute the relative href for a post given its ID and the output mode.
fn post_relative_href(post_id: &str, directories: bool) -> String {
    if directories {
        format!("../{post_id}/")
    } else {
        format!("{post_id}.html")
    }
}

/// Replace the older-nav placeholder with the real link and rewrite the file on disk.
fn finalize_buffered_post(buf: &BufferedHtmlPost, older_html: &str) -> anyhow::Result<()> {
    let final_content = buf.content.replace(OLDER_NAV_PLACEHOLDER, older_html);
    write_atomic(&buf.output_file, final_content.as_bytes())?;
    Ok(())
}

/// Archives written by pre-cursor versions kept each post's images in
/// `<post_id>/media/`; the shared store lives at `media/images/`. Link the old
/// files into the store so they are recognized as already downloaded instead
/// of re-fetched. Hard links (with a copy fallback) keep the originals in
/// place, so old posts' HTML continues to work untouched.
///
/// Runs only until the shared store exists — its creation, here or by the
/// first download, marks the archive as migrated.
fn migrate_directories_media_layout(output_dir: &camino::Utf8Path) -> anyhow::Result<u64> {
    let store = output_dir.join("media").join("images");
    if fs_err::exists(&store)? {
        return Ok(0);
    }

    let mut migrated: u64 = 0;
    let entries = match fs_err::read_dir(output_dir) {
        Ok(entries) => entries,
        // A missing/unreadable output dir surfaces later with a better error.
        Err(_) => return Ok(0),
    };

    fs_err::create_dir_all(&store)?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Old layout used the numeric post id as the directory name.
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let media_dir = output_dir.join(name).join("media");
        let Ok(files) = fs_err::read_dir(&media_dir) else {
            continue;
        };
        for file in files.flatten() {
            let file_name = file.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let src = media_dir.join(file_name);
            let dest = store.join(file_name);
            if fs_err::exists(&dest).unwrap_or(false) {
                continue;
            }
            let linked =
                fs_err::hard_link(&src, &dest).or_else(|_| fs_err::copy(&src, &dest).map(|_| ()));
            match linked {
                Ok(()) => migrated += 1,
                Err(e) => log::warn!("could not migrate {src} into the media store: {e}"),
            }
        }
    }

    Ok(migrated)
}

async fn run_backup(
    client: &Crabrave,
    config: &ResolvedConfig,
    renderer: Option<&PostRenderer<'_>>,
    job: &mut JobState,
    job_file: &camino::Utf8Path,
) -> anyhow::Result<Option<PostTimestamp>> {
    let mut posts_archived: u64 = 0;
    let mut buffered: Option<BufferedHtmlPost> = None;
    let mut newest_observed: Option<PostTimestamp> = None;
    // Ids processed this run. Window jumps deliberately re-include the boundary
    // second (so same-second posts can never be skipped), which makes re-seeing
    // a few posts per page normal; this set keeps them from being re-processed.
    // Seeded from the resumed job's last committed page so those files (already
    // complete on disk) aren't rewritten with a severed nav chain.
    let mut seen_ids: HashSet<String> = job.last_page_ids.iter().cloned().collect();
    let mut stalled_pages: u32 = 0;

    // Nav-chain handoff across a --resume process boundary: the first post this
    // run writes still links "newer" to the previous run's last post, and that
    // post's placeholder older-slot gets finalized against it.
    let resume_newer_href: Option<String> = job.last_post_href.clone();
    let mut resume_heal_file: Option<camino::Utf8PathBuf> = job.last_post_file.clone();

    // One download pool and one media store for the whole run: the shared
    // store lets the hash-prefixed filenames dedup reblogged images across
    // posts, which per-post media dirs used to defeat in --directories mode.
    let download_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let media_dir = config.output_dir.join("media").join("images");
    let relative_media_prefix = if config.directories {
        "../media/images/"
    } else {
        "media/images/"
    };

    // Pin the top of the pagination window when the job starts (+1 so a post
    // stamped in the start second is included even with an exclusive bound):
    // posts published mid-run then can't shift pages under us. A resumed job
    // continues from its saved cursor instead.
    let mut cursor: PostTimestamp = match job.before_cursor {
        Some(saved) => saved,
        None => config.before.unwrap_or(job.started_at + 1),
    };
    let mut window_offset: u64 = job.window_offset;

    let loop_result: anyhow::Result<()> = async {
        let mut pending_fetch = Some(spawn_fetch(client, config, cursor, window_offset));
        loop {
            let Some(fetch) = pending_fetch.take() else {
                break;
            };
            let response = fetch
                .await
                .map_err(|e| anyhow::anyhow!("page fetch task failed: {e}"))??;

            if response.posts.is_empty() {
                log::info!("no more posts to fetch, ending backup");
                break;
            }

            // `before`-window pages are documented reverse-chronological, but
            // fold the max over every batch rather than trusting ordering.
            let batch_newest = response.posts.iter().map(|p| p.timestamp).max();
            newest_observed = newest_observed.max(batch_newest);

            let batch_min = response
                .posts
                .iter()
                .map(|p| p.timestamp)
                .min()
                .unwrap_or(cursor);
            let (next_cursor, next_window_offset) =
                advance_window(cursor, window_offset, batch_min, response.posts.len());

            // `--after` (and incremental mode) act as a client-side floor:
            // `before`-windows have a guaranteed order, unlike server-side
            // `after` filtering, whose order Tumblr never documents.
            let reached_floor = config
                .after
                .is_some_and(|floor| response.posts.iter().any(|p| p.timestamp <= floor));

            let new_posts: Vec<&Post> = response
                .posts
                .iter()
                .filter(|p| !seen_ids.contains(&p.id))
                .filter(|p| config.after.is_none_or(|floor| p.timestamp > floor))
                .collect();

            // If pages stop yielding unseen posts the server is likely not
            // honoring our pagination (e.g. `offset` within a window); abort
            // rather than refetch the same page forever.
            if new_posts.is_empty() && !reached_floor {
                stalled_pages += 1;
                if stalled_pages >= MAX_STALLED_PAGES {
                    return Err(anyhow::anyhow!(
                        "pagination made no progress for {MAX_STALLED_PAGES} consecutive \
                         pages; aborting rather than refetching the same posts forever. \
                         Use --resume to try again."
                    ));
                }
            } else {
                stalled_pages = 0;
            }

            // Start fetching the next page now, so the API round-trip runs
            // concurrently with this batch's downloads and rendering.
            if !reached_floor {
                pending_fetch = Some(spawn_fetch(client, config, next_cursor, next_window_offset));
            }

            log::info!(
                "fetched {} posts (cursor {cursor}, {} new)",
                response.posts.len(),
                new_posts.len()
            );

            // Download the whole batch's images through the shared pool before
            // rendering, so single-image posts no longer serialize downloads.
            let url_map: HashMap<String, String> = if config.save_images {
                let urls: Vec<String> = new_posts
                    .iter()
                    .flat_map(|p| collect_image_urls(p))
                    .collect();
                if urls.is_empty() {
                    HashMap::new()
                } else {
                    download_images(
                        client.client(),
                        &urls,
                        &media_dir,
                        relative_media_prefix,
                        &download_semaphore,
                    )
                    .await?
                }
            } else {
                HashMap::new()
            };

            for &post in &new_posts {
                ensure_numeric_id(&post.id)?;

                log::info!("processing post {}", post.id);

                let post_notes = if config.save_notes {
                    fetch_notes_for_post(client, config, post).await?
                } else {
                    None
                };

                let post: Cow<'_, Post> = if url_map.is_empty() {
                    Cow::Borrowed(post)
                } else {
                    rewrite_post_image_urls(post, &url_map)
                };

                if config.json {
                    // JSON mode: write immediately, no navigation
                    let content = render_json(&post, post_notes.as_ref())?;
                    let output_file = if config.directories {
                        let post_dir = config.output_dir.join(&post.id);
                        if !fs_err::exists(&post_dir)? {
                            fs_err::create_dir(&post_dir)?;
                        }
                        post_dir.join("index.json")
                    } else {
                        config.output_dir.join(format!("{}.json", post.id))
                    };
                    write_atomic(&output_file, content.as_bytes())?;
                    log::debug!("saved post {} to {}", post.id, output_file);
                } else {
                    // HTML mode: buffer for nav link injection
                    let r = renderer
                        .ok_or_else(|| anyhow::anyhow!("renderer is required for HTML mode"))?;

                    let newer_href = buffered
                        .as_ref()
                        .map(|b| b.relative_href.as_str())
                        .or(resume_newer_href.as_deref());
                    let content = r.render(&post, newer_href, post_notes.as_ref())?;

                    let output_file = if config.directories {
                        let post_dir = config.output_dir.join(&post.id);
                        if !fs_err::exists(&post_dir)? {
                            fs_err::create_dir(&post_dir)?;
                        }
                        post_dir.join("index.html")
                    } else {
                        config.output_dir.join(format!("{}.html", post.id))
                    };

                    let current_href = post_relative_href(&post.id, config.directories);

                    // Persist this post before the batch's offset can be saved past
                    // it: the placeholder is an HTML comment, so the on-disk copy
                    // is complete minus the "older" link, and an interrupt at any
                    // point loses nothing. Written before the previous post gains
                    // its "older" link so no on-disk link ever targets a missing
                    // file.
                    write_atomic(&output_file, content.as_bytes())?;

                    // Finalize the previous buffered post now that we know its "older" neighbor
                    if let Some(prev) = buffered.take() {
                        let older_link = build_older_nav_link(&current_href);
                        finalize_buffered_post(&prev, &older_link)?;
                    } else if let Some(prev_file) = resume_heal_file.take() {
                        // First post of a resumed run: the previous run's last
                        // post is on disk with its older-slot placeholder
                        // intact; link it to this post.
                        heal_resumed_older_link(&prev_file, &build_older_nav_link(&current_href))?;
                    }

                    buffered = Some(BufferedHtmlPost {
                        content,
                        output_file,
                        relative_href: current_href,
                    });
                }

                posts_archived += 1;
            }

            // Mark every post in the raw batch (not just the processed ones)
            // so boundary-second re-fetches after a window jump are skipped.
            for post in &response.posts {
                seen_ids.insert(post.id.clone());
            }

            // Commit the cursor only after the batch's files are on disk; an
            // interrupt before this line re-fetches the batch on --resume
            // instead of skipping it.
            cursor = next_cursor;
            window_offset = next_window_offset;
            job.before_cursor = Some(cursor);
            job.window_offset = window_offset;
            job.last_page_ids = response.posts.iter().map(|p| p.id.clone()).collect();
            if let Some(b) = buffered.as_ref() {
                job.last_post_href = Some(b.relative_href.clone());
                job.last_post_file = Some(b.output_file.clone());
            }
            job.save(job_file)?;

            if !config.quiet {
                say!("  {} posts archived", posts_archived)?;
            }

            if reached_floor {
                log::info!("reached the --after floor, ending backup");
                break;
            }
        }
        Ok(())
    }
    .await;

    // On the normal exit the last processed post is the genuinely-oldest one:
    // close its older slot. On an error path (e.g. a rate limit) leave the
    // placeholder in place — it is an HTML comment, so the file renders
    // identically, and a later --resume run can still finalize it against the
    // real older neighbour via the persisted last_post_file.
    if loop_result.is_ok()
        && let Some(last) = buffered.take()
    {
        finalize_buffered_post(&last, "<span></span>")?;
    }

    loop_result?;

    Ok(newest_observed)
}

/// The id becomes a filesystem path component and a nav href, so refuse
/// anything but the numeric ids Tumblr actually issues (defense-in-depth
/// against a hostile or proxied API).
fn ensure_numeric_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(anyhow::anyhow!(
            "API returned post id {id:?}, which is not numeric; \
             refusing to use it as a file name"
        ));
    }
    Ok(())
}

/// Serializes a post for --json output, embedding its captured notes under a
/// `notes` key when --save-notes is active.
fn render_json(post: &Post, notes: Option<&PostNotes>) -> anyhow::Result<String> {
    let content = match notes {
        Some(n) => {
            let mut value = serde_json::to_value(post)?;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("notes".to_owned(), serde_json::to_value(n)?);
            }
            serde_json::to_string_pretty(&value)?
        }
        None => serde_json::to_string_pretty(post)?,
    };
    Ok(content)
}

/// Fetches a post's notes under --save-notes. A client-side rejection (4xx)
/// of one post's notes degrades gracefully — the post is still archived,
/// without notes, and the failure is printed — while rate-limit or retry
/// exhaustion still aborts the run so --resume can pick it up.
async fn fetch_notes_for_post(
    client: &Crabrave,
    config: &ResolvedConfig,
    post: &Post,
) -> anyhow::Result<Option<PostNotes>> {
    if post.note_count == 0 {
        return Ok(Some(PostNotes::default()));
    }
    match fetch_post_notes(
        client,
        &config.blog_name,
        &post.id,
        config.notes_limit,
        config.quiet,
    )
    .await
    {
        Ok(n) => Ok(Some(n)),
        // A 4xx refusal of this one post, or an unparseable body that
        // outlived the retry budget (a WAF's HTML error page), should cost
        // this post its notes — not the whole run.
        Err(e) if api::is_client_api_error(&e) || api::is_parse_error(&e) => {
            // Not gated on --quiet: an archive silently missing notes
            // contradicts an explicit --save-notes.
            say!(
                "  Warning: could not fetch notes for post {} ({e:#}); \
                 archiving it without notes.",
                post.id
            )?;
            Ok(Some(PostNotes::default()))
        }
        Err(e) => Err(e),
    }
}

/// The authenticated-user-only post collections backed up alongside the blog.
#[derive(Clone, Copy)]
enum SpecialKind {
    Drafts,
    Queue,
    Submissions,
}

impl SpecialKind {
    const ALL: [Self; 3] = [Self::Drafts, Self::Queue, Self::Submissions];

    fn enabled(self, config: &ResolvedConfig) -> bool {
        match self {
            Self::Drafts => config.drafts,
            Self::Queue => config.queue,
            Self::Submissions => config.submissions,
        }
    }

    /// Subdirectory of the output dir the collection is written to.
    fn dir_name(self) -> &'static str {
        match self {
            Self::Drafts => "drafts",
            Self::Queue => "queue",
            Self::Submissions => "submissions",
        }
    }

    fn plural(self) -> &'static str {
        match self {
            Self::Drafts => "drafts",
            Self::Queue => "queued posts",
            Self::Submissions => "submissions",
        }
    }
}

/// Fetches one page of a special collection. Drafts paginate by `before_id`;
/// queue and submissions by offset.
async fn fetch_special_page(
    client: &Crabrave,
    blog_name: &str,
    kind: SpecialKind,
    before_id: Option<&str>,
    offset: u64,
    quiet: bool,
) -> anyhow::Result<Vec<Post>> {
    match kind {
        SpecialKind::Drafts => api::send_with_retries(
            || {
                let mut builder = client.blogs(blog_name.to_owned()).drafts();
                if let Some(id) = before_id {
                    builder = builder.before_id(id.to_owned());
                }
                builder.send()
            },
            quiet,
        )
        .await
        .map(|r| r.posts),
        SpecialKind::Queue => api::send_with_retries(
            || {
                let mut builder = client.blogs(blog_name.to_owned()).queue();
                if offset > 0 {
                    builder = builder.offset(offset);
                }
                builder.send()
            },
            quiet,
        )
        .await
        .map(|r| r.posts),
        SpecialKind::Submissions => api::send_with_retries(
            || {
                let mut builder = client.blogs(blog_name.to_owned()).submissions();
                if offset > 0 {
                    builder = builder.offset(offset);
                }
                builder.send()
            },
            quiet,
        )
        .await
        .map(|r| r.posts),
    }
}

/// Backs up one special collection (drafts, queue, or submissions) into its
/// subdirectory of the output dir. These sets are small and mutate freely
/// between runs (posts leave the queue when published), so no pagination
/// state is persisted: every run re-fetches the whole collection and
/// rewrites its files, meaning an interrupt here costs nothing on --resume.
async fn backup_special(
    client: &Crabrave,
    config: &ResolvedConfig,
    renderer: Option<&PostRenderer<'_>>,
    kind: SpecialKind,
) -> anyhow::Result<()> {
    if !config.quiet {
        say!("Backing up {}...", kind.plural())?;
    }

    let subdir = config.output_dir.join(kind.dir_name());
    if !fs_err::exists(&subdir)? {
        fs_err::create_dir_all(&subdir)?;
    }

    let download_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let media_dir = config.output_dir.join("media").join("images");
    // Files live one level below the output dir (two with --directories), so
    // the shared media store needs a deeper relative prefix than the main
    // backup uses.
    let relative_media_prefix = if config.directories {
        "../../media/images/"
    } else {
        "../media/images/"
    };

    let mut archived: u64 = 0;
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut before_id: Option<String> = None;
    let mut offset: u64 = 0;

    loop {
        let posts = fetch_special_page(
            client,
            &config.blog_name,
            kind,
            before_id.as_deref(),
            offset,
            config.quiet,
        )
        .await
        .with_context(|| {
            format!(
                "fetching {} (the blog must belong to the authenticated account)",
                kind.plural()
            )
        })?;
        if posts.is_empty() {
            break;
        }

        let new_posts: Vec<&Post> = posts.iter().filter(|p| !seen_ids.contains(&p.id)).collect();
        // A page of only re-seen posts means the server is not honoring our
        // pagination; stop rather than refetch the same page forever.
        if new_posts.is_empty() {
            break;
        }

        let url_map: HashMap<String, String> = if config.save_images {
            let urls: Vec<String> = new_posts
                .iter()
                .flat_map(|p| collect_image_urls(p))
                .collect();
            if urls.is_empty() {
                HashMap::new()
            } else {
                download_images(
                    client.client(),
                    &urls,
                    &media_dir,
                    relative_media_prefix,
                    &download_semaphore,
                )
                .await?
            }
        } else {
            HashMap::new()
        };

        for &post in &new_posts {
            ensure_numeric_id(&post.id)?;

            let post: Cow<'_, Post> = if url_map.is_empty() {
                Cow::Borrowed(post)
            } else {
                rewrite_post_image_urls(post, &url_map)
            };

            let output_file = if config.directories {
                let post_dir = subdir.join(&post.id);
                if !fs_err::exists(&post_dir)? {
                    fs_err::create_dir(&post_dir)?;
                }
                post_dir.join(if config.json { "index.json" } else { "index.html" })
            } else {
                let ext = if config.json { "json" } else { "html" };
                subdir.join(format!("{}.{ext}", post.id))
            };

            let content = if config.json {
                render_json(&post, None)?
            } else {
                let r = renderer
                    .ok_or_else(|| anyhow::anyhow!("renderer is required for HTML mode"))?;
                // No nav chain among drafts/queue/submissions: close the
                // older slot immediately.
                r.render(&post, None, None)?
                    .replace(OLDER_NAV_PLACEHOLDER, "<span></span>")
            };
            write_atomic(&output_file, content.as_bytes())?;
            archived += 1;
        }

        for post in &posts {
            seen_ids.insert(post.id.clone());
        }
        // Overlap the offset stride by one post, mirroring advance_window's
        // deletion insurance: the queue is guaranteed to shrink under us
        // (Tumblr publishes from it on a schedule), and a full stride would
        // silently skip the post that shifts into the gap. Re-included posts
        // are filtered by seen_ids.
        offset += (posts.len() as u64).saturating_sub(1).max(1);
        before_id = posts.last().map(|p| p.id.clone());
    }

    if !config.quiet {
        say!("  {} {} archived", archived, kind.plural())?;
    }

    Ok(())
}

/// Replaces the older-nav placeholder in a previous run's last written post,
/// linking it to the first post of the resumed run. Best-effort: if the file
/// is gone or was already finalized (no placeholder), leave it alone.
fn heal_resumed_older_link(file: &camino::Utf8Path, older_html: &str) -> anyhow::Result<()> {
    let Ok(content) = fs_err::read_to_string(file) else {
        return Ok(());
    };
    if content.contains(OLDER_NAV_PLACEHOLDER) {
        let healed = content.replace(OLDER_NAV_PLACEHOLDER, older_html);
        write_atomic(file, healed.as_bytes())?;
    }
    Ok(())
}

/// Consecutive pages yielding no unseen posts before concluding pagination is
/// not making progress.
const MAX_STALLED_PAGES: u32 = 3;

/// Fetches a page on a separate task so the API round-trip (including any
/// rate-limit sleep) overlaps with processing of the previous batch.
fn spawn_fetch(
    client: &Crabrave,
    config: &ResolvedConfig,
    cursor: PostTimestamp,
    window_offset: u64,
) -> tokio::task::JoinHandle<anyhow::Result<PostsResponse>> {
    let client = client.clone();
    let blog_name = config.blog_name.clone();
    let quiet = config.quiet;
    tokio::spawn(async move { fetch_page(&client, &blog_name, cursor, window_offset, quiet).await })
}

/// Fetches one page of posts with the shared timeout/retry/rate-limit policy.
async fn fetch_page(
    client: &Crabrave,
    blog_name: &str,
    before_cursor: PostTimestamp,
    window_offset: u64,
    quiet: bool,
) -> anyhow::Result<PostsResponse> {
    api::send_with_retries(
        || {
            let mut builder = client
                .blogs(blog_name.to_owned())
                .posts()
                .before(before_cursor);
            if window_offset > 0 {
                builder = builder.offset(window_offset);
            }
            builder.send()
        },
        quiet,
    )
    .await
}

/// Decides the next pagination window after a fetched batch.
///
/// Normally the cursor jumps to just above the oldest timestamp in the batch:
/// the next window then re-includes that boundary second, so same-second posts
/// straddling a page break can never be skipped (re-seen ones are filtered by
/// id). When an entire page shares one second the cursor cannot move, so page
/// within the unchanged window by offset instead — overlapping the stride by
/// one so a single deletion inside that second shifts a seen post into view
/// (a re-fetch) rather than an unseen one out of it (a skip). A deletion
/// anywhere outside the current same-second window can only cause re-fetches;
/// several deletions inside one >page-sized second between two fetches can
/// still lose a post, an unavoidable limit of Tumblr's before+offset API.
fn advance_window(
    cursor: PostTimestamp,
    window_offset: u64,
    batch_min: PostTimestamp,
    batch_len: usize,
) -> (PostTimestamp, u64) {
    if batch_min + 1 < cursor {
        (batch_min + 1, 0)
    } else {
        let stride = (batch_len as u64).saturating_sub(1).max(1);
        (cursor, window_offset + stride)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn advance_window_jumps_past_boundary_second() {
        // Normal page: oldest post well below the cursor → jump to just above
        // it, so the next window re-includes the boundary second.
        assert_eq!(advance_window(1_000, 0, 500, 20), (501, 0));
    }

    #[test]
    fn advance_window_pages_within_single_second_run() {
        // Entire page shares the second just below the cursor: the cursor
        // cannot advance, so walk the same window by offset, overlapping the
        // stride by one post as deletion insurance.
        assert_eq!(advance_window(501, 0, 500, 20), (501, 19));
        assert_eq!(advance_window(501, 19, 500, 20), (501, 38));
    }

    #[test]
    fn advance_window_offset_stride_always_progresses() {
        // A single-post page must still advance the offset despite the
        // one-post overlap rule.
        assert_eq!(advance_window(501, 5, 500, 1), (501, 6));
    }

    #[test]
    fn advance_window_handles_inclusive_bound() {
        // If the server treats `before` inclusively a post can share the
        // cursor's own timestamp; the window must not move upward.
        assert_eq!(advance_window(500, 0, 500, 20), (500, 19));
    }

    #[test]
    fn advance_window_resets_offset_on_jump() {
        assert_eq!(advance_window(501, 40, 100, 20), (101, 0));
    }

    #[test]
    fn post_relative_href_modes() {
        assert_eq!(post_relative_href("123", false), "123.html");
        assert_eq!(post_relative_href("123", true), "../123/");
    }

    #[test]
    fn ensure_numeric_id_accepts_digits_only() {
        assert!(ensure_numeric_id("1234567890").is_ok());
        assert!(ensure_numeric_id("").is_err());
        assert!(ensure_numeric_id("../etc/passwd").is_err());
        assert!(ensure_numeric_id("123abc").is_err());
    }

    fn minimal_post() -> Post {
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
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn render_json_without_notes_matches_plain_post() {
        let post = minimal_post();
        let json = render_json(&post, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("notes").is_none());
        // Post deserializes from `id_string` but serializes back out as `id`.
        assert_eq!(value["id"], "123");
    }

    #[test]
    fn render_json_embeds_notes() {
        let post = minimal_post();
        let notes = PostNotes {
            total_notes: 1,
            notes: vec![
                serde_json::from_value(serde_json::json!({
                    "type": "like",
                    "timestamp": 1700000100,
                    "blog_name": "liker",
                }))
                .unwrap(),
            ],
            rollup_notes: vec![],
        };
        let json = render_json(&post, Some(&notes)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["notes"]["total_notes"], 1);
        assert_eq!(value["notes"]["notes"][0]["blog_name"], "liker");
        // The post's own fields are still present alongside the notes.
        assert_eq!(value["id"], "123");
    }
}
