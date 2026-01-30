use camino::Utf8PathBuf;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "archivr", version, about = "A Tumblr backup tool", long_about = None)]
pub struct Args {
    pub blog_name: String,

    #[arg(long, help = "Tumblr OAuth consumer key")]
    pub consumer_key: Option<String>,

    #[arg(long, help = "Tumblr OAuth consumer secret")]
    pub consumer_secret: Option<String>,

    #[arg(long, help = "Job config file")]
    pub config_file: Option<Utf8PathBuf>,

    #[arg(
        long,
        help = "Resume last job, if applicable. Exits with error if no job in progress was found"
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

    #[arg(long, help = "Set to download videos from Tumblr")]
    pub save_video: bool,

    #[arg(long, help = "Set to download audio from Tumblr")]
    pub save_audio: bool,

    #[arg(long, help = "Set post notes")]
    pub save_notes: bool,

    #[arg(long, help = "Creats an index.html file as a landing page.")]
    pub index_file: bool,

    #[arg(long, help = "Save posts as JSON")]
    pub json: bool,

    #[arg(long, help = "Fetches liked posts instead of blog posts")]
    pub likes: bool,

    #[arg(
        short,
        long,
        help = "Path to output posts to, defaulting to ./{blog-name}"
    )]
    pub output_dir: Option<Utf8PathBuf>,

    #[arg(
        long,
        value_delimiter = ',',
        help = "If set, will only back up posts that include these tags. Must be comma-separated without spaces, e.g. foo,bar,baz"
    )]
    pub include_tags: Option<Vec<String>>,

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

    #[arg(
        long,
        help = "Only fetches all new posts since last run based on the destination directory. If a job has never been run, this flag functionally has no effect"
    )]
    pub incremental: bool,

    #[arg(long, help = "Force re-authentication, ignoring any saved tokens")]
    pub reauth: bool,
}
