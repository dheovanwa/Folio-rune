//! Where the reader stopped in each article, down to the paragraph.
//!
//! The library already keeps a `0..1` progress fraction, which is enough for a bar and useless
//! for returning to a place: the same fraction of a different window size lands mid-sentence.
//! This keeps the block index instead, so reopening an article reveals that paragraph by key
//! (`scroll_into_view`) rather than guessing an offset in points.
//!
//! One JSON file beside the library, bounded so a reader with thousands of articles does not
//! grow it without end.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A place in one article.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// The block the reader had at the top of the viewport.
    pub block: usize,
    /// The scroll offset in points when the mark was taken, for the progress bar and for
    /// diagnostics. Restoring uses `block`, because points depend on the window size.
    pub offset: f32,
    /// Reading progress 0..1 at the time, which the library row shows.
    pub frac: f32,
    /// Seconds since the epoch, for pruning the oldest marks first.
    pub at: u64,
}

/// Every article's last place, keyed by URL.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Positions {
    #[serde(default)]
    marks: BTreeMap<String, Mark>,
}

/// How many articles keep a place. Beyond this the oldest marks go, so the file stays small.
pub const KEEP: usize = 512;

impl Positions {
    pub fn get(&self, url: &str) -> Option<Mark> {
        self.marks.get(url).copied()
    }

    /// Record `mark` for `url`, dropping the oldest entries past [`KEEP`].
    ///
    /// The timestamp is taken here rather than passed in, so every caller stamps the same way.
    pub fn remember(&mut self, url: &str, block: usize, offset: f32, frac: f32) {
        if url.is_empty() {
            return;
        }
        self.marks.insert(
            url.to_string(),
            Mark {
                block,
                offset: offset.max(0.0),
                frac: frac.clamp(0.0, 1.0),
                at: crate::library::now_secs(),
            },
        );
        self.prune();
    }

    pub fn forget(&mut self, url: &str) {
        self.marks.remove(url);
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Drop the oldest marks until [`KEEP`] remain. Cheap: it only runs when a mark is added.
    fn prune(&mut self) {
        if self.marks.len() <= KEEP {
            return;
        }
        let mut by_age: Vec<(u64, String)> =
            self.marks.iter().map(|(url, m)| (m.at, url.clone())).collect();
        by_age.sort_unstable();
        let drop = self.marks.len() - KEEP;
        for (_, url) in by_age.into_iter().take(drop) {
            self.marks.remove(&url);
        }
    }

    pub fn path() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("folio").join("positions.json"))
    }

    /// Missing or unreadable marks mean "no saved place", never a failed start.
    pub fn load() -> Positions {
        let Some(path) = Self::path() else { return Positions::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Positions::default() };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_round_trips_and_clamps() {
        let mut p = Positions::default();
        p.remember("https://example.com/a", 12, 840.5, 1.7);
        let m = p.get("https://example.com/a").expect("mark saved");
        assert_eq!(m.block, 12);
        assert!((m.offset - 840.5).abs() < 0.01);
        assert_eq!(m.frac, 1.0, "progress is clamped to 0..1");
        assert!(p.get("https://example.com/none").is_none());
    }

    #[test]
    fn an_empty_url_is_not_recorded() {
        let mut p = Positions::default();
        p.remember("", 3, 10.0, 0.1);
        assert!(p.is_empty());
    }

    #[test]
    fn pruning_keeps_the_newest_marks() {
        let mut p = Positions::default();
        for i in 0..(KEEP + 8) {
            // Equal timestamps are the common case in a test run: the order must stay stable.
            p.marks.insert(
                format!("https://example.com/{i}"),
                Mark { block: i, offset: 0.0, frac: 0.0, at: i as u64 },
            );
        }
        p.prune();
        assert_eq!(p.len(), KEEP, "the file is bounded");
        assert!(p.get("https://example.com/0").is_none(), "the oldest went");
        assert!(p.get(&format!("https://example.com/{}", KEEP + 7)).is_some(), "the newest stayed");
    }
}
