pub mod auth;
pub mod cmd;
pub mod config;
pub mod images;
pub mod job;
pub mod template;

pub use cmd::Args;
pub use config::{Config, ResolvedConfig};
pub use job::JobState;
pub use template::{DEFAULT_TEMPLATE, PostRenderer};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

pub const DEFAULT_CALLBACK_PORT: u16 = 6263;

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
}

pub async fn capture_callback() -> anyhow::Result<(String, Option<String>)> {
    let listener = TcpListener::bind(("127.0.0.1", DEFAULT_CALLBACK_PORT)).await?;

    let (mut stream, _) = listener.accept().await?;
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);

    // Read the request line: GET /callback?code=xyz&state=abc HTTP/1.1
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Extract the code and state from the query string
    let code_and_state = parse_code_from_request(&request_line)?;

    // Send a minimal response
    let body = "<h1>Success!</h1><p>You can close this tab.</p>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    writer.write_all(response.as_bytes()).await?;

    Ok(code_and_state)
}

/// Parses the authorization code and optional state from an OAuth callback HTTP request line.
fn parse_code_from_request(request_line: &str) -> anyhow::Result<(String, Option<String>)> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or(ArchivrError::MalformedCallback)?;

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

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
}
