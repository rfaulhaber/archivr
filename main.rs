use archivr::Args;
use clap::Parser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    Ok(())
}
