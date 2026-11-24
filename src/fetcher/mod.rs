//! Web fetching policy: one shared connection pool, one polite clock per host, and the
//! speculative prefetch that fills the cache with the chapters the reader is about to reach.
//!
//! This module owns *policy*, not parsing: it never touches `ArticleDoc` or the DOM. The caller
//! hands in a URL and gets bytes back (or a wait, or a cached answer), which keeps the article
//! extraction in `article.rs` exactly where it is.
//!
//! The layers, from the outside in:
//!
//! - [`domain`] — the per-host clock. Never more than N requests in flight to one host, never
//!   less than the current gap between them, and a gap that widens on 429 and narrows again on
//!   a run of successes.
//! - [`backoff`] — the retry schedule: exponential with a capped, deterministic jitter, and
//!   `Retry-After` honoured when the server sends one.
//! - [`prefetch`] — which chapters to fetch next, in what order, and which to skip because they
//!   are already cached or already queued.
//! - [`http`] — the single `ureq` agent, the pool, and the retry loop that ties the three
//!   above together.
//! - [`worker`] — the one background thread that turns planned chapters into cached ones.
//!
//! Wiring it into the app (`mod fetcher;` in `main.rs`, a `Prefetcher` in `State`) is outside
//! this module's scope, so until then the files are inert and the crate builds as before. They
//! are `pub` with nothing calling them yet, so a binary build would warn about every item; the
//! allow goes away with the first real caller, not before.
#![allow(dead_code)]

pub mod backoff;
pub mod domain;
pub mod http;
pub mod prefetch;
pub mod worker;

/// The host of an absolute or scheme-less URL, lowercased, without a port or a leading `www.`.
///
/// Hand-rolled rather than pulled from a URL crate: the only thing this module needs is the
/// key a rate limit hangs off, and `Host` equality is that key. Returns `None` when there is no
/// authority to speak of, which is a URL the fetcher cannot rank anyway.
pub fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    // Userinfo is not part of the host, and a bare `example.com:8080` keeps the host.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = authority.split(':').next()?.trim();
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn the_host_is_the_rate_limit_key() {
        assert_eq!(host("https://Example.COM/a/b?c=d").as_deref(), Some("example.com"));
        assert_eq!(host("http://www.example.com").as_deref(), Some("example.com"));
        assert_eq!(host("example.com:8443/x").as_deref(), Some("example.com"));
        assert_eq!(host("https://user:pw@example.com/x").as_deref(), Some("example.com"));
        assert_eq!(host("https://example.com./x").as_deref(), Some("example.com"));
    }

    #[test]
    fn a_search_query_is_not_a_host() {
        assert_eq!(host("quiet page"), None);
        assert_eq!(host(""), None);
        assert_eq!(host("https://"), None);
    }
}
