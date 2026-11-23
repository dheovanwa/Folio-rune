//! The library and history, stored as one JSON file next to the settings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::article::{ArticleDoc, Block};

/// A saved article: the extracted document plus when it was read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub site: Option<String>,
    pub byline: Option<String>,
    /// Seconds since the Unix epoch, for ordering.
    pub saved_at: u64,
    /// The reading position when it was last open, 0..1.
    pub progress: f32,
    pub doc: StoredDoc,
}

/// `ArticleDoc` in a form serde can write: `Block`/`Inline` are plain data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredDoc {
    pub url: String,
    pub title: String,
    pub byline: Option<String>,
    pub site: Option<String>,
    pub excerpt: Option<String>,
    pub blocks: Vec<Block>,
    pub plain: String,
    /// The next chapter of a serialised read, so a restored entry can still be prefetched.
    #[serde(default)]
    pub next_chapter_url: Option<String>,
}

impl From<&ArticleDoc> for StoredDoc {
    fn from(d: &ArticleDoc) -> StoredDoc {
        StoredDoc {
            url: d.url.clone(),
            title: d.title.clone(),
            byline: d.byline.clone(),
            site: d.site.clone(),
            excerpt: d.excerpt.clone(),
            blocks: d.blocks.clone(),
            plain: d.plain.clone(),
            next_chapter_url: d.next_chapter_url.clone(),
        }
    }
}

impl From<StoredDoc> for ArticleDoc {
    fn from(d: StoredDoc) -> ArticleDoc {
        ArticleDoc {
            url: d.url,
            title: d.title,
            byline: d.byline,
            site: d.site,
            excerpt: d.excerpt,
            blocks: d.blocks,
            plain: d.plain,
            next_chapter_url: d.next_chapter_url,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub entries: Vec<Entry>,
}

impl Library {
    /// Newest first, so the library reads like a reading list.
    pub fn sorted(&self) -> Vec<&Entry> {
        let mut all: Vec<&Entry> = self.entries.iter().collect();
        all.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        all
    }

    pub fn find(&self, url: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.url == url)
    }

    /// Add or refresh an entry, keeping its saved time when it was already there.
    pub fn upsert(&mut self, doc: &ArticleDoc, progress: f32) {
        let saved_at = self.find(&doc.url).map_or_else(now_secs, |e| e.saved_at);
        let entry = Entry {
            url: doc.url.clone(),
            title: doc.title.clone(),
            site: doc.site.clone(),
            byline: doc.byline.clone(),
            saved_at,
            progress,
            doc: StoredDoc::from(doc),
        };
        match self.entries.iter_mut().find(|e| e.url == doc.url) {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    pub fn remove(&mut self, url: &str) {
        self.entries.retain(|e| e.url != url);
    }

    pub fn set_progress(&mut self, url: &str, progress: f32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url) {
            e.progress = progress;
        }
    }

    pub fn path() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("folio").join("library.json"))
    }

    pub fn load() -> Library {
        let Some(path) = Self::path() else { return Library::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Library::default() };
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

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "3h ago" / "12 Nov" — how the library stamps an entry.
pub fn relative_time(saved_at: u64) -> String {
    let now = now_secs();
    let secs = now.saturating_sub(saved_at);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=604_799 => format!("{}d ago", secs / 86_400),
        _ => format!("{}d ago", secs / 86_400),
    }
}
