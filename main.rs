use std::io::Write;

use archivr::{Args, JobState, LastRun, PostRenderer, ResolvedConfig, auth::authenticate};
use clap::Parser;
use crabrave::Crabrave;

const PROJECT_QUALIFIER: &str = "com.ryanfaulhaber";
const PROJECT_NAME: &str = "archivr";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();
    log::debug!("args: {:?}", args);

    let config = ResolvedConfig::from_args(args)?;

    let project_dir = directories::ProjectDirs::from(PROJECT_QUALIFIER, "", PROJECT_NAME)
        .ok_or_else(|| anyhow::anyhow!("Could not determine project directory"))?;
    let data_dir = camino::Utf8Path::from_path(project_dir.data_local_dir())
        .ok_or_else(|| anyhow::anyhow!("Data directory path is not valid UTF-8"))?;

    let client = authenticate(
        &config.consumer_key,
        &config.consumer_secret,
        data_dir,
        config.reauth,
    )
    .await?;

    if !fs_err::exists(&config.output_dir)? {
        fs_err::create_dir_all(&config.output_dir)?;
    }

    let renderer = if config.json {
        None
    } else if let Some(ref template_path) = config.template_path {
        Some(PostRenderer::from_file(template_path)?)
    } else {
        Some(PostRenderer::new())
    };

    let job_file = JobState::job_file_path(&config.output_dir);
    let mut job = if config.resume {
        JobState::load(&job_file)?
    } else {
        JobState::new(&config.blog_name)
    };

    let marker_file = LastRun::marker_path(&config.output_dir);
    let incremental_cutoff = if config.incremental {
        match LastRun::load(&marker_file) {
            Ok(last_run) => Some(last_run.newest_post_timestamp),
            Err(_e) => {
                log::info!("no previous run marker found, performing full backup");
                None
            }
        }
    } else {
        None
    };

    if !config.quiet {
        if config.incremental {
            writeln!(
                std::io::stdout(),
                "Backing up {} (incremental)...",
                config.blog_name
            )?;
        } else {
            writeln!(std::io::stdout(), "Backing up {}...", config.blog_name)?;
        }
    }

    let newest_timestamp = run_backup(
        &client,
        &config,
        renderer.as_ref(),
        &mut job,
        &job_file,
        incremental_cutoff,
    )
    .await?;

    if fs_err::exists(&job_file)? {
        JobState::delete(&job_file)?;
    }

    if let Some(ts) = newest_timestamp {
        let last_run = LastRun::new(&config.blog_name, ts);
        last_run.save(&marker_file)?;
    }

    if !config.quiet {
        writeln!(std::io::stdout(), "Backup complete.")?;
    }

    Ok(())
}

async fn run_backup(
    client: &Crabrave,
    config: &ResolvedConfig,
    renderer: Option<&PostRenderer<'_>>,
    job: &mut JobState,
    job_file: &camino::Utf8Path,
    incremental_cutoff: Option<i64>,
) -> anyhow::Result<Option<i64>> {
    let mut newest_timestamp: Option<i64> = None;
    let mut posts_archived: u64 = 0;

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

        let post_response = post_builder.send().await?;

        if post_response.posts.is_empty() {
            log::info!("no more posts to fetch, ending backup");
            break;
        }

        log::info!("({}) Fetching next batch of posts...", job.offset,);

        let mut reached_cutoff = false;

        for post in &post_response.posts {
            if let Some(cutoff) = incremental_cutoff
                && post.timestamp <= cutoff
            {
                log::info!(
                    "reached incremental cutoff at post {} (timestamp {})",
                    post.id,
                    post.timestamp
                );
                reached_cutoff = true;
                break;
            }

            newest_timestamp = Some(match newest_timestamp {
                Some(current) if post.timestamp > current => post.timestamp,
                Some(current) => current,
                None => post.timestamp,
            });

            log::info!("processing post {}", post.id);

            let (content, ext) = if config.json {
                (serde_json::to_string_pretty(post)?, "json")
            } else {
                let r = renderer
                    .ok_or_else(|| anyhow::anyhow!("renderer is required for HTML mode"))?;
                (r.render(post)?, "html")
            };

            let output_file = if config.directories {
                let post_dir = config.output_dir.join(&post.id);
                if !fs_err::exists(&post_dir)? {
                    fs_err::create_dir(&post_dir)?;
                }
                post_dir.join(format!("index.{ext}"))
            } else {
                config.output_dir.join(format!("{}.{ext}", post.id))
            };

            fs_err::write(&output_file, &content)?;
            log::debug!("saved post {} to {}", post.id, output_file);
            posts_archived += 1;
        }

        job.offset += post_response.posts.len() as u64;
        job.save(job_file)?;

        if !config.quiet {
            writeln!(std::io::stdout(), "  {} posts archived", posts_archived)?;
        }

        if reached_cutoff {
            break;
        }
    }

    Ok(newest_timestamp)
}
