use archivr::{ArchivrError, Args, Config, PostRenderer, auth::Auth};
use camino::Utf8PathBuf;
use clap::Parser;
use crabrave::Crabrave;

const PROJECT_QUALIFIER: &'static str = "com.ryanfaulhaber";
const PROJECT_NAME: &'static str = "archivr";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();

    log::debug!("args: {:?}", args);

    // Load config file if specified
    let config: Option<Config> = if let Some(ref config_path) = args.config_file {
        let config_file_str = fs_err::read_to_string(config_path)?;
        Some(serde_json::from_str(&config_file_str)?)
    } else {
        None
    };

    // CLI args take precedence over config file
    let consumer_key = args
        .consumer_key
        .or_else(|| config.as_ref().and_then(|c| c.consumer_key.clone()))
        .ok_or(ArchivrError::NoConsumerKeyAndSecret)?;

    let consumer_secret = args
        .consumer_secret
        .or_else(|| config.as_ref().and_then(|c| c.consumer_secret.clone()))
        .ok_or(ArchivrError::NoConsumerKeyAndSecret)?;

    // check if we are already authenticated
    // check if consumer key and secret are specified
    // check config file for extra settings
    // if not authenticated, go through authentication flow
    // if authenticated, proceed with backup

    let project_dir = directories::ProjectDirs::from(PROJECT_QUALIFIER, "", PROJECT_NAME)
        .ok_or_else(|| anyhow::anyhow!("Could not determine project directory"))?;

    let data_dir = project_dir.data_local_dir();

    let data_dir_exists = fs_err::exists(data_dir)?;

    if !data_dir_exists {
        std::fs::create_dir_all(data_dir)?;
    }

    let auth_file_path = data_dir.join("auth.json");

    let auth_file_exists = fs_err::exists(auth_file_path.clone())?;

    let client = if auth_file_exists {
        let auth_str = fs_err::read_to_string(auth_file_path)?;
        let auth: Auth = serde_json::from_str(&auth_str)?;

        Crabrave::builder()
            .consumer_key(consumer_key)
            .consumer_secret(consumer_secret)
            .access_token(auth.access_token)
            .build()?
    } else {
        let oauth_config = crabrave::oauth::OAuth2Config::new(
            consumer_key.clone(),
            consumer_secret.clone(),
            format!(
                "http://localhost:{}/redirect",
                archivr::DEFAULT_CALLBACK_PORT
            ),
        );

        let auth_url = oauth_config.authorize_url().0;

        match open::that(auth_url) {
            Ok(_) => {
                println!("Opening Tumblr to complete OAuth...");
            }
            Err(e) => {
                // println!("Could not open browser. Please navigate to this URL and paste the code you get back below: a")
                todo!("Manually have user paste in code");
            }
        }

        let auth_code = archivr::capture_callback().await?;
        let oauth2_token = oauth_config.exchange_code(auth_code).await?;

        let auth = Auth {
            access_token: oauth2_token.access_token.clone(),
            refresh_token: oauth2_token.refresh_token,
        };

        let _ = fs_err::write(auth_file_path, serde_json::to_string(&auth)?)?;

        Crabrave::builder()
            .consumer_key(consumer_key)
            .consumer_secret(consumer_secret)
            .access_token(&oauth2_token.access_token)
            .build()?
    };

    let blog_name = args.blog_name;

    let archive_path = Utf8PathBuf::try_from(std::env::current_dir()?)?.join(&blog_name);

    if !fs_err::exists(&archive_path)? {
        fs_err::create_dir(&archive_path)?;
    }

    // Set up the template renderer
    let renderer = if let Some(ref template_path) = args.template {
        PostRenderer::from_file(template_path)?
    } else {
        PostRenderer::new()
    };

    let mut post_response = client.blogs(blog_name.clone()).posts().send().await?;

    let post_count = post_response.total_posts;
    let mut post_offset: usize = 0;

    loop {
        println!(
            "({}/{}) Fetching next batch of posts...",
            post_offset, post_count
        );

        post_offset += post_response.posts.len();

        for post in &post_response.posts {
            log::info!("processing post {}", post.id);

            // Render the post using the template
            let rendered = renderer.render(post)?;

            // Determine output path
            let output_file = if args.directories {
                let post_dir = archive_path.join(&post.id);
                if !fs_err::exists(&post_dir)? {
                    fs_err::create_dir(&post_dir)?;
                }
                post_dir.join("index.html")
            } else {
                archive_path.join(format!("{}.html", post.id))
            };

            fs_err::write(&output_file, &rendered)?;
            log::debug!("saved post {} to {}", post.id, output_file);
        }

        post_response = client
            .blogs(blog_name.clone())
            .posts()
            .offset(post_offset.try_into().unwrap())
            .send()
            .await?;

        if post_offset > post_count.try_into().unwrap() {
            break;
        }
    }

    Ok(())
}
