use std::io::Write;
use std::sync::Arc;

use anyhow::Context as _;
use camino::{Utf8Path, Utf8PathBuf};
use crabrave::{CookieJar, Crabrave, oauth::OAuthScope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    let data = serde_json::to_string(auth)?;

    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt as _;
        use std::io::Write as _;
        // Create the file 0600 from the start so the token is never briefly
        // world-readable, and tighten perms in case it pre-existed looser.
        let mut file = fs_err::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(data.as_bytes())?;
        use std::os::unix::fs::PermissionsExt as _;
        fs_err::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    fs_err::write(path, data)?;

    Ok(())
}

fn make_oauth_config(
    consumer_key: &str,
    consumer_secret: &str,
) -> crabrave::CrabResult<crabrave::oauth::OAuth2Config> {
    crabrave::oauth::OAuth2Config::new(
        consumer_key.to_owned(),
        consumer_secret.to_owned(),
        format!("http://localhost:{}/redirect", crate::DEFAULT_CALLBACK_PORT),
        vec![OAuthScope::Basic, OAuthScope::Offline],
    )
}

fn auth_from_token(token: crabrave::oauth::OAuth2Token, previous_refresh: Option<String>) -> Auth {
    let expires_at = compute_expires_at(&token);
    Auth {
        access_token: token.access_token,
        // Preserve the old refresh token if the server didn't issue a new one.
        refresh_token: token.refresh_token.or(previous_refresh),
        expires_at,
    }
}

/// Base URL for Tumblr's internal dashboard API
const DASHBOARD_API_URL: &str = "https://www.tumblr.com/api/v2";

/// The authenticated API session for a run: the current credentials, the
/// client built from them, and everything needed to rebuild that client
/// when the access token is renewed. Cheap to clone and safe to share across
/// tasks; every clone sees renewals.
#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    consumer_key: String,
    consumer_secret: String,
    auth_file_path: Utf8PathBuf,
    cookie_jar: Option<Arc<CookieJar>>,
    dashboard: bool,
    state: parking_lot::Mutex<SessionState>,
    /// Serializes renewals. Tumblr may rotate the refresh token on use, so
    /// two tasks renewing from the same snapshot would have the loser's
    /// request fail for a token the winner just retired; waiting here (and
    /// re-checking the generation after) turns that into a no-op instead.
    renewal: tokio::sync::Semaphore,
}

struct SessionState {
    auth: Auth,
    client: Crabrave,
    /// Bumped on every adopted renewal so a renewal that raced another one
    /// can tell it lost (see [`Session::adopt`]).
    generation: u64,
}

impl Session {
    fn new(
        consumer_key: &str,
        consumer_secret: &str,
        auth: Auth,
        auth_file_path: Utf8PathBuf,
        cookie_jar: Option<Arc<CookieJar>>,
        dashboard: bool,
    ) -> anyhow::Result<Self> {
        let client = build_client(
            consumer_key,
            consumer_secret,
            &auth.access_token,
            cookie_jar.clone(),
            dashboard,
        )?;
        Ok(Self {
            inner: Arc::new(SessionInner {
                consumer_key: consumer_key.to_owned(),
                consumer_secret: consumer_secret.to_owned(),
                auth_file_path,
                cookie_jar,
                dashboard,
                state: parking_lot::Mutex::new(SessionState {
                    auth,
                    client,
                    generation: 0,
                }),
                renewal: tokio::sync::Semaphore::new(1),
            }),
        })
    }

    /// The underlying HTTP client, for fetching media that needs no
    /// authorization. Shares its connection pool with the API client.
    pub fn http_client(&self) -> reqwest::Client {
        self.inner.state.lock().client.client().clone()
    }

    /// The refresh token to renew with and the generation it belongs to.
    fn snapshot(&self) -> (Option<String>, u64) {
        let state = self.inner.state.lock();
        (state.auth.refresh_token.clone(), state.generation)
    }

    /// Installs a renewed token obtained from the `generation` snapshot,
    /// rebuilding the client and persisting the credentials. Returns `false`
    /// without changing anything when another renewal was adopted in the
    /// meantime: that one's credentials are newer, and the server may
    /// already have rotated the refresh token this result was derived from.
    fn adopt(&self, generation: u64, token: crabrave::oauth::OAuth2Token) -> anyhow::Result<bool> {
        let mut state = self.inner.state.lock();
        if state.generation != generation {
            return Ok(false);
        }
        let auth = auth_from_token(token, state.auth.refresh_token.clone());
        let client = build_client(
            &self.inner.consumer_key,
            &self.inner.consumer_secret,
            &auth.access_token,
            self.inner.cookie_jar.clone(),
            self.inner.dashboard,
        )?;
        // The server has already retired the old refresh token by now, so the
        // new credentials are used for this run even if they can't be saved;
        // the failure costs an interactive sign-in next run, not this backup.
        if let Err(e) = save_auth(&auth, &self.inner.auth_file_path) {
            log::warn!(
                "could not save the renewed credentials to {}: {e:#}",
                self.inner.auth_file_path
            );
        }
        state.auth = auth;
        state.client = client;
        state.generation += 1;
        Ok(true)
    }

    /// Renews the credentials unless they have already been renewed since
    /// the `observed` generation was read. Single-flight: a caller that
    /// waited behind another renewal finds the generation advanced and
    /// returns without contacting the server. `Ok(false)` when there is no
    /// refresh token to renew with.
    async fn renew_from(&self, observed: u64) -> anyhow::Result<bool> {
        let _permit = self
            .inner
            .renewal
            .acquire()
            .await
            .map_err(|_closed| anyhow::anyhow!("renewal gate closed"))?;
        let (refresh_token, generation) = self.snapshot();
        if generation != observed {
            log::info!("access token already renewed by a concurrent request");
            return Ok(true);
        }
        let Some(refresh_token) = refresh_token else {
            return Ok(false);
        };
        let oauth_config =
            make_oauth_config(&self.inner.consumer_key, &self.inner.consumer_secret)?;
        let token = oauth_config
            .refresh_access_token(refresh_token)
            .await
            .map_err(anyhow::Error::new)
            .context(
                "the access token has expired and could not be refreshed; \
                 run again with --reauth to sign in afresh",
            )?;
        self.adopt(generation, token)?;
        log::info!("access token renewed");
        Ok(true)
    }
}

impl crate::api::ClientSource for Session {
    fn client(&self) -> Crabrave {
        self.inner.state.lock().client.clone()
    }

    fn is_expired(&self) -> bool {
        self.inner.state.lock().auth.is_expired()
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        let (_, observed) = self.snapshot();
        self.renew_from(observed).await
    }
}

fn build_client(
    consumer_key: &str,
    consumer_secret: &str,
    access_token: &str,
    cookie_jar: Option<Arc<CookieJar>>,
    dashboard: bool,
) -> anyhow::Result<Crabrave> {
    let mut builder = Crabrave::builder()
        .consumer_key(consumer_key.to_owned())
        .consumer_secret(consumer_secret.to_owned())
        .access_token(access_token);

    if let Some(jar) = cookie_jar {
        builder = builder.cookie_jar(jar);
    }

    if dashboard {
        builder = builder.base_url(DASHBOARD_API_URL);
    }

    let client = builder.build()?;
    Ok(client)
}

async fn interactive_auth(
    consumer_key: &str,
    consumer_secret: &str,
    auth_file_path: &Utf8Path,
    headless: bool,
) -> anyhow::Result<Auth> {
    let oauth_config = make_oauth_config(consumer_key, consumer_secret)?;
    let (auth_url, csrf_token) = oauth_config.authorize_url();

    // Always print the URL to stdout so headless/no-RUST_LOG users can see it
    writeln!(
        std::io::stdout(),
        "Please navigate to this URL to authenticate:\n  {auth_url}"
    )?;

    let (code, state) = if headless {
        let url = crate::read_callback_url_from_stdin()?;
        crate::parse_code_from_url(&url)?
    } else {
        match open::that(auth_url.as_str()) {
            Ok(()) => log::debug!("opened browser for authentication"),
            Err(_e) => log::debug!("could not open browser automatically"),
        }
        crate::capture_callback(csrf_token.secret()).await?
    };

    // Verify CSRF state parameter. Tumblr always echoes `state`, so a callback
    // without one cannot be a genuine redirect — treat it as hostile rather
    // than skipping verification.
    match state {
        Some(ref s) if s != csrf_token.secret() => {
            return Err(crate::ArchivrError::CsrfMismatch {
                expected: csrf_token.secret().clone(),
                actual: s.clone(),
            }
            .into());
        }
        None => return Err(crate::ArchivrError::MissingCsrfState.into()),
        Some(_) => {}
    }

    let auth = auth_from_token(oauth_config.exchange_code(code).await?, None);
    save_auth(&auth, auth_file_path)?;
    Ok(auth)
}

/// Parses a Netscape/Mozilla-format cookie file into a [`CookieJar`].
///
/// The format is tab-separated with fields:
/// `domain \t include_subdomains \t path \t secure \t expiry \t name \t value`
///
/// Lines starting with `#` are comments and blank lines are skipped.
fn parse_cookie_file(path: &Utf8Path) -> anyhow::Result<Arc<CookieJar>> {
    let contents = fs_err::read_to_string(path)?;
    let jar = CookieJar::default();
    let tumblr_url: url::Url = "https://www.tumblr.com"
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse tumblr URL: {e}"))?;

    for (lineno, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // HttpOnly cookies are encoded with a `#HttpOnly_` marker on the domain
        // field; strip it so they aren't mistaken for comment lines and dropped.
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.starts_with('#') {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            // Identify the line by number only: any slice of a malformed line
            // (including its first "field", which for a tab-less line is the
            // whole line) can still contain a live session cookie value.
            log::warn!(
                "skipping malformed cookie line {} ({} fields, expected 7)",
                lineno + 1,
                fields.len()
            );
            continue;
        }

        let domain = fields[0];
        // Only load cookies relevant to Tumblr
        if !domain.contains("tumblr.com") {
            continue;
        }

        let name = fields[5];
        let value = fields[6];
        jar.add_cookie_str(&format!("{name}={value}"), &tumblr_url);
    }

    Ok(Arc::new(jar))
}

/// Establishes the run's API session from the saved credentials, refreshing
/// an expired token or falling back to the interactive flow as needed.
pub async fn authenticate(
    consumer_key: &str,
    consumer_secret: &str,
    data_dir: &Utf8Path,
    reauth: bool,
    cookies_file: Option<&Utf8Path>,
    dashboard: bool,
    headless: bool,
) -> anyhow::Result<Session> {
    fs_err::create_dir_all(data_dir)?;
    let auth_file_path = data_dir.join("auth.json");

    let cookie_jar = cookies_file.map(parse_cookie_file).transpose()?;

    let auth = if reauth {
        interactive_auth(consumer_key, consumer_secret, &auth_file_path, headless).await?
    } else {
        match load_saved_auth(consumer_key, consumer_secret, &auth_file_path).await? {
            Some(auth) => auth,
            None => {
                interactive_auth(consumer_key, consumer_secret, &auth_file_path, headless).await?
            }
        }
    };

    Session::new(
        consumer_key,
        consumer_secret,
        auth,
        auth_file_path,
        cookie_jar,
        dashboard,
    )
}

/// The saved credentials, refreshed if expired; `None` when there are none
/// usable (missing, unreadable, or expired with no working refresh token),
/// in which case the interactive flow is the only way forward.
async fn load_saved_auth(
    consumer_key: &str,
    consumer_secret: &str,
    auth_file_path: &Utf8Path,
) -> anyhow::Result<Option<Auth>> {
    if !fs_err::exists(auth_file_path)? {
        return Ok(None);
    }
    let auth_str = fs_err::read_to_string(auth_file_path)?;
    // A corrupt token file shouldn't dead-end the tool: fall back to
    // interactive auth (which rewrites it) with a visible notice.
    let auth: Auth = match serde_json::from_str(&auth_str) {
        Ok(auth) => auth,
        Err(e) => {
            writeln!(
                std::io::stdout(),
                "Saved authentication at {auth_file_path} is unreadable ({e}); \
                 re-running interactive authentication."
            )?;
            return Ok(None);
        }
    };

    if !auth.is_expired() {
        return Ok(Some(auth));
    }

    let Some(refresh_token) = auth.refresh_token.clone() else {
        return Ok(None);
    };
    log::info!("access token expired, attempting refresh");
    let oauth_config = make_oauth_config(consumer_key, consumer_secret)?;
    match oauth_config.refresh_access_token(refresh_token).await {
        Ok(new_token) => {
            let refreshed = auth_from_token(new_token, auth.refresh_token);
            save_auth(&refreshed, auth_file_path)?;
            Ok(Some(refreshed))
        }
        Err(e) => {
            log::warn!("token refresh failed: {e}, falling back to interactive auth");
            Ok(None)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::api::ClientSource as _;
    use crabrave::oauth::OAuth2Token;

    fn temp_auth_path(tag: &str) -> camino::Utf8PathBuf {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        dir.join(format!(
            "archivr-test-auth-{tag}-{}.json",
            std::process::id()
        ))
    }

    fn expired_session(path: &Utf8Path) -> Session {
        let auth = Auth {
            access_token: "stale".to_owned(),
            refresh_token: Some("refresh-0".to_owned()),
            expires_at: Some(chrono::Utc::now().timestamp() - 1),
        };
        Session::new("key", "secret", auth, path.to_owned(), None, false).unwrap()
    }

    fn renewed(access_token: &str, refresh_token: Option<&str>) -> OAuth2Token {
        OAuth2Token {
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.map(str::to_owned),
            expires_in: Some(2520),
        }
    }

    #[test]
    fn adopting_a_renewed_token_clears_expiry_and_persists_it() {
        let path = temp_auth_path("adopt");
        let session = expired_session(&path);
        assert!(session.is_expired());

        let (refresh_token, generation) = session.snapshot();
        assert_eq!(refresh_token.as_deref(), Some("refresh-0"));
        assert!(session.adopt(generation, renewed("fresh", None)).unwrap());

        assert!(!session.is_expired());
        let on_disk: Auth = serde_json::from_str(&fs_err::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk.access_token, "fresh");
        // The server issued no new refresh token, so the old one is kept.
        assert_eq!(on_disk.refresh_token.as_deref(), Some("refresh-0"));
        // The next renewal starts from the adopted credentials.
        let (refresh_token, next_generation) = session.snapshot();
        assert_eq!(refresh_token.as_deref(), Some("refresh-0"));
        assert_ne!(next_generation, generation);

        fs_err::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn a_renewal_that_waited_behind_a_completed_one_does_not_contact_the_server() {
        let path = temp_auth_path("waited");
        let session = expired_session(&path);
        let (_, stale_generation) = session.snapshot();
        session
            .adopt(stale_generation, renewed("winner", None))
            .unwrap();

        // The credentials these would have been constructed from are bogus,
        // so reaching the token endpoint could only fail.
        assert!(session.renew_from(stale_generation).await.unwrap());
        let on_disk: Auth = serde_json::from_str(&fs_err::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk.access_token, "winner");

        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn a_renewed_token_is_used_even_when_it_cannot_be_persisted() {
        let path = Utf8Path::new("/nonexistent-archivr-test-dir/auth.json");
        let session = expired_session(path);
        let (_, generation) = session.snapshot();
        assert!(session.adopt(generation, renewed("fresh", None)).unwrap());
        assert!(!session.is_expired());
    }

    #[test]
    fn a_renewal_that_lost_the_race_is_discarded() {
        let path = temp_auth_path("race");
        let session = expired_session(&path);

        let (_, generation) = session.snapshot();
        assert!(
            session
                .adopt(generation, renewed("winner", Some("refresh-1")))
                .unwrap()
        );
        // A second task that started from the same snapshot finishes later;
        // its token must not clobber the winner's, whose refresh token the
        // server may already have rotated.
        assert!(
            !session
                .adopt(generation, renewed("loser", Some("refresh-2")))
                .unwrap()
        );

        let on_disk: Auth = serde_json::from_str(&fs_err::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk.access_token, "winner");
        assert_eq!(on_disk.refresh_token.as_deref(), Some("refresh-1"));
        assert!(!session.is_expired());

        fs_err::remove_file(&path).unwrap();
    }
}
