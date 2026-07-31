use camino::Utf8PathBuf;
use clap::Parser;

fn non_empty_string(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("blog name cannot be empty".to_owned())
    } else {
        Ok(s.to_owned())
    }
}

/// Normalizes one `--include-tags` entry: trims whitespace and a leading `#`
/// (people naturally type tags the way Tumblr displays them).
fn parse_tag(s: &str) -> Result<String, String> {
    let tag = s.trim().trim_start_matches('#').trim();
    if tag.is_empty() {
        Err("tag cannot be empty".to_owned())
    } else {
        Ok(tag.to_owned())
    }
}

/// Legacy post types accepted by the API's server-side `type` filter.
const POST_TYPES: [&str; 8] = [
    "text", "photo", "quote", "link", "chat", "audio", "video", "answer",
];

#[derive(Parser)]
#[command(name = "archivr", bin_name = "archivr", version, about = "A Tumblr backup tool", long_about = None)]
pub struct Args {
    #[arg(
        help = "Names of the Tumblr blogs to back up (each goes to its own output directory)",
        value_parser = non_empty_string,
        num_args = 1..,
        required = true
    )]
    pub blog_names: Vec<String>,

    #[arg(long, help = "Tumblr OAuth consumer key")]
    pub consumer_key: Option<String>,

    #[arg(long, help = "Tumblr OAuth consumer secret")]
    pub consumer_secret: Option<String>,

    #[arg(long, help = "Job config file")]
    pub config_file: Option<Utf8PathBuf>,

    #[arg(
        long,
        help = "Resume last job if one exists, otherwise start a new backup"
    )]
    pub resume: bool,

    #[arg(
        long,
        conflicts_with_all = ["resume", "after"],
        help = "Fetch only posts newer than the last successful backup. Falls back to a full backup if no prior state exists"
    )]
    pub incremental: bool,

    #[arg(
        short,
        long,
        conflicts_with = "json",
        help = "Jinja template for formatting Tumblr posts. Exclusive to the --json flag"
    )]
    pub template: Option<Utf8PathBuf>,

    #[arg(short, long, help = "Use directories for each post")]
    pub directories: bool,

    #[arg(long, help = "Set to download post images rather than link to them")]
    pub save_images: bool,

    #[arg(long, help = "Save posts as JSON")]
    pub json: bool,

    #[arg(
        long,
        help = "Fetch and archive each post's notes (likes, reblogs, replies)"
    )]
    pub save_notes: bool,

    #[arg(
        long,
        requires = "save_notes",
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Maximum number of notes to fetch per post, newest first (default: all)"
    )]
    pub notes_limit: Option<u64>,

    #[arg(
        long,
        help = "Also back up the blog's draft posts (the blog must belong to the authenticated user)"
    )]
    pub drafts: bool,

    #[arg(
        long,
        help = "Also back up the blog's queued posts (the blog must belong to the authenticated user)"
    )]
    pub queue: bool,

    #[arg(
        long,
        help = "Also back up the blog's post submissions (the blog must belong to the authenticated user)"
    )]
    pub submissions: bool,

    #[arg(
        long,
        help = "Also back up the blog's liked posts (the blog's likes must be public, unless it belongs to the authenticated user)"
    )]
    pub likes: bool,

    #[arg(
        long,
        value_name = "TAGS",
        value_delimiter = ',',
        value_parser = parse_tag,
        help = "Only back up posts carrying at least one of these tags (comma-separated)"
    )]
    pub include_tags: Vec<String>,

    #[arg(
        long = "type",
        value_name = "TYPE",
        value_parser = POST_TYPES,
        help = "Only back up posts of this type"
    )]
    pub post_type: Option<String>,

    #[arg(
        long,
        conflicts_with = "only_reblogs",
        help = "Skip reblogs, backing up only the blog's original posts"
    )]
    pub no_reblogs: bool,

    #[arg(long, help = "Back up only reblogs, skipping the blog's original posts")]
    pub only_reblogs: bool,

    #[arg(
        long,
        help = "Set to download native Tumblr video files rather than link to them"
    )]
    pub save_video: bool,

    #[arg(
        long,
        help = "Set to download native Tumblr audio files rather than link to them"
    )]
    pub save_audio: bool,

    #[arg(
        long,
        help = "Download externally hosted video embeds (YouTube, Vimeo, ...) with yt-dlp, if it is installed"
    )]
    pub save_video_external: bool,

    #[arg(
        long,
        help = "Generate an index.html in the output directory linking all archived posts, grouped by month"
    )]
    pub index_file: bool,

    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with = "incremental",
        help = "Back up at most N posts (newest first)"
    )]
    pub count: Option<u64>,

    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with = "incremental",
        help = "Skip the N newest posts before backing up"
    )]
    pub skip: Option<u64>,

    #[arg(
        long,
        help = "Also download the blog's avatar image into the output directory"
    )]
    pub save_avatar: bool,

    #[arg(
        long,
        help = "Write media-list.jsonl in the output directory: one JSON line per archived post mapping its id to its media URLs"
    )]
    pub media_list: bool,

    #[arg(
        long,
        value_name = "IDS",
        value_delimiter = ',',
        value_parser = non_empty_string,
        conflicts_with_all = ["resume", "incremental", "before", "after", "count", "skip"],
        help = "Back up only these posts (comma-separated ids) instead of paginating the blog"
    )]
    pub post_id: Vec<String>,

    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["resume", "incremental", "before", "after", "count", "skip"],
        help = "Back up only the post ids listed in this file (one per line) instead of paginating the blog"
    )]
    pub id_file: Option<Utf8PathBuf>,

    #[arg(
        long,
        help = "When a media URL returns 403/404, retry it through the Internet Archive's Wayback Machine"
    )]
    pub wayback: bool,

    #[arg(
        long,
        help = "Do not pass provider embed HTML (YouTube, Vimeo, ...) into archived pages; render a plain link instead"
    )]
    pub no_embeds: bool,

    #[arg(
        short,
        long,
        help = "Path to output posts to, defaulting to ./{blog-name}"
    )]
    pub output_dir: Option<Utf8PathBuf>,

    #[arg(
        long,
        help = "Retrieve all posts before this date. Date can either be specified as a Unix timestamp prefixed with '@' or as an RFC3339-formatted date/datetime",
        value_name = "DATE",
        value_parser = parse_date
    )]
    pub before: Option<i64>,

    #[arg(
        long,
        help = "Retrieve all posts after this date. Date can either be specified as a Unix timestamp prefixed with '@' or as an RFC3339-formatted date/datetime",
        value_name = "DATE",
        value_parser = parse_date
    )]
    pub after: Option<i64>,

    #[arg(short, long, help = "Suppress progress output")]
    pub quiet: bool,

    #[arg(long, help = "Force re-authentication, ignoring any saved tokens")]
    pub reauth: bool,

    #[arg(
        long,
        help = "Path to a Netscape/Mozilla-format cookies file (e.g. exported by a browser extension). Cookies will be sent with API requests, enabling access to dashboard-only blogs"
    )]
    pub cookies_file: Option<Utf8PathBuf>,

    #[arg(
        long,
        requires = "cookies_file",
        help = "Use Tumblr's internal dashboard API instead of the public API. Requires --cookies-file. Enables access to dashboard-only blogs"
    )]
    pub dashboard: bool,

    #[arg(
        long,
        help = "Use manual authentication flow for environments without a browser (e.g. servers, containers). You will be prompted to paste the redirect URL after authenticating"
    )]
    pub headless: bool,
}

/// Manual impl so `RUST_LOG=debug` output (the first thing users paste into
/// bug reports) never contains the OAuth credentials.
impl std::fmt::Debug for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn redact(secret: &Option<String>) -> &'static str {
            match secret {
                Some(_) => "Some(<redacted>)",
                None => "None",
            }
        }

        f.debug_struct("Args")
            .field("blog_names", &self.blog_names)
            .field("consumer_key", &redact(&self.consumer_key))
            .field("consumer_secret", &redact(&self.consumer_secret))
            .field("config_file", &self.config_file)
            .field("resume", &self.resume)
            .field("incremental", &self.incremental)
            .field("template", &self.template)
            .field("directories", &self.directories)
            .field("save_images", &self.save_images)
            .field("json", &self.json)
            .field("save_notes", &self.save_notes)
            .field("notes_limit", &self.notes_limit)
            .field("drafts", &self.drafts)
            .field("queue", &self.queue)
            .field("submissions", &self.submissions)
            .field("likes", &self.likes)
            .field("include_tags", &self.include_tags)
            .field("post_type", &self.post_type)
            .field("no_reblogs", &self.no_reblogs)
            .field("only_reblogs", &self.only_reblogs)
            .field("save_video", &self.save_video)
            .field("save_audio", &self.save_audio)
            .field("save_video_external", &self.save_video_external)
            .field("index_file", &self.index_file)
            .field("count", &self.count)
            .field("skip", &self.skip)
            .field("save_avatar", &self.save_avatar)
            .field("media_list", &self.media_list)
            .field("post_id", &self.post_id)
            .field("id_file", &self.id_file)
            .field("wayback", &self.wayback)
            .field("no_embeds", &self.no_embeds)
            .field("output_dir", &self.output_dir)
            .field("before", &self.before)
            .field("after", &self.after)
            .field("quiet", &self.quiet)
            .field("reauth", &self.reauth)
            .field("cookies_file", &self.cookies_file)
            .field("dashboard", &self.dashboard)
            .field("headless", &self.headless)
            .finish()
    }
}

fn parse_date(s: &str) -> Result<i64, String> {
    if let Some(rest) = s.strip_prefix('@') {
        return rest
            .parse::<i64>()
            .map_err(|e| format!("invalid unix timestamp `{s}`: {e}"));
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }

    // A bare `YYYY-MM-DD` has no time or offset, so parse it as a naive date and
    // anchor it to midnight UTC. (`DateTime::parse_from_str` would reject it.)
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        && let Some(dt) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(dt.and_utc().timestamp());
    }

    Err(format!(
        "invalid date `{s}` (expected RFC 3339 like \
         `2024-01-02T03:04:05Z`, a date `2024-01-02`, or `@<unix-seconds>`)"
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_parsing_unix() {
        let input = "@1700000000";
        let expected = Ok(1700000000);
        assert_eq!(parse_date(input), expected);
    }

    #[test]
    fn timestamp_parsing_rfc3339() {
        let input = "2023-11-14T00:00:00Z";
        let expected = Ok(1699920000_i64);
        assert_eq!(parse_date(input), expected);
    }

    #[test]
    fn timestamp_parsing_date_only() {
        // 2024-01-02T00:00:00Z
        assert_eq!(parse_date("2024-01-02"), Ok(1_704_153_600));
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let args = Args::parse_from([
            "archivr",
            "myblog",
            "--consumer-key",
            "key-sensitive-value",
            "--consumer-secret",
            "secret-sensitive-value",
        ]);
        let debug = format!("{args:?}");
        assert!(!debug.contains("key-sensitive-value"));
        assert!(!debug.contains("secret-sensitive-value"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("myblog"));
    }

    #[test]
    fn notes_limit_requires_save_notes() {
        let result = Args::try_parse_from(["archivr", "myblog", "--notes-limit", "50"]);
        assert!(result.is_err());
    }

    #[test]
    fn notes_limit_rejects_zero() {
        let result =
            Args::try_parse_from(["archivr", "myblog", "--save-notes", "--notes-limit", "0"]);
        assert!(result.is_err());
    }

    #[test]
    fn notes_limit_parses_with_save_notes() {
        let args = Args::parse_from(["archivr", "myblog", "--save-notes", "--notes-limit", "50"]);
        assert!(args.save_notes);
        assert_eq!(args.notes_limit, Some(50));
    }

    #[test]
    fn special_backup_flags_parse() {
        let args = Args::parse_from(["archivr", "myblog", "--drafts", "--queue", "--submissions"]);
        assert!(args.drafts);
        assert!(args.queue);
        assert!(args.submissions);
    }

    #[test]
    fn include_tags_split_and_normalized() {
        let args = Args::parse_from(["archivr", "myblog", "--include-tags", "#art, gif art ,b"]);
        assert_eq!(args.include_tags, vec!["art", "gif art", "b"]);
    }

    #[test]
    fn include_tags_rejects_empty_entry() {
        let result = Args::try_parse_from(["archivr", "myblog", "--include-tags", "a,,b"]);
        assert!(result.is_err());
    }

    #[test]
    fn post_type_rejects_unknown_type() {
        assert!(Args::try_parse_from(["archivr", "myblog", "--type", "photoset"]).is_err());
        let args = Args::parse_from(["archivr", "myblog", "--type", "video"]);
        assert_eq!(args.post_type.as_deref(), Some("video"));
    }

    #[test]
    fn reblog_filters_conflict() {
        assert!(Args::try_parse_from(["archivr", "myblog", "--no-reblogs", "--only-reblogs"]).is_err());
    }

    #[test]
    fn phase4_flags_parse() {
        let args = Args::parse_from([
            "archivr",
            "myblog",
            "--likes",
            "--save-video",
            "--save-audio",
            "--save-video-external",
            "--index-file",
        ]);
        assert!(args.likes);
        assert!(args.save_video);
        assert!(args.save_audio);
        assert!(args.save_video_external);
        assert!(args.index_file);
    }

    #[test]
    fn multiple_blog_names_parse() {
        let args = Args::parse_from(["archivr", "blog-a", "blog-b", "blog-c"]);
        assert_eq!(args.blog_names, vec!["blog-a", "blog-b", "blog-c"]);
    }

    #[test]
    fn blog_name_is_required() {
        assert!(Args::try_parse_from(["archivr"]).is_err());
    }

    #[test]
    fn count_and_skip_conflict_with_incremental() {
        assert!(Args::try_parse_from(["archivr", "b", "--count", "5", "--incremental"]).is_err());
        assert!(Args::try_parse_from(["archivr", "b", "--skip", "5", "--incremental"]).is_err());
        let args = Args::parse_from(["archivr", "b", "--count", "5", "--skip", "10"]);
        assert_eq!(args.count, Some(5));
        assert_eq!(args.skip, Some(10));
    }

    #[test]
    fn count_rejects_zero() {
        assert!(Args::try_parse_from(["archivr", "b", "--count", "0"]).is_err());
    }

    #[test]
    fn post_id_splits_and_conflicts_with_pagination_flags() {
        let args = Args::parse_from(["archivr", "b", "--post-id", "123,456"]);
        assert_eq!(args.post_id, vec!["123", "456"]);
        assert!(Args::try_parse_from(["archivr", "b", "--post-id", "1", "--resume"]).is_err());
        assert!(Args::try_parse_from(["archivr", "b", "--id-file", "ids.txt", "--incremental"]).is_err());
        assert!(Args::try_parse_from(["archivr", "b", "--post-id", "1", "--count", "5"]).is_err());
    }

    #[test]
    fn phase5_flags_parse() {
        let args = Args::parse_from([
            "archivr",
            "myblog",
            "--save-avatar",
            "--media-list",
            "--wayback",
            "--no-embeds",
        ]);
        assert!(args.save_avatar);
        assert!(args.media_list);
        assert!(args.wayback);
        assert!(args.no_embeds);
    }

    #[test]
    fn timestamp_parsing_invalid() {
        let input = "not-a-date";
        let result = parse_date(input);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("invalid date"));
    }
}
