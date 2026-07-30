use std::borrow::Cow;
use std::io::Write;

use archivr::{
    Args, BackupState, JobState, PostRenderer, PostTimestamp, ResolvedConfig, auth::authenticate,
    fsutil::write_atomic,
    images::{collect_image_urls, download_images, rewrite_post_image_urls},
    template::{OLDER_NAV_PLACEHOLDER, build_older_nav_link},
};
use clap::Parser;
use crabrave::Crabrave;
use crabrave::handlers::blog::Post;

const PROJECT_QUALIFIER: &str = "com.ryanfaulhaber";
const PROJECT_NAME: &str = "archivr";

/// Writes a progress line to stdout, treating a closed pipe (e.g. `archivr ... | head`)
/// as success so the backup keeps running even after the reader goes away.
macro_rules! say {
    ($($arg:tt)*) => {{
        match writeln!(std::io::stdout(), $($arg)*) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            other => other,
        }
    }};
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
    let resuming_existing_job = config.resume && fs_err::exists(&job_file).unwrap_or(false);
    let mut job = if resuming_existing_job {
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
                loaded
            }
            Err(e) => {
                log::warn!("failed to load job file ({e}); starting a new backup");
                JobState::new(&config)
            }
        }
    } else {
        JobState::new(&config)
    };

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
                say!(
                    "Resuming with saved parameters: {}",
                    params.join(" ")
                )?;
            }
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
        if config.resume {
            say!(
                "Backing up {} (resuming previous job)...",
                config.blog_name
            )?;
        } else if config.incremental && config.after.is_some() {
            say!("Backing up {} (incremental)...", config.blog_name)?;
        } else {
            say!("Backing up {}...", config.blog_name)?;
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
        let new_state = BackupState {
            blog_name: config.blog_name.clone(),
            last_run_at: now,
            // Tumblr allows arbitrary post dates; cap the mark at the present
            // so a single future-dated post can't push the incremental
            // boundary past posts that haven't been written yet.
            newest_post_timestamp: newest_ts.max(prior_high_water).min(now),
        };
        new_state.save(&state_file)?;
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

    let loop_result: anyhow::Result<()> = async {
        loop {
        let mut post_builder = client
            .blogs(config.blog_name.clone())
            .posts()
            .offset(job.offset);

        if let Some(before) = config.before {
            post_builder = post_builder.before(before);
        }

        if let Some(after) = config.after {
            post_builder = post_builder.after(after);
        }

        let post_response = post_builder.send().await;

        if let Err(crabrave::CrabError::RateLimit { retry_after }) = post_response {
            let msg = match retry_after {
                Some(i) => format!(
                    "Hit rate limit. Retry after {i} seconds. \
                     Use --resume to continue from where you left off."
                ),
                None => "Hit rate limit, please retry later. \
                         Use --resume to continue from where you left off."
                    .into(),
            };

            return Err(anyhow::anyhow!(msg));
        }

        let post_response = post_response?;

        if post_response.posts.is_empty() {
            log::info!("no more posts to fetch, ending backup");
            break;
        }

        log::info!("({}) Fetching next batch of posts...", job.offset,);

        // Tumblr never documents a sort order for `after`-filtered queries, so
        // fold the max over every batch rather than trusting the first batch
        // to hold the newest post.
        let batch_newest = post_response.posts.iter().map(|p| p.timestamp).max();
        newest_observed = newest_observed.max(batch_newest);

        for post in &post_response.posts {
            log::info!("processing post {}", post.id);

            let post: Cow<'_, Post> = if config.save_images {
                let urls = collect_image_urls(post);
                if urls.is_empty() {
                    Cow::Borrowed(post)
                } else {
                    let (media_dir, relative_prefix) = if config.directories {
                        let dir = config.output_dir.join(&post.id).join("media");
                        (dir, "media/".to_owned())
                    } else {
                        let dir = config.output_dir.join("media").join("images");
                        (dir, "media/images/".to_owned())
                    };

                    let url_map =
                        download_images(client.client(), &urls, &media_dir, &relative_prefix)
                            .await;
                    rewrite_post_image_urls(post, &url_map)
                }
            } else {
                Cow::Borrowed(post)
            };

            if config.json {
                // JSON mode: write immediately, no navigation
                let content = serde_json::to_string_pretty(&*post)?;
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

                let newer_href = buffered.as_ref().map(|b| b.relative_href.as_str());
                let content = r.render(&post, newer_href)?;

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
                }

                buffered = Some(BufferedHtmlPost {
                    content,
                    output_file,
                    relative_href: current_href,
                });
            }

            posts_archived += 1;
        }

            job.offset += post_response.posts.len() as u64;
            job.save(job_file)?;

            if !config.quiet {
                say!("  {} posts archived", posts_archived)?;
            }
        }
        Ok(())
    }
    .await;

    // The last processed post is already on disk but still carries the nav
    // placeholder; swap in an empty older slot. On the normal exit this is the
    // genuinely-oldest post; on an error path (e.g. a rate limit) its older
    // neighbour may only be fetched on a later --resume run.
    if let Some(last) = buffered.take() {
        finalize_buffered_post(&last, "<span></span>")?;
    }

    loop_result?;

    Ok(newest_observed)
}
