use camino::Utf8PathBuf;
use serde::Deserialize;

use crate::{ArchivrError, Args, JobState, PostTimestamp};

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
    pub json: bool,
    pub resume: bool,
    pub incremental: bool,
    pub quiet: bool,
    pub reauth: bool,
    pub before: Option<PostTimestamp>,
    pub after: Option<PostTimestamp>,
    pub cookies_file: Option<Utf8PathBuf>,
    pub dashboard: bool,
    pub headless: bool,
}

impl ResolvedConfig {
    pub fn from_args(args: Args) -> anyhow::Result<Self> {
        let config: Option<Config> = if let Some(ref config_path) = args.config_file {
            let config_file_str = fs_err::read_to_string(config_path)?;
            Some(serde_json::from_str(&config_file_str)?)
        } else {
            None
        };

        let consumer_key = args
            .consumer_key
            .or_else(|| config.as_ref().and_then(|c| c.consumer_key.clone()))
            .ok_or(ArchivrError::NoConsumerKey)?;

        let consumer_secret = args
            .consumer_secret
            .or_else(|| config.as_ref().and_then(|c| c.consumer_secret.clone()))
            .ok_or(ArchivrError::NoConsumerSecret)?;

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
            json: args.json,
            resume: args.resume,
            incremental: args.incremental,
            quiet: args.quiet,
            reauth: args.reauth,
            before: args.before,
            after: args.after,
            cookies_file: args.cookies_file,
            dashboard: args.dashboard,
            headless: args.headless,
        })
    }

    pub fn apply_job_state(&mut self, job: &JobState) {
        self.before = job.before;
        self.after = job.after;
        self.json = job.json;
        self.directories = job.directories;
        self.save_images = job.save_images;
        self.template_path = job.template_path.clone();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Args;

    fn base_args() -> Args {
        Args {
            blog_name: "testblog".to_owned(),
            consumer_key: Some("key".to_owned()),
            consumer_secret: Some("secret".to_owned()),
            config_file: None,
            resume: false,
            incremental: false,
            template: None,
            directories: false,
            save_images: false,
            json: false,
            output_dir: Some(Utf8PathBuf::from("/tmp/test-output")),
            before: None,
            after: None,
            quiet: false,
            reauth: false,
            cookies_file: None,
            dashboard: false,
            headless: false,
        }
    }

    #[test]
    fn from_args_basic() {
        let config = ResolvedConfig::from_args(base_args()).unwrap();
        assert_eq!(config.blog_name, "testblog");
        assert_eq!(config.consumer_key, "key");
        assert_eq!(config.consumer_secret, "secret");
        assert_eq!(config.output_dir, Utf8PathBuf::from("/tmp/test-output"));
    }

    #[test]
    fn from_args_missing_consumer_key() {
        let mut args = base_args();
        args.consumer_key = None;
        let result = ResolvedConfig::from_args(args);
        assert!(result.is_err());
        assert!(format!("{}", result.err().unwrap()).contains("Consumer key"));
    }

    #[test]
    fn from_args_missing_consumer_secret() {
        let mut args = base_args();
        args.consumer_secret = None;
        let result = ResolvedConfig::from_args(args);
        assert!(result.is_err());
        assert!(format!("{}", result.err().unwrap()).contains("Consumer secret"));
    }

    #[test]
    fn output_dir_defaults_to_blog_name() {
        let mut args = base_args();
        args.output_dir = None;
        let config = ResolvedConfig::from_args(args).unwrap();
        // Should end with the blog name
        assert!(config.output_dir.as_str().ends_with("testblog"));
    }
}
