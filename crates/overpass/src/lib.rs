//! Overpass API over HTTP, shared by `tilebake` (blocks of the bake) and `worldmap` (streets and
//! neighborhoods on demand). One request at a time is the caller's job; this crate gives the
//! client trait, the real HTTP client, and the retry with doubling backoff on 429/502/503/504.
use std::io;
use std::time::Duration;

/// Overpass interpreter endpoint.
pub const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
/// Environment variable that replaces the endpoint (checks use an unreachable host to simulate a
/// machine without network). Unset or empty: `OVERPASS_URL`.
pub const OVERPASS_URL_ENV: &str = "DTE_OVERPASS_URL";
/// User-Agent of every network request of the project (Overpass, Nominatim, Copernicus): game
/// name, workspace version and repository URL. `tilebake` uses this same value, since it depends
/// on this crate.
pub const USER_AGENT: &str = concat!(
    "driving-the-earth/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/faelcorreia/driving-the-earth)"
);
/// Minimum gap between the start of two Overpass requests (callers pace with it).
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// Attempts per request (the first try included) on 429, 502, 503 or 504.
pub const MAX_ATTEMPTS: u32 = 6;
/// Wait before the first retry; doubled on each following retry.
pub const BACKOFF_BASE: Duration = Duration::from_secs(5);

/// The endpoint in use: `DTE_OVERPASS_URL` when set and not empty, else `OVERPASS_URL`.
pub fn overpass_url() -> String {
    std::env::var(OVERPASS_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| OVERPASS_URL.to_string())
}

fn other_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

/// Failed HTTP request: the status when the server answered, `None` for transport errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    pub status: Option<u16>,
    pub message: String,
}

/// Sends an Overpass query (the `data` form field) and returns the response body.
pub trait OverpassClient {
    fn post(&self, query: &str) -> Result<String, HttpError>;
}

/// Overpass over HTTP, with the project User-Agent.
pub struct UreqOverpass;

impl OverpassClient for UreqOverpass {
    fn post(&self, query: &str) -> Result<String, HttpError> {
        match ureq::post(&overpass_url())
            .set("User-Agent", USER_AGENT)
            .send_form(&[("data", query)])
        {
            Ok(resp) => resp.into_string().map_err(|e| HttpError {
                status: None,
                message: e.to_string(),
            }),
            Err(ureq::Error::Status(code, _)) => Err(HttpError {
                status: Some(code),
                message: format!("HTTP {code}"),
            }),
            Err(e) => Err(HttpError {
                status: None,
                message: e.to_string(),
            }),
        }
    }
}

/// How blocks are fetched: client, pacing and retry. `sleep` is injected so tests do not wait.
pub struct Downloader<'a> {
    pub client: &'a dyn OverpassClient,
    pub sleep: &'a dyn Fn(Duration),
    pub min_gap: Duration,
    pub max_attempts: u32,
    pub backoff_base: Duration,
}

impl Downloader<'_> {
    /// Posts `query`, retrying with doubling backoff on 429, 502, 503 and 504. Other errors fail at once.
    pub fn post_with_retry(&self, query: &str) -> io::Result<String> {
        self.post_with_retry_counted(query).map(|(body, _)| body)
    }

    /// Like `post_with_retry`, and also returns how many retries were needed.
    pub fn post_with_retry_counted(&self, query: &str) -> io::Result<(String, u32)> {
        let mut delay = self.backoff_base;
        let mut retries = 0;
        for attempt in 1..=self.max_attempts {
            match self.client.post(query) {
                Ok(body) => return Ok((body, retries)),
                Err(e)
                    if matches!(e.status, Some(429 | 502 | 503 | 504))
                        && attempt < self.max_attempts =>
                {
                    eprintln!(
                        "tilebake: Overpass answered {} (attempt {attempt}/{}), retrying in {delay:?}",
                        e.message, self.max_attempts
                    );
                    (self.sleep)(delay);
                    delay *= 2;
                    retries += 1;
                }
                Err(e) => return Err(other_err(format!("Overpass request failed: {}", e.message))),
            }
        }
        Err(other_err("Overpass: max_attempts is zero"))
    }
}

#[cfg(test)]
mod tests {
    use super::{overpass_url, OVERPASS_URL, OVERPASS_URL_ENV, USER_AGENT};

    #[test]
    fn user_agent_names_the_game_version_and_repository() {
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            USER_AGENT,
            format!(
                "driving-the-earth/{version} (+https://github.com/faelcorreia/driving-the-earth)"
            )
        );
        assert!(!USER_AGENT.contains("projeto pessoal"));
    }

    #[test]
    fn endpoint_comes_from_the_environment_when_set() {
        // Only this test touches the variable, so the other tests are not affected.
        std::env::remove_var(OVERPASS_URL_ENV);
        assert_eq!(overpass_url(), OVERPASS_URL);
        std::env::set_var(OVERPASS_URL_ENV, "  ");
        assert_eq!(overpass_url(), OVERPASS_URL);
        std::env::set_var(OVERPASS_URL_ENV, "https://overpass.invalid/api/interpreter");
        assert_eq!(overpass_url(), "https://overpass.invalid/api/interpreter");
        std::env::remove_var(OVERPASS_URL_ENV);
    }
}
