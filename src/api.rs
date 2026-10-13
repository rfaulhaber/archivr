//! Shared execution policy for Tumblr API requests: a per-request timeout,
//! bounded retries with exponential backoff for transient failures, and
//! rate-limit sleeps honoring the API's requested wait.

use std::future::Future;
use std::io::Write;
use std::time::Duration;

use anyhow::Context as _;
use crabrave::{CrabError, Crabrave};

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

/// Where each request attempt gets its client from. The client's access token
/// is fixed for its lifetime, and Tumblr's tokens expire well within the span
/// of a long backup (a single rate-limit sleep can outlive one), so the policy
/// loop asks for a client per attempt and renews the credentials between
/// attempts instead of holding one client for the whole run.
pub trait ClientSource {
    /// A client carrying the current credentials.
    fn client(&self) -> Crabrave;
    /// Whether the current access token is past (or within a safety margin
    /// of) its expiry, as recorded when it was issued.
    fn is_expired(&self) -> bool;
    /// Obtains a fresh access token and makes subsequent [`Self::client`]
    /// calls use it. `Ok(false)` means renewal is impossible (no refresh
    /// token was ever saved), as opposed to attempted and failed.
    fn refresh(&self) -> impl Future<Output = anyhow::Result<bool>> + Send;
}

/// Renews the source's credentials under the per-request deadline: the token
/// endpoint is one more network call that can stall, and a stall here would
/// otherwise hang the backup forever.
async fn renew<S: ClientSource + ?Sized>(source: &S, why: &str) -> anyhow::Result<bool> {
    match tokio::time::timeout(REQUEST_TIMEOUT, source.refresh()).await {
        Ok(result) => result.with_context(|| format!("renewing the access token {why}")),
        Err(_elapsed) => Err(ApiFailure(format!(
            "renewing the access token {why} timed out after {}s",
            REQUEST_TIMEOUT.as_secs()
        ))
        .into()),
    }
}

/// Executes an API request with the shared timeout/retry/rate-limit policy.
///
/// `op` builds and sends a fresh request each attempt (builders are consumed
/// by `send`). Rate-limit sleeps honor the API's requested wait; transient
/// failures back off exponentially; anything else is returned immediately, so
/// callers can distinguish client-side rejections via [`is_client_api_error`].
pub async fn send_with_retries<S, T, F, Fut>(source: &S, op: F, quiet: bool) -> anyhow::Result<T>
where
    S: ClientSource + ?Sized,
    F: Fn(Crabrave) -> Fut,
    Fut: Future<Output = Result<T, CrabError>>,
{
    let mut transient_attempts: u32 = 0;
    let mut rate_limit_waits: u32 = 0;
    // A 401 earns exactly one renewal attempt: the recorded expiry can lag
    // reality (clock skew, a token revoked server-side), but a token that is
    // refused right after being issued is a genuine authorization failure.
    let mut renewed_on_unauthorized = false;

    loop {
        // Checked before every attempt, not just after a sleep: the token can
        // also lapse during a long media batch between two page fetches.
        // A legacy token file with no refresh token gets no renewal; the
        // request goes out anyway and its 401, if any, is reported as such.
        if source.is_expired() {
            log::info!("access token expired; renewing before the next request");
            renew(source, "that expired").await?;
        }

        let error: anyhow::Error = match tokio::time::timeout(REQUEST_TIMEOUT, op(source.client()))
            .await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(e @ CrabError::Api { status: 401, .. })) if !renewed_on_unauthorized => {
                renewed_on_unauthorized = true;
                log::info!("request was refused as unauthorized; renewing the access token once");
                if renew(source, "after an unauthorized response").await? {
                    continue;
                }
                return Err(e.into());
            }
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
    use std::cell::Cell;

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

    /// A credential source whose token the test can expire at will; `refresh`
    /// un-expires it and counts.
    struct FakeSource {
        client: Crabrave,
        expired: Cell<bool>,
        refreshes: Cell<u32>,
        can_renew: bool,
    }

    impl FakeSource {
        fn new() -> Self {
            let client = Crabrave::builder()
                .consumer_key("key".to_owned())
                .access_token("token")
                .build()
                .unwrap();
            Self {
                client,
                expired: Cell::new(false),
                refreshes: Cell::new(0),
                can_renew: true,
            }
        }

        /// A legacy token file: no refresh token was ever saved.
        fn without_refresh_token() -> Self {
            Self {
                can_renew: false,
                ..Self::new()
            }
        }

        fn refreshed(&self) -> bool {
            self.refreshes.get() > 0
        }
    }

    impl ClientSource for FakeSource {
        fn client(&self) -> Crabrave {
            self.client.clone()
        }

        fn is_expired(&self) -> bool {
            self.expired.get()
        }

        fn refresh(&self) -> impl Future<Output = anyhow::Result<bool>> + Send {
            if !self.can_renew {
                return std::future::ready(Ok(false));
            }
            self.refreshes.set(self.refreshes.get() + 1);
            self.expired.set(false);
            std::future::ready(Ok(true))
        }
    }

    fn unauthorized() -> CrabError {
        CrabError::Api {
            status: 401,
            message: "Unauthorized".to_owned(),
            errors: Vec::new(),
        }
    }

    #[tokio::test]
    async fn expired_token_is_renewed_after_a_rate_limit_sleep_before_retrying() {
        let source = FakeSource::new();
        let attempts = Cell::new(0u32);
        let result: anyhow::Result<u32> = send_with_retries(
            &source,
            |_client| {
                attempts.set(attempts.get() + 1);
                let outcome = if attempts.get() == 1 {
                    // The token outlives the request but not the sleep.
                    source.expired.set(true);
                    Err(CrabError::RateLimit {
                        retry_after: Some(0),
                    })
                } else if source.refreshed() {
                    Ok(7)
                } else {
                    Err(unauthorized())
                };
                std::future::ready(outcome)
            },
            true,
        )
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(attempts.get(), 2);
        assert_eq!(source.refreshes.get(), 1);
    }

    #[tokio::test]
    async fn unauthorized_response_forces_one_renewal_then_retries() {
        let source = FakeSource::new();
        let attempts = Cell::new(0u32);
        let result: anyhow::Result<u32> = send_with_retries(
            &source,
            |_client| {
                attempts.set(attempts.get() + 1);
                let outcome = if source.refreshed() {
                    Ok(7)
                } else {
                    Err(unauthorized())
                };
                std::future::ready(outcome)
            },
            true,
        )
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(attempts.get(), 2);
        assert_eq!(source.refreshes.get(), 1);
    }

    #[tokio::test]
    async fn unauthorized_after_renewal_is_fatal() {
        let source = FakeSource::new();
        let attempts = Cell::new(0u32);
        let result: anyhow::Result<u32> = send_with_retries(
            &source,
            |_client| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err(unauthorized()))
            },
            true,
        )
        .await;
        let err = result.unwrap_err();
        assert!(is_client_api_error(&err), "got: {err:#}");
        assert_eq!(attempts.get(), 2);
        assert_eq!(source.refreshes.get(), 1);
    }

    #[tokio::test]
    async fn unauthorized_without_a_refresh_token_is_returned_as_is() {
        let source = FakeSource::without_refresh_token();
        let attempts = Cell::new(0u32);
        let result: anyhow::Result<u32> = send_with_retries(
            &source,
            |_client| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err(unauthorized()))
            },
            true,
        )
        .await;
        // Callers that skip per-item 4xx refusals must still see the 401.
        assert!(is_client_api_error(&result.unwrap_err()));
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test]
    async fn expired_token_without_a_refresh_token_still_sends_the_request() {
        let source = FakeSource::without_refresh_token();
        source.expired.set(true);
        let attempts = Cell::new(0u32);
        let result: anyhow::Result<u32> = send_with_retries(
            &source,
            |_client| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Ok(7))
            },
            true,
        )
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn parse_error_classification() {
        let serde_err = serde_json::from_str::<u32>("<html>").unwrap_err();
        let e = anyhow::Error::new(CrabError::Serialization(serde_err)).context("giving up");
        assert!(is_parse_error(&e));
        assert!(!is_parse_error(&api_error(404)));
    }
}
