use archivr::Args;
use camino::Utf8Path;
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // check if we are already authenticated
    // check if consumer key and secret are specified
    // check config file for extra settings
    // if not authenticated, go through authentication flow
    // if authenticated, proceed with backup

    let project_dir = directories::ProjectDirs::from("com.ryanfaulhaber", "", "archivr")
        .ok_or_else(|| anyhow::anyhow!("Could not determine project directory"))?;

    let data_dir = project_dir.data_local_dir();

    let data_dir_exists = fs_err::exists(data_dir)?;

    if !data_dir_exists {
        std::fs::create_dir_all(data_dir)?;
    }

    let auth_file_exists = fs_err::exists(data_dir.join("auth.json"))?;

    if !auth_file_exists {}

    Ok(())
}
