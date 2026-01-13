pub mod cmd;

pub use cmd::Args;

use camino::Utf8PathBuf;
use clap::Parser;
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
}

async fn capture_callback() -> anyhow::Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", DEFAULT_CALLBACK_PORT)).await?;

    let (mut stream, _) = listener.accept().await?;
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);

    // Read the request line: GET /callback?code=xyz HTTP/1.1
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Extract the code from the query string
    let code = parse_code_from_request(&request_line)?;

    // Send a minimal response
    let body = "<h1>Success!</h1><p>You can close this tab.</p>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    writer.write_all(response.as_bytes()).await?;

    Ok(code)
}

fn parse_code_from_request(request_line: &str) -> anyhow::Result<String> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or(ArchivrError::MalformedCallback)?;

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == "code" {
                return Ok(urlencoding::decode(value)?.into_owned());
            }
            if key == "error" {
                return Err(ArchivrError::OAuth(urlencoding::decode(value)?.into_owned()).into());
            }
        }
    }

    Err(ArchivrError::MalformedCallback.into())
}
