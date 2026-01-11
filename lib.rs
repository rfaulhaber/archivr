use camino::Utf8PathBuf;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "archivr", version, about = "A Tumblr backup tool", long_about = None)]
pub struct Args {
    blog_name: String,

    #[arg(long, help = "Tumblr OAuth consumer key")]
    consumer_key: Option<String>,

    #[arg(long, help = "Tumblr OAuth consumer secret")]
    consumer_secret: Option<String>,

    #[arg(long, help = "Job config file")]
    config_file: Option<Utf8PathBuf>,
}
