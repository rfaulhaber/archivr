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

    #[arg(short, long, help = "Jinja template for formatting Tumblr posts")]
    pub template: Option<Utf8PathBuf>,

    #[arg(short, long, help = "Use directories for each post")]
    pub directories: bool,

    #[arg(long, help = "Set to download videos from Tumblr")]
    pub save_video: bool,

    #[arg(long, help = "Set to download audio from Tumblr")]
    pub save_audio: bool,

    #[arg(long, help = "Set post notes")]
    pub save_notes: bool,

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

    #[arg(long, value_delimiter = ',', help = "List of tags to filter for")]
    pub tags: Option<Vec<String>>,
}
