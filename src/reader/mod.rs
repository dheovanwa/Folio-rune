//! The reading model, as one value the app holds: auto-hide for the chrome, the saved place in
//! each article, and the tester's scratch state.
//!
//! Deliberately not a `dyn` anything: the view reads plain fields, and every method that writes
//! a file runs from a press or a key, never from a frame.

pub mod chrome;
pub mod position;
pub mod selector;
pub mod virtuals;

use rune::TextEdit;

use crate::article::{ArticleDoc, Block};
use crate::config::Config;

/// The tester's own state: whether it shows, what was typed, and what was last asked about.
pub struct Tester {
    pub open: bool,
    pub edit: TextEdit,
    /// The selector last asked about. The rect is read in the view, where layout exists.
    pub probe: Option<String>,
}

impl Default for Tester {
    fn default() -> Self {
        Tester { open: false, edit: TextEdit::new(""), probe: None }
    }
}

/// Everything the reader surface owns besides the document itself.
pub struct Reader {
    pub chrome: chrome::Chrome,
    pub positions: position::Positions,
    pub tester: Tester,
    /// The block range the virtual list has built, `(first, last)` inclusive, in *block* indexes.
    pub visible_rows: (usize, usize),
    /// The block index of the list's first row. Usually 1, because the H1 the article head
    /// already draws is not a row.
    pub first: usize,
    /// Bumped when a different article opens, so the virtual list carries no measured heights
    /// across from the last one.
    pub generation: u64,
    /// The block the reader was at when the last mark was taken, for `save_place`.
    pub at_block: usize,
    /// The scroll offset the last poll saw, so a scroll can count as the reader being active
    /// without a handler on every wheel tick.
    pub last_offset: f32,
    /// Distraction-free mode's switch. Runtime-only: `config.rs` belongs to another agent's
    /// scope, so this does not persist yet.
    pub distraction_free: bool,
}

impl Default for Reader {
    fn default() -> Self {
        Reader::new()
    }
}

impl Reader {
    pub fn new() -> Reader {
        Reader {
            chrome: chrome::Chrome::default(),
            positions: position::Positions::load(),
            tester: Tester::default(),
            visible_rows: (0, 0),
            first: 0,
            generation: 0,
            at_block: 0,
            last_offset: 0.0,
            distraction_free: true,
        }
    }

    /// A different article is opening: a fresh list, the chrome showing, and the saved place
    /// ready to be revealed once layout has run.
    ///
    /// Returns the block to return to, if this article has one.
    pub fn opened(&mut self, doc: &ArticleDoc) -> Option<usize> {
        self.generation = self.generation.wrapping_add(1);
        self.visible_rows = (0, 0);
        self.first = first_row(doc);
        self.at_block = self.first;
        self.chrome.set_enabled(self.distraction_free);
        self.chrome.opened();
        self.positions.get(&doc.url).filter(|m| m.block >= self.first).map(|m| m.block)
    }

    /// Record where the reader is, and write it. Called from a key or a press that leaves the
    /// page, or from the close hook, never from a frame.
    pub fn remember(&mut self, url: &str, offset: f32, frac: f32) {
        if url.is_empty() {
            return;
        }
        self.positions.remember(url, self.at_block, offset, frac);
        self.positions.save();
    }

    /// The row-height settings, copied out so a build closure captures nothing borrowed.
    pub fn metrics(cfg: &Config) -> virtuals::Metrics {
        virtuals::Metrics {
            font: cfg.font_size,
            line: cfg.line_px(),
            measure: cfg.max_width,
            gap: cfg.paragraph_spacing_px,
        }
    }

    /// Which block is at the top of the viewport, from the rows the list has built and where it
    /// is scrolled. A row that is built but scrolled past is not the one being read, so the
    /// caller passes the offset and the row heights it can know cheaply: the top row plus the
    /// fraction of the way through the built rows.
    ///
    /// This is an approximation on purpose. It has to be: the exact block under the viewport's
    /// top edge is only known after layout, and asking layout from a handler would commit.
    pub fn block_at(&self, built: (usize, usize), frac_through_rows: f32) -> usize {
        let (a, b) = built;
        if b <= a {
            return a;
        }
        let span = (b - a) as f32;
        a + (frac_through_rows.clamp(0.0, 1.0) * span).round() as usize
    }
}

/// The block index the first row shows: a leading H1 is drawn in the article head instead.
pub fn first_row(doc: &ArticleDoc) -> usize {
    match doc.blocks.first() {
        Some(Block::Heading { level: 1, .. }) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article::{Inline, InlineKind};

    fn doc(first_level: u8) -> ArticleDoc {
        ArticleDoc {
            url: "https://example.com/x".into(),
            title: "T".into(),
            byline: None,
            site: None,
            excerpt: None,
            blocks: vec![
                Block::Heading { level: first_level, text: "T".into() },
                Block::Para { spans: vec![Inline { text: "body".into(), kind: InlineKind::Text }] },
            ],
            plain: "T body".into(),
            next_chapter_url: None,
        }
    }

    #[test]
    fn a_leading_h1_is_not_a_row_but_any_other_heading_is() {
        assert_eq!(first_row(&doc(1)), 1, "the headline is drawn in the article head");
        assert_eq!(first_row(&doc(2)), 0, "an H2 is content and keeps its row");
    }

    #[test]
    fn opening_a_new_article_starts_a_fresh_list_and_restores_the_saved_block() {
        let mut r = Reader::new();
        r.positions.forget("https://example.com/x");
        r.positions.remember("https://example.com/x", 1, 120.0, 0.4);
        let at = r.opened(&doc(1));
        assert_eq!(at, Some(1));
        assert_eq!(r.generation, 1);
        assert!(r.chrome.is_visible(), "a new article shows the chrome");
        r.opened(&doc(1));
        assert_eq!(r.generation, 2, "a second article is a second list");
    }

    #[test]
    fn no_saved_place_means_no_reveal() {
        let mut r = Reader::new();
        r.positions.forget("https://example.com/x");
        assert_eq!(r.opened(&doc(2)), None);
        assert_eq!(r.first, 0);
    }

    #[test]
    fn a_mark_before_the_first_row_is_not_restored() {
        let mut r = Reader::new();
        r.positions.remember("https://example.com/x", 0, 0.0, 0.0);
        assert_eq!(r.opened(&doc(1)), None, "block 0 is the headline, not a row");
    }

    #[test]
    fn the_block_at_the_top_follows_where_the_list_is() {
        let r = Reader::new();
        assert_eq!(r.block_at((10, 30), 0.0), 10, "at the top of the built rows");
        assert_eq!(r.block_at((10, 30), 1.0), 30, "at the bottom of them");
        assert_eq!(r.block_at((7, 7), 0.5), 7, "a single row is its own answer");
    }
}
