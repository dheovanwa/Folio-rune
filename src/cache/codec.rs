//! The byte transform between an entry's body and what lands on disk.
//!
//! [`Identity`] ships today: it copies. That sounds useless, but it is the seam that makes the
//! zstd step a one-line swap instead of a refactor, and it lets the cache be measured before the
//! compression lands — a cache whose read path is already exercised is a cache whose zstd bug
//! will be a zstd bug, not a cache bug.
//!
//! When `zstd` is approved for `Cargo.toml`, `Zstd` implements the same two methods over
//! `zstd::stream::encode_all` / `decode_all` at level 3 (the default sweet spot for HTML: it
//! halves a chapter in about a millisecond, where level 19 costs 100× for a few percent more).
//! Nothing above this file changes, because the entry header already records the encoded length
//! and the codec is chosen when the cache is opened, not when an entry is written.

/// A reversible byte transform.
pub trait Codec: Send + Sync {
    /// Compress (or pass through).
    fn encode(&self, body: &[u8]) -> Vec<u8>;
    /// Decompress. `None` means the bytes were not produced by this codec, which the cache
    /// treats as a miss rather than a panic.
    fn decode(&self, stored: &[u8]) -> Option<Vec<u8>>;
}

/// A codec that stores bytes as they are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Identity;

impl Codec for Identity {
    fn encode(&self, body: &[u8]) -> Vec<u8> {
        body.to_vec()
    }

    fn decode(&self, stored: &[u8]) -> Option<Vec<u8>> {
        Some(stored.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_lossless_for_arbitrary_bytes() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        assert_eq!(Identity.encode(&bytes), bytes);
        assert_eq!(Identity.decode(&bytes).as_deref(), Some(&bytes[..]));
    }

    #[test]
    fn identity_handles_the_empty_page() {
        assert_eq!(Identity.encode(&[]), Vec::<u8>::new());
        assert_eq!(Identity.decode(&[]), Some(Vec::new()));
    }
}
