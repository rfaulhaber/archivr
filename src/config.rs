use camino::Utf8PathBuf;
use serde::Deserialize;

use crate::{ArchivrError, Args, JobState, PostTimestamp};

#[derive(Deserialize)]
pub struct Config {
    pub blog_name: String,
    pub consumer_key: Option<String>,
    pub consumer_secret: Option<String>,
}

/// Manual impl so debug-logging a `Config` can never leak the OAuth
/// credentials read from the config file.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("blog_name", &self.blog_name)
            .field(
                "consumer_key",
                &self.consumer_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "consumer_secret",
                &self.consumer_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct ResolvedConfig {
    pub blog_name: String,
    pub consumer_key: String,
    pub consumer_secret: String,
    pub output_dir: Utf8PathBuf,
    pub template_path: Option<Utf8PathBuf>,
    pub directories: bool,
    pub save_images: bool,
    pub json: bool,
    pub save_notes: bool,
    pub notes_limit: Option<u64>,
    pub drafts: bool,
    pub queue: bool,
    pub submissions: bool,
    pub likes: bool,
    pub include_tags: Vec<String>,
    pub post_type: Option<String>,
    pub no_reblogs: bool,
    pub only_reblogs: bool,
    pub save_video: bool,
    pub save_audio: bool,
    pub save_video_external: bool,
    pub index_file: bool,
    pub count: Option<u64>,
    pub skip: Option<u64>,
    pub save_avatar: bool,
    pub media_list: bool,
    /// Explicit post ids to fetch instead of paginating (`--post-id` plus the
    /// contents of `--id-file`). Non-empty selects single-post mode.
    pub post_ids: Vec<String>,
    pub wayback: bool,
    pub no_embeds: bool,
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
    /// Resolves the CLI arguments into one config per requested blog. With an
    /// explicit `--output-dir` and multiple blogs, each blog gets its own
    /// subdirectory of it — the job/state files inside an output directory
    /// describe exactly one blog.
    pub fn from_args(args: Args) -> anyhow::Result<Vec<Self>> {
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

        let mut post_ids = args.post_id.clone();
        if let Some(ref id_file) = args.id_file {
            post_ids.extend(read_id_file(id_file)?);
        }

        let multiple = args.blog_names.len() > 1;
        let cwd = Utf8PathBuf::try_from(std::env::current_dir()?)?;

        args.blog_names
            .iter()
            .map(|blog_name| {
                let output_dir = match (&args.output_dir, multiple) {
                    (Some(dir), false) => dir.clone(),
                    (Some(dir), true) => dir.join(blog_name),
                    (None, _) => cwd.join(blog_name),
                };
                Ok(Self {
                    blog_name: blog_name.clone(),
                    consumer_key: consumer_key.clone(),
                    consumer_secret: consumer_secret.clone(),
                    output_dir,
                    template_path: args.template.clone(),
                    directories: args.directories,
                    save_images: args.save_images,
                    json: args.json,
                    save_notes: args.save_notes,
                    notes_limit: args.notes_limit,
                    drafts: args.drafts,
                    queue: args.queue,
                    submissions: args.submissions,
                    likes: args.likes,
                    include_tags: args.include_tags.clone(),
                    post_type: args.post_type.clone(),
                    no_reblogs: args.no_reblogs,
                    only_reblogs: args.only_reblogs,
                    save_video: args.save_video,
                    save_audio: args.save_audio,
                    save_video_external: args.save_video_external,
                    index_file: args.index_file,
                    count: args.count,
                    skip: args.skip,
                    save_avatar: args.save_avatar,
                    media_list: args.media_list,
                    post_ids: post_ids.clone(),
                    wayback: args.wayback,
                    no_embeds: args.no_embeds,
                    resume: args.resume,
                    incremental: args.incremental,
                    quiet: args.quiet,
                    reauth: args.reauth,
                    before: args.before,
                    after: args.after,
                    cookies_file: args.cookies_file.clone(),
                    dashboard: args.dashboard,
                    headless: args.headless,
                })
            })
            .collect()
    }

    pub fn apply_job_state(&mut self, job: &JobState) {
        self.before = job.before;
        self.after = job.after;
        self.json = job.json;
        self.directories = job.directories;
        self.save_images = job.save_images;
        self.save_notes = job.save_notes;
        self.notes_limit = job.notes_limit;
        self.drafts = job.drafts;
        self.queue = job.queue;
        self.submissions = job.submissions;
        self.likes = job.likes;
        self.include_tags = job.include_tags.clone();
        self.post_type = job.post_type.clone();
        self.no_reblogs = job.no_reblogs;
        self.only_reblogs = job.only_reblogs;
        self.save_video = job.save_video;
        self.save_audio = job.save_audio;
        self.save_video_external = job.save_video_external;
        self.index_file = job.index_file;
        self.count = job.count;
        self.skip = job.skip;
        self.save_avatar = job.save_avatar;
        self.media_list = job.media_list;
        self.wayback = job.wayback;
        self.no_embeds = job.no_embeds;
        self.template_path = job.template_path.clone();
    }

    /// Canonical description of this run's post-selection filters, or `None`
    /// for an unfiltered run. Stored in `.archivr-state.json`: the incremental
    /// high-water mark asserts "everything below this is archived", which only
    /// holds between runs selecting the same set of posts.
    pub fn filter_descriptor(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if !self.include_tags.is_empty() {
            let mut tags: Vec<String> =
                self.include_tags.iter().map(|t| t.to_lowercase()).collect();
            tags.sort();
            tags.dedup();
            parts.push(format!("tags={}", tags.join(",")));
        }
        if let Some(ref t) = self.post_type {
            parts.push(format!("type={t}"));
        }
        if self.no_reblogs {
            parts.push("no-reblogs".to_owned());
        }
        if self.only_reblogs {
            parts.push("only-reblogs".to_owned());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(";"))
        }
    }
}

/// Reads post ids from an `--id-file`: one id per line, blank lines and
/// `#`-comment lines ignored.
fn read_id_file(path: &camino::Utf8Path) -> anyhow::Result<Vec<String>> {
    let content = fs_err::read_to_string(path)?;
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Args;

    fn base_args() -> Args {
        Args {
            blog_names: vec!["testblog".to_owned()],
            consumer_key: Some("key".to_owned()),
            consumer_secret: Some("secret".to_owned()),
            config_file: None,
            resume: false,
            incremental: false,
            template: None,
            directories: false,
            save_images: false,
            json: false,
            save_notes: false,
            notes_limit: None,
            drafts: false,
            queue: false,
            submissions: false,
            likes: false,
            include_tags: Vec::new(),
            post_type: None,
            no_reblogs: false,
            only_reblogs: false,
            save_video: false,
            save_audio: false,
            save_video_external: false,
            index_file: false,
            count: None,
            skip: None,
            save_avatar: false,
            media_list: false,
            post_id: Vec::new(),
            id_file: None,
            wayback: false,
            no_embeds: false,
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

    fn resolve_one(args: Args) -> ResolvedConfig {
        let mut configs = ResolvedConfig::from_args(args).unwrap();
        assert_eq!(configs.len(), 1);
        configs.remove(0)
    }

    #[test]
    fn from_args_basic() {
        let config = resolve_one(base_args());
        assert_eq!(config.blog_name, "testblog");
        assert_eq!(config.consumer_key, "key");
        assert_eq!(config.consumer_secret, "secret");
        assert_eq!(config.output_dir, Utf8PathBuf::from("/tmp/test-output"));
    }

    #[test]
    fn from_args_multiple_blogs_split_output_dirs() {
        let mut args = base_args();
        args.blog_names = vec!["one".to_owned(), "two".to_owned()];
        let configs = ResolvedConfig::from_args(args).unwrap();
        assert_eq!(configs.len(), 2);
        // With an explicit --output-dir, each blog gets its own subdirectory.
        assert_eq!(configs[0].output_dir, Utf8PathBuf::from("/tmp/test-output/one"));
        assert_eq!(configs[1].output_dir, Utf8PathBuf::from("/tmp/test-output/two"));
        assert_eq!(configs[0].blog_name, "one");
        assert_eq!(configs[1].blog_name, "two");
    }

    #[test]
    fn from_args_reads_id_file() {
        let dir = std::env::temp_dir();
        let dir = camino::Utf8Path::from_path(&dir).unwrap();
        let path = dir.join(format!("archivr-test-ids-{}.txt", std::process::id()));
        fs_err::write(&path, "123\n\n# comment\n 456 \n").unwrap();

        let mut args = base_args();
        args.post_id = vec!["789".to_owned()];
        args.id_file = Some(path.clone());
        let config = resolve_one(args);
        assert_eq!(config.post_ids, vec!["789", "123", "456"]);

        fs_err::remove_file(&path).unwrap();
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
    fn filter_descriptor_none_when_unfiltered() {
        let config = resolve_one(base_args());
        assert_eq!(config.filter_descriptor(), None);
    }

    #[test]
    fn filter_descriptor_canonicalizes_tag_order_and_case() {
        let mut args = base_args();
        args.include_tags = vec!["Zebra".to_owned(), "art".to_owned()];
        args.post_type = Some("video".to_owned());
        args.no_reblogs = true;
        let config = resolve_one(args);
        assert_eq!(
            config.filter_descriptor().as_deref(),
            Some("tags=art,zebra;type=video;no-reblogs")
        );

        // Same filters in a different spelling produce the same descriptor.
        let mut args2 = base_args();
        args2.include_tags = vec!["ART".to_owned(), "zebra".to_owned()];
        args2.post_type = Some("video".to_owned());
        args2.no_reblogs = true;
        let config2 = resolve_one(args2);
        assert_eq!(config.filter_descriptor(), config2.filter_descriptor());
    }

    #[test]
    fn output_dir_defaults_to_blog_name() {
        let mut args = base_args();
        args.output_dir = None;
        let config = resolve_one(args);
        // Should end with the blog name
        assert!(config.output_dir.as_str().ends_with("testblog"));
    }
}
