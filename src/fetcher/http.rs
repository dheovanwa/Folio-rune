//! The HTTP client: one pool for the process, one polite clock per host, one retry schedule.
//!
//! This is the only file that touches `ureq`. Everything above it — the planner, the cache, the
//! article extractor — speaks in URLs and bytes, which is what makes the network policy testable
//! by inspection and replaceable by a fake.
//!
//! The request path is deliberately linear:
//!
//! ```text
//! acquire(host) ──wait──> sleep ──┐
//!      │ ready                    │
//!      ▼                          │
//!    GET ──> 200 ──> body ──> Success, return
//!      │        429 ──> Retry-After ──> RateLimited ──> backoff ──> retry
//!      │        5xx ──> Error ──> backoff ──> retry
//!      └─ 4xx other ──> give up, no retry: the page will not appear
//! ```
//!
//! Two decisions worth naming:
//!
//! - **One `Agent` for the whole process.** `ureq`'s agent owns the connection pool, so building
//!   one per request would pay a TLS handshake every time — which is both slower and *less*
//!   polite, because a fresh connection looks like a fresh client to the host's rate limiter.
//! - **A body cap.** A chapter is tens of kilobytes; a cap of 8 MiB refuses to buffer a video
//!   someone linked to by mistake, and gives the caller a clear error instead of an OOM.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ureq::ResponseExt;

use super::backoff::{parse_retry_after, Backoff};
use super::domain::{Acquired, DomainLimiter, Outcome};

/// How a fetch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Transport-level: DNS, TLS, timeout, connection reset.
    Network(String),
    /// The host answered, but not with a page we can use.
    Status(u16),
    /// The host kept saying 429 past the retry budget. The caller should back off for a while,
    /// not retry: this is the "stop asking" signal.
    RateLimited,
    /// The body was larger than [`Client::max_body`], so it was not buffered.
    TooLarge,
    /// The caller asked to stop before the request finished.
    Cancelled,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Network(e) => write!(f, "{e}"),
            Failure::Status(code) => write!(f, "The site answered with HTTP {code}."),
            Failure::RateLimited => {
                write!(f, "The site is asking Folio to slow down. Try again shortly.")
            }
            Failure::TooLarge => write!(f, "That page is too large to read."),
            Failure::Cancelled => write!(f, "Cancelled."),
        }
    }
}

/// A page that came back.
#[derive(Clone, Debug)]
pub struct Page {
    /// The URL after redirects, which is what relative links must resolve against.
    pub url: String,
    pub body: Vec<u8>,
}

impl Page {
    /// The body as text, which is what the extractor wants.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A cancellation flag a prefetch thread checks between requests. Cheap to clone, and shared with
/// the UI thread that decides the reader has moved on.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<std::sync::atomic::AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The shared HTTP client.
#[derive(Debug)]
pub struct Client {
    agent: ureq::Agent,
    limiter: Arc<DomainLimiter>,
    backoff: Backoff,
    /// The largest body Folio will buffer, in bytes.
    pub max_body: u64,
}

impl Client {
    /// Build the process-wide client.
    ///
    /// The pool is sized for the reader's actual pattern — the current chapter plus two ahead —
    /// rather than for a crawler: a handful of idle connections, a short idle age, and at most
    /// one request in flight per host by default.
    pub fn new(policy: super::domain::Policy, backoff: Backoff) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // 429 and 5xx must reach this module as statuses, not as opaque errors: the retry
            // decision needs the code.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .max_idle_connections(8)
            .max_idle_connections_per_host(2)
            .max_idle_age(Duration::from_secs(30))
            .user_agent(concat!("Folio/", env!("CARGO_PKG_VERSION"), " (Rune reader)"))
            .build()
            .into();
        Client { agent, limiter: DomainLimiter::new(policy), backoff, max_body: 8 << 20 }
    }

    /// A client with the module's default policy.
    pub fn with_defaults() -> Client {
        Client::new(super::domain::Policy::default(), Backoff::default())
    }

    /// The limiter, so the UI can show a host's current gap or reset it.
    pub fn limiter(&self) -> &Arc<DomainLimiter> {
        &self.limiter
    }

    /// Fetch `url`, honouring the host's rate limit and retrying the failures worth retrying.
    ///
    /// Blocking: call it from a worker thread. The UI thread's job is to poll a slot, not to sit
    /// in a socket read.
    pub fn get(&self, url: &str, cancel: &Cancel) -> Result<Page, Failure> {
        let host = super::host(url).unwrap_or_default();
        let seed = seed_of(url);
        let mut attempt: u32 = 1;

        loop {
            if cancel.is_cancelled() {
                return Err(Failure::Cancelled);
            }

            // Wait for this host's turn. The wait is bounded by the host's own policy, so a
            // blocked host delays only its own requests.
            let slot = loop {
                match self.limiter.try_acquire(&host, Instant::now()) {
                    Acquired::Ready(slot) => break slot,
                    Acquired::Wait(d) => {
                        if cancel.is_cancelled() {
                            return Err(Failure::Cancelled);
                        }
                        // A zero wait means "a slot will free up", not "poll as fast as you can".
                        if !self.sleep_cancellable(d.max(Duration::from_millis(5)), cancel) {
                            return Err(Failure::Cancelled);
                        }
                    }
                }
            };

            match self.request(url) {
                Ok(page) => {
                    slot.report(Outcome::Success, Instant::now());
                    return Ok(page);
                }
                Err(RequestError::Status(429, retry_after)) => {
                    // The server told us to stop. Widen this host's gap and retry once the
                    // schedule says so — or once `Retry-After` says so, whichever is longer.
                    slot.report(Outcome::RateLimited { retry_after }, Instant::now());
                    if !self.backoff.may_retry(attempt) {
                        return Err(Failure::RateLimited);
                    }
                    let wait = self.backoff.delay(attempt + 1, seed, retry_after);
                    if !self.sleep_cancellable(wait, cancel) {
                        return Err(Failure::Cancelled);
                    }
                    attempt += 1;
                }
                Err(RequestError::Status(code, _)) if code >= 500 => {
                    slot.report(Outcome::Error, Instant::now());
                    if !self.backoff.may_retry(attempt) {
                        return Err(Failure::Status(code));
                    }
                    let wait = self.backoff.delay(attempt + 1, seed, None);
                    if !self.sleep_cancellable(wait, cancel) {
                        return Err(Failure::Cancelled);
                    }
                    attempt += 1;
                }
                Err(RequestError::Status(code, _)) => {
                    // A 404 will still be a 404 in a second. Give up immediately.
                    slot.report(Outcome::Error, Instant::now());
                    return Err(Failure::Status(code));
                }
                Err(RequestError::Network(e)) => {
                    slot.report(Outcome::Error, Instant::now());
                    if !self.backoff.may_retry(attempt) {
                        return Err(Failure::Network(e));
                    }
                    let wait = self.backoff.delay(attempt + 1, seed, None);
                    if !self.sleep_cancellable(wait, cancel) {
                        return Err(Failure::Cancelled);
                    }
                    attempt += 1;
                }
                Err(RequestError::TooLarge) => {
                    slot.report(Outcome::Error, Instant::now());
                    return Err(Failure::TooLarge);
                }
            }
        }
    }

    /// One request, no policy. Returns the page, or enough information for `get` to decide.
    fn request(&self, url: &str) -> Result<Page, RequestError> {
        let mut response = self.agent.get(url).header("Accept", "text/html,application/xhtml+xml").call().map_err(
            |e| match e {
                ureq::Error::StatusCode(code) => RequestError::Status(code, None),
                other => RequestError::Network(other.to_string()),
            },
        )?;

        let status = response.status().as_u16();
        if status != 200 {
            // The header is read before the response is dropped, so the retry schedule can use
            // the server's own advice on the 429 path.
            return Err(RequestError::Status(status, retry_after_of(&response)));
        }

        let final_url = response.get_uri().to_string();

        let body = match response.body_mut().with_config().limit(self.max_body).read_to_vec() {
            Ok(body) => body,
            Err(ureq::Error::BodyExceedsLimit(_)) => return Err(RequestError::TooLarge),
            Err(e) => return Err(RequestError::Network(e.to_string())),
        };

        Ok(Page { url: final_url, body })
    }

    /// Sleep, but wake early when the caller cancels. Returns false if cancelled.
    fn sleep_cancellable(&self, total: Duration, cancel: &Cancel) -> bool {
        let deadline = Instant::now() + total;
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            // Sleep in short hops so a cancel does not have to wait out a 20-second backoff.
            std::thread::sleep((deadline - now).min(Duration::from_millis(100)));
        }
    }
}

enum RequestError {
    Status(u16, Option<Duration>),
    Network(String),
    TooLarge,
}

/// A stable seed for the jitter: the same URL always retries on the same schedule, so a bug
/// report is reproducible and two chapters still drift apart.
fn seed_of(url: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Read `Retry-After` off a response, for the 429 path.
///
/// Kept public because a caller that does its own `ureq` call — a test, or a future feed reader —
/// should read the header the same way.
pub fn retry_after_of(response: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
    let value = response.headers().get("retry-after")?.to_str().ok()?;
    parse_retry_after(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn the_same_url_always_gets_the_same_retry_seed() {
        assert_eq!(seed_of("https://example.com/a"), seed_of("https://example.com/a"));
        assert_ne!(seed_of("https://example.com/a"), seed_of("https://example.com/b"));
    }

    #[test]
    fn a_cancelled_client_stops_before_the_first_request() {
        let c = Client::with_defaults();
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(c.get("https://example.com/never", &cancel).unwrap_err(), Failure::Cancelled);
    }

    #[test]
    fn cancellation_interrupts_a_backoff_sleep() {
        let c = Client::with_defaults();
        let cancel = Cancel::new();
        let handle = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                // 30 seconds of "backoff", cancelled after 50ms, must return quickly.
                std::thread::sleep(Duration::from_millis(50));
                cancel.cancel();
            })
        };
        let started = Instant::now();
        assert!(!c.sleep_cancellable(Duration::from_secs(30), &cancel));
        assert!(started.elapsed() < Duration::from_secs(5), "woke up promptly");
        handle.join().unwrap();
    }

    /// A tiny HTTP/1.1 server that answers each request with the next scripted status.
    ///
    /// Written against `std::net` rather than pulling in a test server: the point is to prove the
    /// retry loop against a real socket, and one `TcpListener` is the whole dependency.
    fn scripted_server(script: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<Vec<Instant>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in script {
                let (mut stream, _) = listener.accept().unwrap();
                seen.push(Instant::now());
                // Read the request head; the client sends it in one write.
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let reason = if status == 200 { "OK" } else { "Too Many Requests" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
            seen
        });
        (format!("http://{}", addr), handle)
    }

    #[test]
    fn a_429_is_retried_and_the_retries_are_spaced_out() {
        let (url, server) = scripted_server(vec![
            (429, "slow down"),
            (429, "slow down"),
            (200, "<html><body>the chapter</body></html>"),
        ]);

        let policy = super::super::domain::Policy {
            base_gap: Duration::from_millis(60),
            min_gap: Duration::from_millis(30),
            max_gap: Duration::from_secs(1),
            max_in_flight: 1,
            widen_percent: 200,
            narrow_after: 3,
        };
        // Jitter off, so the schedule is exactly 40ms then 80ms: the test is about spacing, not
        // about the mixer.
        let backoff = Backoff {
            attempts: 4,
            base: Duration::from_millis(40),
            cap: Duration::from_secs(1),
            jitter_percent: 0,
        };
        let client = Client::new(policy, backoff);

        let page = client.get(&url, &Cancel::new()).expect("the third attempt succeeds");
        assert_eq!(page.text(), "<html><body>the chapter</body></html>");

        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 3, "two 429s then the page");

        // Each retry waited: the host's gap widened on the 429, so no two requests were back to
        // back. This is the behaviour that keeps a rate limiter from banning the client.
        for pair in seen.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(gap >= Duration::from_millis(30), "requests were {}ms apart", gap.as_millis());
        }
    }

    #[test]
    fn a_404_is_not_retried() {
        let (url, server) = scripted_server(vec![(404, "gone")]);
        let client = Client::with_defaults();
        let error = client.get(&url, &Cancel::new()).unwrap_err();
        assert_eq!(error, Failure::Status(404));
        // The server thread is still waiting for a second request that must never come, so it
        // is dropped rather than joined: if the client had retried, the assert above would have
        // failed with a timeout instead.
        drop(server);
    }

    #[test]
    fn a_host_that_never_stops_saying_429_gives_up_with_a_clear_error() {
        let (url, _server) = scripted_server(vec![(429, "no"), (429, "no"), (429, "no")]);
        let backoff = Backoff {
            attempts: 3,
            base: Duration::from_millis(10),
            cap: Duration::from_millis(20),
            jitter_percent: 0,
        };
        let policy = super::super::domain::Policy {
            base_gap: Duration::from_millis(5),
            min_gap: Duration::from_millis(1),
            max_gap: Duration::from_millis(50),
            max_in_flight: 1,
            widen_percent: 200,
            narrow_after: 3,
        };
        let client = Client::new(policy, backoff);
        assert_eq!(client.get(&url, &Cancel::new()).unwrap_err(), Failure::RateLimited);
    }
}
