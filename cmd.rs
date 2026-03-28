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
        help = "Retrieve all posts before this date. Date can either be specified as a Unix timestamp or as an RFC3339-formatted date"
    )]
    pub before: Option<String>, // TODO make date

    #[arg(
        long,
        help = "Retrieve all posts after this date. Date can either be specified as a Unix timestamp or as an RFC3339-formatted date"
    )]
    pub after: Option<String>, // TODO make date

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
