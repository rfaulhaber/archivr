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
}
