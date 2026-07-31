pub mod auth;
pub mod cmd;
pub mod config;
pub mod fsutil;
pub mod images;
pub mod job;
pub mod state;
pub mod template;

pub use cmd::Args;
pub use config::{Config, ResolvedConfig};
pub use job::JobState;
pub use state::BackupState;
pub use template::{DEFAULT_TEMPLATE, PostRenderer};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

pub const DEFAULT_CALLBACK_PORT: u16 = 6263;

/// How long to wait for the browser to hit the OAuth callback before giving up.
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Per-connection read deadline, so one stalled local connection can't hold
/// the listener (and with it the whole auth flow) until [`CALLBACK_TIMEOUT`].
const CONNECTION_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub type PostTimestamp = i64;

#[derive(Debug, Error)]
pub enum ArchivrError {
    #[error("Callback was not in expected format. Please report this bug.")]
    MalformedCallback,
    #[error("Error from OAuth: {0}")]
    OAuth(String),
    #[error("Consumer key not specified (use --consumer-key or set it in the config file)")]
    NoConsumerKey,
    #[error("Consumer secret not specified (use --consumer-secret or set it in the config file)")]
    NoConsumerSecret,
    #[error("CSRF state mismatch: expected {expected}, got {actual}")]
    CsrfMismatch { expected: String, actual: String },
    #[error(
        "OAuth callback did not include the CSRF state parameter, so it cannot \
         have come from Tumblr. Refusing the authorization code; re-run to \
         authenticate again."
    )]
    MissingCsrfState,
}

pub async fn capture_callback(expected_state: &str) -> anyhow::Result<(String, Option<String>)> {
    let listener = TcpListener::bind(("127.0.0.1", DEFAULT_CALLBACK_PORT))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "could not listen on 127.0.0.1:{DEFAULT_CALLBACK_PORT} for the OAuth \
                 callback ({e}). Another archivr instance may be mid-authentication, \
                 or another program is using the port."
            )
        })?;

    tokio::time::timeout(CALLBACK_TIMEOUT, accept_callback(&listener, expected_state))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out after {}s waiting for the OAuth callback. Re-run and \
                 complete authentication in the browser, or use --headless.",
                CALLBACK_TIMEOUT.as_secs()
            )
        })?
}

/// Best-effort: the HTTP response is purely cosmetic (the browser tab's
/// success/failure page), so a peer that has already hung up must not be able
/// to fail the auth flow.
async fn write_response(
    writer: &mut tokio::net::tcp::WriteHalf<'_>,
    status_line: &str,
    body: &str,
) {
    let response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    if let Err(e) = writer.write_all(response.as_bytes()).await {
        log::debug!("failed to write OAuth callback response: {e}");
    }
}

/// Accepts connections until one carries an OAuth callback bearing this
/// session's CSRF `state`, then replies and returns the parsed code/state.
///
/// The port is fixed and world-connectable on loopback, so only a request that
/// proves it came through Tumblr — by echoing our `state` (RFC 6749 echoes it
/// on error redirects too) — may terminate the flow, whether with a code or a
/// genuine `error=` denial. Everything else (empty pre-connect sockets, port
/// scanners, forged error/code injections without the state) gets a 404 and
/// the listener keeps waiting.
async fn accept_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> anyhow::Result<(String, Option<String>)> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Read the request line: GET /callback?code=xyz&state=abc HTTP/1.1
        let mut request_line = String::new();
        let bytes_read = match tokio::time::timeout(
            CONNECTION_READ_TIMEOUT,
            reader.read_line(&mut request_line),
        )
        .await
        {
            Ok(Ok(n)) => n,
            // A peer that resets mid-request is a stray connection, not a
            // failure of the auth flow.
            Ok(Err(e)) => {
                log::debug!("ignoring unreadable connection on the OAuth port: {e}");
                continue;
            }
            Err(_elapsed) => {
                log::debug!("ignoring stalled connection on the OAuth port");
                continue;
            }
        };
        if bytes_read == 0 || request_line.trim().is_empty() {
            continue;
        }

        let state_matches =
            request_query_param(&request_line, "state").is_some_and(|s| s == expected_state);

        match parse_code_from_request(&request_line) {
            Ok(code_and_state) if state_matches => {
                let body = "<h1>Success!</h1><p>You can close this tab.</p>";
                write_response(&mut writer, "200 OK", body).await;
                return Ok(code_and_state);
            }
            Err(e)
                if state_matches
                    && e.downcast_ref::<ArchivrError>()
                        .is_some_and(|e| matches!(e, ArchivrError::OAuth(_))) =>
            {
                let body = "<h1>Authentication failed</h1><p>You can close this tab.</p>";
                write_response(&mut writer, "400 Bad Request", body).await;
                return Err(e);
            }
            Ok(_) | Err(_) => {
                log::debug!(
                    "ignoring request on the OAuth port without a matching state parameter"
                );
                write_response(&mut writer, "404 Not Found", "").await;
            }
        }
    }
}

/// Extracts a single query parameter from an HTTP request line, if present.
fn request_query_param(request_line: &str, key: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        if let Some((k, value)) = pair.split_once('=')
            && k == key
        {
            return urlencoding::decode(value).ok().map(|v| v.into_owned());
        }
    }
    None
}

/// Extracts the authorization code and optional state from a query string.
fn parse_code_from_query(query: &str) -> anyhow::Result<(String, Option<String>)> {
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "code" => code = Some(urlencoding::decode(value)?.into_owned()),
                "state" => state = Some(urlencoding::decode(value)?.into_owned()),
                "error" => {
                    return Err(
                        ArchivrError::OAuth(urlencoding::decode(value)?.into_owned()).into(),
                    );
                }
                _ => {}
            }
        }
    }

    let code = code.ok_or(ArchivrError::MalformedCallback)?;
    Ok((code, state))
}

/// Parses the authorization code and optional state from an OAuth callback HTTP request line.
fn parse_code_from_request(request_line: &str) -> anyhow::Result<(String, Option<String>)> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or(ArchivrError::MalformedCallback)?;

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    parse_code_from_query(query)
}

/// Parses the authorization code and optional state from a full OAuth redirect URL.
///
/// Accepts URLs like `http://localhost:6263/redirect?code=abc&state=xyz`.
pub fn parse_code_from_url(url: &str) -> anyhow::Result<(String, Option<String>)> {
    let url = url.trim();
    let query = url
        .split_once('?')
        .map(|(_, q)| q)
        .ok_or(ArchivrError::MalformedCallback)?;
    parse_code_from_query(query)
}

/// Prompts the user to paste the OAuth redirect URL and reads it from stdin.
pub fn read_callback_url_from_stdin() -> anyhow::Result<String> {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    writeln!(
        stdout,
        "\nAfter authenticating in your browser, the page will fail to load."
    )?;
    writeln!(
        stdout,
        "Copy the URL from your browser's address bar and paste it here."
    )?;
    write!(stdout, "\nRedirect URL: ")?;
    stdout.flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim().to_owned();

    if input.is_empty() {
        return Err(ArchivrError::MalformedCallback.into());
    }

    Ok(input)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_with_code_and_state() {
        let request = "GET /redirect?code=abc123&state=csrf_token HTTP/1.1";
        let (code, state) = parse_code_from_request(request).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("csrf_token"));
    }

    #[test]
    fn parse_callback_code_only() {
        let request = "GET /redirect?code=mycode HTTP/1.1";
        let (code, state) = parse_code_from_request(request).unwrap();
        assert_eq!(code, "mycode");
        assert!(state.is_none());
    }

    #[test]
    fn parse_callback_url_encoded_values() {
        let request = "GET /redirect?code=has%20space&state=a%26b HTTP/1.1";
        let (code, state) = parse_code_from_request(request).unwrap();
        assert_eq!(code, "has space");
        assert_eq!(state.as_deref(), Some("a&b"));
    }

    #[test]
    fn parse_callback_error_response() {
        let request = "GET /redirect?error=access_denied HTTP/1.1";
        let err = parse_code_from_request(request).unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn parse_callback_malformed_request() {
        assert!(parse_code_from_request("GARBAGE").is_err());
    }

    #[test]
    fn parse_callback_no_query_string() {
        // No code parameter → MalformedCallback
        assert!(parse_code_from_request("GET /redirect HTTP/1.1").is_err());
    }

    #[test]
    fn parse_url_with_code_and_state() {
        let url = "http://localhost:6263/redirect?code=abc123&state=csrf_token";
        let (code, state) = parse_code_from_url(url).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("csrf_token"));
    }

    #[test]
    fn parse_url_code_only() {
        let url = "http://localhost:6263/redirect?code=mycode";
        let (code, state) = parse_code_from_url(url).unwrap();
        assert_eq!(code, "mycode");
        assert!(state.is_none());
    }

    #[test]
    fn parse_url_with_whitespace() {
        let url = "  http://localhost:6263/redirect?code=abc123&state=xyz  \n";
        let (code, state) = parse_code_from_url(url).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("xyz"));
    }

    #[test]
    fn parse_url_error_response() {
        let url = "http://localhost:6263/redirect?error=access_denied";
        let err = parse_code_from_url(url).unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn parse_url_no_query_string() {
        assert!(parse_code_from_url("http://localhost:6263/redirect").is_err());
    }

    #[test]
    fn parse_url_url_encoded_values() {
        let url = "http://localhost:6263/redirect?code=has%20space&state=a%26b";
        let (code, state) = parse_code_from_url(url).unwrap();
        assert_eq!(code, "has space");
        assert_eq!(state.as_deref(), Some("a&b"));
    }
}
