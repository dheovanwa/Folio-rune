#[path = "../src/cache/mod.rs"]
pub mod cache;
#[path = "../src/fetcher/mod.rs"]
pub mod fetcher;

use cache::codec::Identity;
use cache::store::MemoryStore;
use cache::{Cache, Entry, Kind};
use std::sync::Arc;

#[test]
fn test_cache_and_fetcher_integration_smoke() {
    let store = Box::new(MemoryStore::default());
    let codec = Box::new(Identity);
    let cache: Arc<Cache> = Cache::open(store, codec, 1024 * 1024);

    let url = "https://example.com/test-page";
    assert_eq!(cache.get(url), None);

    let body = b"<html><body>Cached content</body></html>".to_vec();
    let entry = Entry::new(url, Kind::Article, body.clone());
    cache.put(&entry).expect("put should succeed");

    let retrieved = cache.get(url).expect("entry should be cached");
    assert_eq!(retrieved.body, body);
    assert_eq!(retrieved.kind, Kind::Article);
    assert!(cache.contains(url));
}
