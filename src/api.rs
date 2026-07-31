//! Shared execution policy for Tumblr API requests: a per-request timeout,
//! bounded retries with exponential backoff for transient failures, and
//! rate-limit sleeps honoring the API's requested wait.

use std::future::Future;
use std::io::Write;
use std::time::Duration;

use crabrave::CrabError;

/// Per-request deadline (crabrave sets no timeout of its own, so without this
/// a stalled connection hangs the backup forever). 90s mirrors tumblr-utils.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
/// Retries after the initial attempt, for transient API failures.
const MAX_TRANSIENT_RETRIES: u32 = 3;
/// Consecutive rate-limit sleeps tolerated while executing a single request.
const MAX_RATE_LIMIT_WAITS: u32 = 3;
/// Refuse to sleep longer than this on a rate limit: a wait above an hour
/// means the daily quota is exhausted, which only time will fix.
const MAX_RATE_LIMIT_SLEEP_SECS: u64 = 3600;

/// Writes a progress line to stdout, treating a closed pipe (e.g.
/// `archivr ... | head`) as success so the backup keeps running even after
/// the reader goes away.
pub fn say(args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    match writeln!(std::io::stdout(), "{args}") {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// [`say`] for stderr: error reporting that must not interleave with the
/// progress stream (and survives `archivr ... > log`).
pub fn say_error(args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    match writeln!(std::io::stderr(), "{args}") {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// A request that failed at the HTTP/API layer after the retry policy was
/// exhausted (timeout, unrecoverable rate limit). Typed rather than a bare
/// `anyhow!` so the exit-code mapping can classify it — see
/// [`is_http_failure`].
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ApiFailure(pub String);

/// Whether an error chain originates in the HTTP/API layer (as opposed to
/// disk, rendering, or configuration failures). Drives the distinct exit
/// code for network errors.
pub fn is_http_failure(e: &anyhow::Error) -> bool {
    e.downcast_ref::<CrabError>().is_some()
        || e.downcast_ref::<ApiFailure>().is_some()
        || e.downcast_ref::<reqwest::Error>().is_some()
}

/// Executes an API request with the shared timeout/retry/rate-limit policy.
///
/// `op` builds and sends a fresh request each attempt (builders are consumed
/// by `send`). Rate-limit sleeps honor the API's requested wait; transient
/// failures back off exponentially; anything else is returned immediately, so
/// callers can distinguish client-side rejections via [`is_client_api_error`].
pub async fn send_with_retries<T, F, Fut>(op: F, quiet: bool) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, CrabError>>,
{
    let mut transient_attempts: u32 = 0;
    let mut rate_limit_waits: u32 = 0;

    loop {
        let error: anyhow::Error = match tokio::time::timeout(REQUEST_TIMEOUT, op()).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(CrabError::RateLimit { retry_after })) => match retry_after {
                Some(secs) if secs > MAX_RATE_LIMIT_SLEEP_SECS => {
                    return Err(ApiFailure(format!(
                        "Hit rate limit; the API asks for a {secs}s wait (over an hour, so \
                         the daily quota is likely exhausted). Use --resume to continue \
                         from where you left off later."
                    ))
                    .into());
                }
                Some(secs) if rate_limit_waits < MAX_RATE_LIMIT_WAITS => {
                    rate_limit_waits += 1;
                    if !quiet {
                        say(format_args!(
                            "  Rate limited; waiting {secs}s before retrying \
                             ({rate_limit_waits}/{MAX_RATE_LIMIT_WAITS})..."
                        ))?;
                    }
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    continue;
                }
                Some(_) => {
                    return Err(ApiFailure(format!(
                        "Still rate limited after {MAX_RATE_LIMIT_WAITS} waits. \
                         Use --resume to continue from where you left off."
                    ))
                    .into());
                }
                None => {
                    return Err(ApiFailure(
                        "Hit rate limit, please retry later. \
                         Use --resume to continue from where you left off."
                            .to_owned(),
                    )
                    .into());
                }
            },
            Ok(Err(e)) if is_transient(&e) => anyhow::Error::new(e),
            Ok(Err(e)) => return Err(e.into()),
            Err(_elapsed) => anyhow::Error::new(ApiFailure(format!(
                "request timed out after {}s",
                REQUEST_TIMEOUT.as_secs()
            ))),
        };

        transient_attempts += 1;
        if transient_attempts > MAX_TRANSIENT_RETRIES {
            return Err(error.context(format!("giving up after {MAX_TRANSIENT_RETRIES} retries")));
        }
        let backoff = 1u64 << (transient_attempts - 1);
        log::info!(
            "transient API error ({error:#}); retrying in {backoff}s \
             (attempt {transient_attempts}/{MAX_TRANSIENT_RETRIES})"
        );
        tokio::time::sleep(Duration::from_secs(backoff)).await;
    }
}

/// Whether an API error is worth retrying.
fn is_transient(e: &CrabError) -> bool {
    match e {
        // Transport-level failures (connect refused, reset, mid-body drop).
        CrabError::Http(_) | CrabError::HttpClient(_) => true,
        // `status` here comes from Tumblr's JSON error envelope.
        CrabError::Api { status, .. } => matches!(*status, 500 | 502 | 503 | 504),
        // Non-JSON bodies on an error status surface as `Api { status }`, so
        // parse failures here mean a 2xx response that wasn't the expected
        // envelope (a WAF interstitial served with 200, a truncated body);
        // treating those as transient costs a genuine schema break only three
        // quick retries before the same error is returned.
        CrabError::Serialization(_) => true,
        _ => false,
    }
}

/// True when the error is a non-transient client-side API rejection (4xx):
/// the request itself was refused, and retrying cannot help. 429 is
/// excluded — a rate limit that surfaces through the JSON envelope instead
/// of [`CrabError::RateLimit`] must stay fatal, not become a per-item skip.
pub fn is_client_api_error(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<CrabError>(),
        Some(CrabError::Api { status, .. }) if (400..500).contains(status) && *status != 429
    )
}

/// True when the failure was an unparseable response body that survived the
/// retry budget — typically a WAF or edge proxy answering with an HTML error
/// page instead of the API's JSON envelope.
pub fn is_parse_error(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<CrabError>(),
        Some(CrabError::Serialization(_))
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn api_error(status: u16) -> anyhow::Error {
        anyhow::Error::new(CrabError::Api {
            status,
            message: "test".to_owned(),
            errors: Vec::new(),
        })
    }

    #[test]
    fn client_api_error_classification() {
        assert!(is_client_api_error(&api_error(403)));
        assert!(is_client_api_error(&api_error(404)));
        // Envelope-level rate limits are not per-item skippable.
        assert!(!is_client_api_error(&api_error(429)));
        assert!(!is_client_api_error(&api_error(500)));
        assert!(!is_client_api_error(&anyhow::anyhow!("plain error")));
    }

    #[test]
    fn client_api_error_survives_context_layers() {
        let e = api_error(404)
            .context("fetching notes")
            .context("giving up after 3 retries");
        assert!(is_client_api_error(&e));
    }

    #[test]
    fn http_failure_classification() {
        assert!(is_http_failure(&api_error(500)));
        assert!(is_http_failure(
            &anyhow::Error::new(ApiFailure("timed out".to_owned())).context("fetching page")
        ));
        assert!(!is_http_failure(&anyhow::anyhow!("disk full")));
    }

    #[test]
    fn parse_error_classification() {
        let serde_err = serde_json::from_str::<u32>("<html>").unwrap_err();
        let e = anyhow::Error::new(CrabError::Serialization(serde_err)).context("giving up");
        assert!(is_parse_error(&e));
        assert!(!is_parse_error(&api_error(404)));
    }
}
