//! Where cached bytes live.
//!
//! [`Store`] is the only thing [`super::Cache`] knows about storage: `read`, `write`, `remove`,
//! `list`, `clear`. Two implementations ship with this module:
//!
//! - [`MemoryStore`] — a `HashMap`. It is what the unit tests run on, and what a `--no-cache` or
//!   private-window mode would use.
//! - [`FileStore`] — one file per entry under a directory, named by a hash of the URL. Writes go
//!   to a temporary file in the same directory and are then renamed, so a crash cannot produce a
//!   half-written entry that later decodes into garbage. `rename` on the same filesystem is
//!   atomic on Linux, the only platform Folio runs on.
//!
//! A SQLite store is the plan for larger libraries: same trait, same key, one transaction per
//! write, and a `VACUUM` on a size trigger. It needs `rusqlite` in `Cargo.toml`, which is outside
//! this module's scope, so it is left as an impl to land once the dependency is approved.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A durable home for encoded entries.
pub trait Store: Send + Sync {
    /// Read the encoded bytes for `url`.
    fn read(&self, url: &str) -> io::Result<Option<Vec<u8>>>;
    /// Write the encoded bytes for `url`, replacing any previous value.
    fn write(&self, url: &str, bytes: &[u8]) -> io::Result<()>;
    /// Forget `url`. Missing is not an error.
    fn remove(&self, url: &str) -> io::Result<()>;
    /// Every entry as `(url, fetched_at, encoded size, kind)`, for rebuilding the index.
    fn list(&self) -> io::Result<Vec<(String, u64, u64, super::Kind)>>;
    /// Drop everything.
    fn clear(&self) -> io::Result<()>;
}

/// A store that forgets when the process ends.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Vec<u8>>>,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStore").finish_non_exhaustive()
    }
}

impl MemoryStore {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Store for MemoryStore {
    fn read(&self, url: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(self.entries().get(url).cloned())
    }

    fn write(&self, url: &str, bytes: &[u8]) -> io::Result<()> {
        self.entries().insert(url.to_string(), bytes.to_vec());
        Ok(())
    }

    fn remove(&self, url: &str) -> io::Result<()> {
        self.entries().remove(url);
        Ok(())
    }

    fn list(&self) -> io::Result<Vec<(String, u64, u64, super::Kind)>> {
        Ok(self
            .entries()
            .iter()
            .filter_map(|(url, bytes)| {
                // Reuse the shared header reader rather than duplicating the layout.
                super::decode_entry(bytes).map(|(kind, fetched_at, _)| {
                    (url.clone(), fetched_at, bytes.len() as u64, kind)
                })
            })
            .collect())
    }

    fn clear(&self) -> io::Result<()> {
        self.entries().clear();
        Ok(())
    }
}

/// One file per entry, under a directory Folio owns.
#[derive(Debug)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Open a store rooted at `dir`, creating it if needed.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<FileStore> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(FileStore { dir })
    }

    /// A file name for a URL: FNV-1a over the whole URL, hex-encoded. The URL itself is in the
    /// entry header, so a collision would be caught by the URL check on read.
    fn file_for(&self, url: &str) -> PathBuf {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in url.as_bytes() {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.dir.join(format!("{h:016x}.flc"))
    }

    fn list_files(&self) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let read = match std::fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in read {
            let entry = entry?;
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("flc") => out.push(path),
                // A leftover temporary from a crash mid-write. It was never renamed into place,
                // so it is safe to drop.
                Some("tmp") => {
                    let _ = std::fs::remove_file(&path);
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

impl Store for FileStore {
    fn read(&self, url: &str) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.file_for(url)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write(&self, url: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.file_for(url);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        // The rename is the commit: until it happens, readers see the old entry or none.
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    fn remove(&self, url: &str) -> io::Result<()> {
        match std::fs::remove_file(self.file_for(url)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn list(&self) -> io::Result<Vec<(String, u64, u64, super::Kind)>> {
        let mut out = Vec::new();
        for path in self.list_files()? {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            match super::decode_entry(&bytes).and_then(|(kind, fetched_at, _)| {
                url_from_entry(&bytes).map(|url| (url, fetched_at, bytes.len() as u64, kind))
            }) {
                Some(row) => out.push(row),
                None => {
                    // A file that does not decode was written by another version, or truncated
                    // outside our control. Drop it rather than rescanning it every startup.
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(out)
    }

    fn clear(&self) -> io::Result<()> {
        for path in self.list_files()? {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }
}

/// Read the URL field out of an encoded entry without a full decode.
fn url_from_entry(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 + 1 + 8 + 4 || &bytes[..4] != b"FLC1" {
        return None;
    }
    let at = 4 + 1 + 8;
    let url_len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
    let url = bytes.get(at + 4..at + 4 + url_len)?;
    String::from_utf8(url.to_vec()).ok()
}

/// The cache directory Folio owns, alongside the config and library files.
pub fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("folio"))
}

/// Open the on-disk cache Folio uses in normal runs.
pub fn default_store() -> Option<FileStore> {
    FileStore::open(cache_dir()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("folio-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_memory_store_round_trips() {
        let s = MemoryStore::default();
        s.write("u", b"hello").unwrap();
        assert_eq!(s.read("u").unwrap(), Some(b"hello".to_vec()));
        s.remove("u").unwrap();
        assert_eq!(s.read("u").unwrap(), None);
    }

    #[test]
    fn the_file_store_round_trips_and_lists_the_url() {
        let dir = tempdir("roundtrip");
        let s = FileStore::open(&dir).unwrap();
        let entry = super::super::Entry::new(
            "https://example.com/x",
            super::super::Kind::Article,
            b"body".to_vec(),
        );
        s.write(&entry.url, &super::super::encode_entry(&entry)).unwrap();
        assert!(s.read(&entry.url).unwrap().is_some());
        let listed = s.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "https://example.com/x");
        assert_eq!(listed[0].3, super::super::Kind::Article, "the kind survives a restart");
        s.clear().unwrap();
        assert!(s.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leftover_temporary_file_is_swept_not_read() {
        let dir = tempdir("tmp");
        let s = FileStore::open(&dir).unwrap();
        std::fs::write(dir.join("deadbeef.tmp"), b"half a write").unwrap();
        assert!(s.list().unwrap().is_empty());
        assert!(!dir.join("deadbeef.tmp").exists(), "the temporary was cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_urls_do_not_share_a_file() {
        let dir = tempdir("distinct");
        let s = FileStore::open(&dir).unwrap();
        assert_ne!(s.file_for("https://a.example/1"), s.file_for("https://a.example/2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
