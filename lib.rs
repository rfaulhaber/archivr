use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "archivr", version, about = "A Tumblr backup tool", long_about = None)]
pub struct Args {
    blog_name: String,
}
