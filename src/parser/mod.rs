//! Content extraction and per-site cleanup.
//!
//! Pieces, each usable on its own:
//!
//! * [`profile`] reads `profiles/*.toml`, validates it, and compiles the patterns and selectors.
//! * [`sanitize`] applies a profile's rules to the extracted blocks.
//! * [`fallback`] reads a page heuristically when there is no profile and readability gives up.
//! * [`pattern`] is the small wildcard matcher the rules are written in.
//! * [`next`] finds the next chapter's link, for the prefetcher.
//!
//! The module is written as a small library: some of its surface (the accessors, the pattern
//! helpers, [`ProfileSet::from_sources`]) is exercised by the tests and by callers that land
//! later, such as the prefetcher, so dead-code warnings would report work in progress the
//! compiler cannot see rather than anything to remove.
#![allow(dead_code)]

pub mod fallback;
pub mod next;
pub mod pattern;
pub mod profile;
pub mod sanitize;

pub use next::next_url;
pub use profile::{Profile, ProfileSet};

/// The profile set for this process, loaded once from the `profiles/` directory.
///
/// Loading is done lazily so a page with no profile costs one filesystem pass at most, and a
/// failed load never fails a fetch: the reader falls back to plain readability.
pub fn profiles() -> &'static ProfileSet {
    use std::sync::OnceLock;
    static SET: OnceLock<ProfileSet> = OnceLock::new();
    SET.get_or_init(|| match profile::profiles_dir() {
        Some(dir) => ProfileSet::load_dir(&dir),
        None => ProfileSet::default(),
    })
}

/// The profile for `url`, if one is installed and matches its host.
pub fn profile_for(url: &str) -> Option<&'static Profile> {
    profiles().find(url)
}
