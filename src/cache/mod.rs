//! The local cache: what has already been fetched, so a chapter can open instantly and a
//! prefetch is not wasted on a page the reader has already read.
//!
//! Layering, so the storage choice stays a detail:
//!
//! - [`Cache`] is the front door. It maps a URL to an [`Entry`], enforces freshness, and owns the
//!   size budget. Callers never see a file or a row.
//! - [`store::Store`] is the durable side. [`store::FileStore`] is what runs today — one file per
//!   entry, written to a temporary name and renamed, so a crash mid-write cannot leave a torn
//!   entry. A SQLite store implements the same trait once `rusqlite` is approved, and nothing
//!   above this line changes.
//! - [`codec::Codec`] is the byte transform. [`codec::Identity`] ships now; a zstd codec slots in
//!   at the same seam when the dependency lands.
//!
//! The entry format is deliberately hand-rolled rather than serde: it is a fixed header and two
//! length-prefixed fields, and a format that fits in a paragraph is a format a future version can
//! read back without a decoder library.
//!
//! Like the fetcher, nothing calls this yet — `main.rs` is outside this module's scope — so the
//! items are `pub` but unused in a binary build. The allow goes away with the first caller.
#![allow(dead_code)]

pub mod codec;
pub mod store;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codec::Codec;
use store::Store;

/// What kind of page an entry holds. Decides the default freshness: a chapter never changes,
/// a search result page changes every hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A chapter or article. Effectively immutable.
    Article,
    /// A listing, search or index page.
    Listing,
}

impl Kind {
    /// How long an entry of this kind is worth trusting.
    pub fn max_age(self) -> Duration {
        match self {
            Kind::Article => Duration::from_secs(60 * 60 * 24 * 30),
            Kind::Listing => Duration::from_secs(60 * 60),
        }
    }
}

/// A cached page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub kind: Kind,
    /// The page bytes, already decoded.
    pub body: Vec<u8>,
    /// Seconds since the Unix epoch, when it was fetched.
    pub fetched_at: u64,
}

impl Entry {
    pub fn new(url: impl Into<String>, kind: Kind, body: Vec<u8>) -> Entry {
        Entry { url: url.into(), kind, body, fetched_at: now_secs() }
    }

    /// Whether this entry is still worth serving at `now`.
    pub fn is_fresh(&self, now: u64) -> bool {
        now.saturating_sub(self.fetched_at) < self.kind.max_age().as_secs()
    }
}

/// The cache front door. One per process, shared behind an `Arc`.
pub struct Cache {
    store: Box<dyn Store>,
    codec: Box<dyn Codec>,
    /// URLs known to be on disk, with their timestamp and size. Kept in memory so the common
    /// "is this already cached?" question — asked for every prefetch candidate — costs a hash
    /// lookup instead of a stat call.
    index: Mutex<Index>,
    budget: u64,
}

struct Index {
    /// url -> what is on disk for it.
    known: std::collections::HashMap<String, Known>,
    total_bytes: u64,
}

/// What the in-memory index remembers about one cached URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Known {
    fetched_at: u64,
    size: u64,
    kind: Kind,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("budget", &self.budget).finish_non_exhaustive()
    }
}

impl Cache {
    /// Open a cache over `store`, keeping at most `budget` bytes.
    ///
    /// The index is rebuilt from the store once, at startup, so a cache from a previous run is
    /// usable without a migration step.
    pub fn open(store: Box<dyn Store>, codec: Box<dyn Codec>, budget: u64) -> Arc<Cache> {
        let known = store.list().unwrap_or_default();
        let mut total_bytes = 0;
        let mut map = std::collections::HashMap::with_capacity(known.len());
        for (url, fetched_at, size, kind) in known {
            total_bytes += size;
            map.insert(url, Known { fetched_at, size, kind });
        }
        Arc::new(Cache {
            store,
            codec,
            index: Mutex::new(Index { known: map, total_bytes }),
            budget,
        })
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a URL is cached and still fresh, on that entry's own kind. Cheap: no I/O.
    pub fn is_fresh(&self, url: &str, now: u64) -> bool {
        let index = self.index();
        match index.known.get(url) {
            Some(known) => {
                now.saturating_sub(known.fetched_at) < known.kind.max_age().as_secs()
            }
            None => false,
        }
    }

    /// Whether a URL is cached at all, fresh or stale. The prefetch planner uses this so a stale
    /// entry is refreshed rather than skipped forever.
    pub fn contains(&self, url: &str) -> bool {
        self.index().known.contains_key(url)
    }

    /// Read an entry, or `None` on a miss.
    pub fn get(&self, url: &str) -> Option<Entry> {
        let encoded = self.store.read(url).ok()??;
        let (kind, fetched_at, body) = decode_entry(&encoded)?;
        let body = self.codec.decode(&body)?;
        Some(Entry { url: url.to_string(), kind, body, fetched_at })
    }

    /// Store a page, evicting the oldest entries when the budget is exceeded.
    pub fn put(&self, entry: &Entry) -> std::io::Result<()> {
        let encoded = encode_entry(entry);
        self.store.write(&entry.url, &encoded)?;
        let size = encoded.len() as u64;

        let mut index = self.index();
        if let Some(old) = index.known.insert(
            entry.url.clone(),
            Known { fetched_at: entry.fetched_at, size, kind: entry.kind },
        ) {
            index.total_bytes = index.total_bytes.saturating_sub(old.size);
        }
        index.total_bytes += size;

        let over = index.total_bytes.saturating_sub(self.budget);
        if over == 0 {
            return Ok(());
        }
        // Evict oldest-first. The entry just written carries the newest timestamp, so it is
        // never the first victim.
        let mut victims: Vec<(u64, String, u64)> = index
            .known
            .iter()
            .filter(|(url, _)| url.as_str() != entry.url)
            .map(|(url, known)| (known.fetched_at, url.clone(), known.size))
            .collect();
        victims.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut freed = 0;
        for (_, url, size) in victims {
            if freed >= over {
                break;
            }
            let _ = self.store.remove(&url);
            index.known.remove(&url);
            index.total_bytes = index.total_bytes.saturating_sub(size);
            freed += size;
        }
        Ok(())
    }

    /// Total bytes the cache believes it holds.
    pub fn size(&self) -> u64 {
        self.index().total_bytes
    }

    /// Drop everything. Used when the format changes or the user asks for it.
    pub fn clear(&self) {
        let _ = self.store.clear();
        let mut index = self.index();
        index.known.clear();
        index.total_bytes = 0;
    }
}

/// On-disk layout: `FLC1`, a kind byte, `fetched_at` (8 bytes LE), then two length-prefixed
/// byte strings: the URL and the (codec-encoded) body.
pub fn encode_entry(entry: &Entry) -> Vec<u8> {
    let url = entry.url.as_bytes();
    let mut out = Vec::with_capacity(4 + 1 + 8 + 4 + url.len() + 4 + entry.body.len());
    out.extend_from_slice(b"FLC1");
    out.push(match entry.kind {
        Kind::Article => 0,
        Kind::Listing => 1,
    });
    out.extend_from_slice(&entry.fetched_at.to_le_bytes());
    out.extend_from_slice(&(url.len() as u32).to_le_bytes());
    out.extend_from_slice(url);
    out.extend_from_slice(&(entry.body.len() as u32).to_le_bytes());
    out.extend_from_slice(&entry.body);
    out
}

fn decode_entry(bytes: &[u8]) -> Option<(Kind, u64, Vec<u8>)> {
    if bytes.len() < 4 + 1 + 8 + 4 || &bytes[..4] != b"FLC1" {
        return None;
    }
    let mut at = 4;
    let kind = match bytes[at] {
        0 => Kind::Article,
        1 => Kind::Listing,
        _ => return None,
    };
    at += 1;
    let fetched_at = u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?);
    at += 8;
    let url_len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    bytes.get(at..at + url_len)?;
    at += url_len;
    let body_len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let body = bytes.get(at..at + body_len)?.to_vec();
    Some((kind, fetched_at, body))
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::codec::Identity;
    use crate::cache::store::MemoryStore;

    fn cache(budget: u64) -> Arc<Cache> {
        Cache::open(Box::new(MemoryStore::default()), Box::new(Identity), budget)
    }

    #[test]
    fn a_stored_page_comes_back_byte_for_byte() {
        let c = cache(1 << 20);
        let e = Entry::new("https://example.com/a", Kind::Article, b"<html>hi</html>".to_vec());
        c.put(&e).unwrap();
        let got = c.get("https://example.com/a").expect("stored");
        assert_eq!(got.body, b"<html>hi</html>");
        assert_eq!(got.kind, Kind::Article);
        assert_eq!(got.fetched_at, e.fetched_at);
    }

    #[test]
    fn an_unknown_url_is_a_miss_not_an_error() {
        let c = cache(1 << 20);
        assert!(c.get("https://example.com/nope").is_none());
        assert!(!c.contains("https://example.com/nope"));
    }

    #[test]
    fn an_article_stays_fresh_for_a_month_and_a_listing_for_an_hour() {
        let now = now_secs();
        let article = Entry { fetched_at: now, ..Entry::new("u", Kind::Article, vec![]) };
        let listing = Entry { fetched_at: now, ..Entry::new("v", Kind::Listing, vec![]) };
        assert!(article.is_fresh(now + 60 * 60 * 24));
        assert!(!listing.is_fresh(now + 60 * 60 * 2));
    }

    #[test]
    fn the_budget_evicts_the_oldest_first() {
        // Budgets are in encoded bytes, and every entry carries the header and its URL, so the
        // probe uses a URL of the same length as the ones stored.
        let one = encode_entry(&Entry::new("key", Kind::Article, vec![0u8; 100])).len() as u64;
        let c = cache(one * 2 + one / 2);

        let old = Entry { fetched_at: 1, ..Entry::new("old", Kind::Article, vec![0u8; 100]) };
        let mid = Entry { fetched_at: 2, ..Entry::new("mid", Kind::Article, vec![0u8; 100]) };
        c.put(&old).unwrap();
        c.put(&mid).unwrap();
        assert_eq!(c.size(), one * 2);

        let new = Entry { fetched_at: 3, ..Entry::new("new", Kind::Article, vec![0u8; 100]) };
        c.put(&new).unwrap();
        assert!(!c.contains("old"), "the oldest entry made room");
        assert!(c.contains("mid"));
        assert!(c.contains("new"));
        assert!(c.size() <= one * 2 + one / 2, "size was {}", c.size());
    }

    #[test]
    fn freshness_uses_the_entrys_own_kind() {
        let c = cache(1 << 20);
        let stale = now_secs().saturating_sub(60 * 60 * 2);
        c.put(&Entry { fetched_at: stale, ..Entry::new("listing", Kind::Listing, b"x".to_vec()) })
            .unwrap();
        c.put(&Entry { fetched_at: stale, ..Entry::new("article", Kind::Article, b"y".to_vec()) })
            .unwrap();
        let now = now_secs();
        assert!(!c.is_fresh("listing", now), "a listing from two hours ago is stale");
        assert!(c.is_fresh("article", now), "an article from two hours ago is fine");
        assert!(c.contains("listing"), "stale is not absent");
    }

    #[test]
    fn clearing_empties_both_the_store_and_the_index() {
        let c = cache(1 << 20);
        c.put(&Entry::new("u", Kind::Article, b"x".to_vec())).unwrap();
        c.clear();
        assert!(!c.contains("u"));
        assert_eq!(c.size(), 0);
    }

    #[test]
    fn a_torn_or_foreign_file_is_ignored_rather_than_trusted() {
        assert!(decode_entry(b"garbage").is_none());
        assert!(decode_entry(b"FLC1\x00").is_none());
    }
}
