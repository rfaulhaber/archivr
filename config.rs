use camino::Utf8PathBuf;
use serde::Deserialize;

use crate::{ArchivrError, Args, PostTimestamp};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub blog_name: String,
    pub consumer_key: Option<String>,
    pub consumer_secret: Option<String>,
}

pub struct ResolvedConfig {
    pub blog_name: String,
    pub consumer_key: String,
    pub consumer_secret: String,
    pub output_dir: Utf8PathBuf,
    pub template_path: Option<Utf8PathBuf>,
    pub directories: bool,
    pub save_images: bool,
    pub save_video: bool,
    pub save_audio: bool,
    pub save_notes: bool,
    pub json: bool,
    pub likes: bool,
    pub tags: Option<Vec<String>>,
    pub resume: bool,
    pub quiet: bool,
    pub reauth: bool,
    pub before: Option<PostTimestamp>,
    pub after: Option<PostTimestamp>,
    pub cookies_file: Option<Utf8PathBuf>,
    pub dashboard: bool,
}

impl ResolvedConfig {
    pub fn from_args(args: Args) -> anyhow::Result<Self> {
        fn get_timestamp(timestamp_str: Option<String>) -> Option<i64> {
            match timestamp_str {
                Some(timestamp) => {
                    let from_datetime = chrono::DateTime::parse_from_rfc3339(&timestamp).ok();

                    let from_timestamp = timestamp
                        .parse::<i64>()
                        .ok()
                        .and_then(chrono::DateTime::from_timestamp_secs);

                    from_datetime
                        .map(|val| val.timestamp())
                        .or_else(|| from_timestamp.map(|val| val.timestamp()))
                }
                None => None,
            }
        }

        let config: Option<Config> = if let Some(ref config_path) = args.config_file {
            let config_file_str = fs_err::read_to_string(config_path)?;
            Some(serde_json::from_str(&config_file_str)?)
        } else {
            None
        };

        let consumer_key = args
            .consumer_key
            .or_else(|| config.as_ref().and_then(|c| c.consumer_key.clone()))
            .ok_or(ArchivrError::NoConsumerKeyAndSecret)?;

        let consumer_secret = args
            .consumer_secret
            .or_else(|| config.as_ref().and_then(|c| c.consumer_secret.clone()))
            .ok_or(ArchivrError::NoConsumerKeyAndSecret)?;

        let output_dir = match args.output_dir {
            Some(dir) => dir,
            None => {
                let cwd = Utf8PathBuf::try_from(std::env::current_dir()?)?;
                cwd.join(&args.blog_name)
            }
        };

        Ok(Self {
            blog_name: args.blog_name,
            consumer_key,
            consumer_secret,
            output_dir,
            template_path: args.template,
            directories: args.directories,
            save_images: args.save_images,
            save_video: args.save_video,
            save_audio: args.save_audio,
            save_notes: args.save_notes,
            json: args.json,
            likes: args.likes,
            tags: args.include_tags,
            resume: args.resume,
            quiet: args.quiet,
            reauth: args.reauth,
            before: get_timestamp(args.before),
            after: get_timestamp(args.after),
            cookies_file: args.cookies_file,
            dashboard: args.dashboard,
        })
    }
}
