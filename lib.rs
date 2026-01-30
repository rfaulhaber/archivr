pub mod auth;
pub mod cmd;
pub mod config;
pub mod job;
pub mod template;

pub use cmd::Args;
pub use config::{Config, ResolvedConfig};
pub use job::{JobState, LastRun};
pub use template::{DEFAULT_TEMPLATE, PostRenderer};

use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

pub const DEFAULT_CALLBACK_PORT: u16 = 6263;

#[derive(Debug, Error)]
pub enum ArchivrError {
    #[error("Callback was not in expected format. Please report this bug.")]
    MalformedCallback,
    #[error("Error from OAuth: {0}")]
    OAuth(String),
    #[error("Consumer key and secret not specified")]
    NoConsumerKeyAndSecret,
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
