use std::io::Write;

use camino::Utf8Path;
use crabrave::Crabrave;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Auth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Auth {
    /// Returns `true` if the token is known to be expired (with a 60-second safety buffer).
    /// Returns `false` if no expiry info exists (treat legacy tokens as valid).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => chrono::Utc::now().timestamp() >= expires_at - 60,
            None => false,
        }
    }
}

fn compute_expires_at(token: &crabrave::oauth::OAuth2Token) -> Option<i64> {
    token
        .expires_in
        .map(|secs| chrono::Utc::now().timestamp() + secs as i64)
}

fn save_auth(auth: &Auth, path: &Utf8Path) -> anyhow::Result<()> {
    fs_err::write(path, serde_json::to_string(auth)?)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs_err::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

fn make_oauth_config(
    consumer_key: &str,
    consumer_secret: &str,
) -> crabrave::oauth::OAuth2Config {
    crabrave::oauth::OAuth2Config::new(
        consumer_key.to_owned(),
        consumer_secret.to_owned(),
        format!(
            "http://localhost:{}/redirect",
            crate::DEFAULT_CALLBACK_PORT
        ),
    )
}

fn build_client(
    consumer_key: &str,
    consumer_secret: &str,
    access_token: &str,
) -> anyhow::Result<Crabrave> {
    let client = Crabrave::builder()
        .consumer_key(consumer_key.to_owned())
        .consumer_secret(consumer_secret.to_owned())
        .access_token(access_token)
        .build()?;
    Ok(client)
}

async fn interactive_auth(
    consumer_key: &str,
    consumer_secret: &str,
    auth_file_path: &Utf8Path,
) -> anyhow::Result<Crabrave> {
    let oauth_config = make_oauth_config(consumer_key, consumer_secret);
    let (auth_url, csrf_token) = oauth_config.authorize_url();

    // Always print the URL to stdout so headless/no-RUST_LOG users can see it
    writeln!(
        std::io::stdout(),
        "Please navigate to this URL to authenticate:\n  {auth_url}"
    )?;

    match open::that(auth_url.as_str()) {
        Ok(()) => log::debug!("opened browser for authentication"),
        Err(_e) => log::debug!("could not open browser automatically"),
    }

    let (code, state) = crate::capture_callback().await?;

    // Verify CSRF state parameter
    match state {
        Some(ref s) if s != csrf_token.secret() => {
            return Err(crate::ArchivrError::CsrfMismatch {
                expected: csrf_token.secret().clone(),
                actual: s.clone(),
            }
            .into());
        }
        None => {
            log::warn!("no state parameter in OAuth callback; skipping CSRF verification");
        }
        Some(_) => {}
    }

    let oauth2_token = oauth_config.exchange_code(code).await?;
    let expires_at = compute_expires_at(&oauth2_token);

    let auth = Auth {
        access_token: oauth2_token.access_token.clone(),
        refresh_token: oauth2_token.refresh_token,
        expires_at,
    };

    save_auth(&auth, auth_file_path)?;
    build_client(consumer_key, consumer_secret, &auth.access_token)
}

pub async fn authenticate(
    consumer_key: &str,
    consumer_secret: &str,
    data_dir: &Utf8Path,
    reauth: bool,
) -> anyhow::Result<Crabrave> {
    fs_err::create_dir_all(data_dir)?;
    let auth_file_path = data_dir.join("auth.json");

    if reauth {
        return interactive_auth(consumer_key, consumer_secret, &auth_file_path).await;
    }

    if fs_err::exists(&auth_file_path)? {
        let auth_str = fs_err::read_to_string(&auth_file_path)?;
        let auth: Auth = serde_json::from_str(&auth_str)?;

        if !auth.is_expired() {
            return build_client(consumer_key, consumer_secret, &auth.access_token);
        }

        // Token is expired — try refreshing
        if let Some(refresh_token) = auth.refresh_token.clone() {
            log::info!("access token expired, attempting refresh");
            let oauth_config = make_oauth_config(consumer_key, consumer_secret);
            match oauth_config
                .refresh_access_token(refresh_token)
                .await
            {
                Ok(new_token) => {
                    let expires_at = compute_expires_at(&new_token);
                    // Preserve old refresh token if the server didn't issue a new one
                    let refresh_token = new_token.refresh_token.or(auth.refresh_token);
                    let refreshed_auth = Auth {
                        access_token: new_token.access_token,
                        refresh_token,
                        expires_at,
                    };
                    save_auth(&refreshed_auth, &auth_file_path)?;
                    return build_client(
                        consumer_key,
                        consumer_secret,
                        &refreshed_auth.access_token,
                    );
                }
                Err(_e) => {
                    log::warn!("token refresh failed, falling back to interactive auth");
                }
            }
        }
    }

    interactive_auth(consumer_key, consumer_secret, &auth_file_path).await
}
