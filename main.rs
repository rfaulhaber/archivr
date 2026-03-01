use std::borrow::Cow;
use std::io::Write;

use archivr::{
    Args, JobState, PostRenderer, PostTimestamp, ResolvedConfig, auth::authenticate,
    images::{collect_image_urls, download_images, rewrite_post_image_urls},
    template::{OLDER_NAV_PLACEHOLDER, build_older_nav_link},
};
use clap::Parser;
use crabrave::Crabrave;
use crabrave::handlers::blog::Post;

const PROJECT_QUALIFIER: &str = "com.ryanfaulhaber";
const PROJECT_NAME: &str = "archivr";

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
    )
    .await?;

    if !fs_err::exists(&config.output_dir)? {
        fs_err::create_dir_all(&config.output_dir)?;
    }

    let job_file = JobState::job_file_path(&config.output_dir);
    let mut job = if config.resume {
        JobState::load(&job_file).unwrap_or_else(|_| JobState::new(&config))
    } else {
        JobState::new(&config)
    };

    if config.resume && fs_err::exists(&job_file).unwrap_or(false) {
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
                writeln!(std::io::stdout(), "Resuming with default parameters")?;
            } else {
                writeln!(
                    std::io::stdout(),
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
            writeln!(
                std::io::stdout(),
                "Backing up {} (resuming previous job)...",
                config.blog_name
            )?;
        } else {
            writeln!(std::io::stdout(), "Backing up {}...", config.blog_name)?;
        }
    }

    let _ = run_backup(&client, &config, renderer.as_ref(), &mut job, &job_file).await?;

    if fs_err::exists(&job_file)? {
        JobState::delete(&job_file)?;
    }

    if !config.quiet {
        writeln!(std::io::stdout(), "Backup complete.")?;
    }

    Ok(())
}

/// A buffered HTML post waiting for its "older" nav link before being written to disk.
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

/// Replace the older-nav placeholder with the real link and write the file to disk.
fn finalize_buffered_post(buf: &BufferedHtmlPost, older_html: &str) -> anyhow::Result<()> {
    let final_content = buf.content.replace(OLDER_NAV_PLACEHOLDER, older_html);
    fs_err::write(&buf.output_file, &final_content)?;
    Ok(())
}

async fn run_backup(
    client: &Crabrave,
    config: &ResolvedConfig,
    renderer: Option<&PostRenderer<'_>>,
    job: &mut JobState,
    job_file: &camino::Utf8Path,
) -> anyhow::Result<Option<PostTimestamp>> {
    let mut newest_timestamp: Option<PostTimestamp> = None;
    let mut posts_archived: u64 = 0;
    let mut buffered: Option<BufferedHtmlPost> = None;

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

        for post in &post_response.posts {
            newest_timestamp = Some(match newest_timestamp {
                Some(current) if post.timestamp > current => post.timestamp,
                Some(current) => current,
                None => post.timestamp,
            });

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
                fs_err::write(&output_file, &content)?;
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
            writeln!(std::io::stdout(), "  {} posts archived", posts_archived)?;
        }
    }

    // Finalize the last buffered HTML post (no older neighbor)
    if let Some(last) = buffered.take() {
        finalize_buffered_post(&last, "<span></span>")?;
    }

    Ok(newest_timestamp)
}
