use camino::Utf8PathBuf;
use clap::Parser;

fn non_empty_string(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("blog name cannot be empty".to_owned())
    } else {
        Ok(s.to_owned())
    }
}

#[derive(Debug, Parser)]
#[command(name = "archivr", bin_name = "archivr", version, about = "A Tumblr backup tool", long_about = None)]
pub struct Args {
    #[arg(help = "Name of the Tumblr blog to back up", value_parser = non_empty_string)]
    pub blog_name: String,

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
        short,
        long,
        help = "Path to output posts to, defaulting to ./{blog-name}"
    )]
    pub output_dir: Option<Utf8PathBuf>,

    #[arg(
        long,
        help = "Retrieve all posts after this date. Date can either be specified as a Unix timestamp prefixed with '@' or as an RFC3339-formatted date/datetime",
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

fn parse_date(s: &str) -> Result<i64, String> {
    if let Some(rest) = s.strip_prefix('@') {
        return rest
            .parse::<i64>()
            .map_err(|e| format!("invalid unix timestamp `{s}`: {e}"));
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }

    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%d") {
        return Ok(dt.timestamp());
    }

    Err(format!(
        "invalid date `{s}` (expected RFC 3339 like \
         `2024-01-02T03:04:05Z`, a date `2024-01-02`, or `@<unix-seconds>`)"
    ))
}

#[cfg(test)]
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
        let expected = Ok(1699920000 as i64);
        assert_eq!(parse_date(input), expected);
    }

    #[test]
    fn timestamp_parsing_invalid() {
        let input = "not-a-date";
        let result = parse_date(input);
        assert!(result.is_err());
        assert!(format!("{}", result.err().unwrap()).contains("invalid date"));
    }
}
