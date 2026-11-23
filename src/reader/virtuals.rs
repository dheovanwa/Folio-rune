//! Row heights for the virtualised reader.
//!
//! A virtual list lays out only its mounted rows and needs a height for the ones it has not
//! measured yet. Guessing per block kind (a heading is short, a code block is one line per row
//! of text) keeps the scroll bar honest before the reader arrives, and Rune replaces the guess
//! with the measured height as soon as a row is built, keyed by its block so it stays put.
//!
//! A height here is the row's **content** height. The gap between rows belongs to the virtual
//! list's own `gap`, which Rune applies between rows and counts in the extent, so adding it here
//! too would double-space the article.
//!
//! Estimates are pure arithmetic on the article model: no shaping, no layout, nothing that
//! touches the UI thread's budget.

use crate::article::Block;

/// The reader settings an estimate depends on, copied out so a build closure captures nothing
/// borrowed from the app state.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// Body type size in points.
    pub font: f32,
    /// Body leading in points (size × line-height).
    pub line: f32,
    /// The measure, inline padding excluded: how wide a line may grow.
    pub measure: f32,
    /// The gap the reader keeps between blocks, which the virtual list applies.
    pub gap: f32,
}

impl Metrics {
    /// How many characters of body type fit on one line. The 0.5 factor is the average advance
    /// of a serif lowercase at these sizes; it is a guess, and a row's measured height replaces
    /// it the moment it is built.
    fn chars_per_line(&self) -> f32 {
        (self.measure / (self.font * 0.5).max(1.0)).max(12.0)
    }

    fn lines(&self, chars: usize) -> f32 {
        (chars as f32 / self.chars_per_line()).ceil().max(1.0)
    }
}

/// The guessed content height of one block, without the gap below it.
pub fn block_height(m: Metrics, block: &Block) -> f32 {
    match block {
        Block::Heading { level, text } => {
            let scale = match level {
                1 => 1.55,
                2 => 1.3,
                3 => 1.12,
                _ => 1.0,
            };
            let line = m.font * scale * 1.25;
            // Headings are set on a narrower average advance than prose: shorter words, wider
            // tracking at the big sizes.
            let per_line = (m.measure / (m.font * scale * 0.55).max(1.0)).max(8.0);
            let lines = (text.chars().count() as f32 / per_line).ceil().max(1.0);
            let pad = if *level <= 2 { 18.0 } else { 10.0 };
            lines * line + pad
        }
        Block::Para { spans } => {
            let chars: usize = spans.iter().map(|s| s.text.chars().count()).sum();
            m.lines(chars) * m.line
        }
        Block::Quote { spans } => {
            let chars: usize = spans.iter().map(|s| s.text.chars().count()).sum();
            // The 3 pt rule and its 16 pt gutter take width away from the text.
            let narrowed = Metrics { measure: (m.measure - 19.0).max(80.0), ..m };
            narrowed.lines(chars) * m.line
        }
        Block::List { items, .. } => {
            // The 24 pt marker column and its 12 pt gap narrow every item's text alike.
            let narrowed = Metrics { measure: (m.measure - 36.0).max(80.0), ..m };
            let mut total = 0.0;
            for (n, item) in items.iter().enumerate() {
                let chars: usize = item.iter().map(|s| s.text.chars().count()).sum();
                total += narrowed.lines(chars) * m.line;
                // The list's own 8 pt gap, which is inside the block rather than between rows.
                if n + 1 < items.len() {
                    total += 8.0;
                }
            }
            total.max(m.line)
        }
        Block::Code { text } => text.lines().count().max(1) as f32 * m.font * 0.82 * 1.35 + 32.0,
        // The rule's own 12 pt padding above and below, and the hairline itself.
        Block::Rule => 25.0,
    }
}

/// One row height for a list that does not know each of its rows yet: the mean of the first
/// blocks, which follow the article's own mix of prose and headings far better than a constant.
///
/// The first 24 blocks are enough to be representative and cheap to walk on every commit.
pub fn estimate(m: Metrics, blocks: &[Block]) -> f32 {
    let sample = blocks.len().min(24);
    if sample == 0 {
        return m.line;
    }
    let total: f32 = blocks[..sample].iter().map(|b| block_height(m, b)).sum();
    (total / sample as f32).max(12.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article::{Inline, InlineKind};

    fn m() -> Metrics {
        Metrics { font: 18.0, line: 30.6, measure: 720.0, gap: 16.0 }
    }

    fn para(chars: usize) -> Block {
        Block::Para {
            spans: vec![Inline { text: "a".repeat(chars), kind: InlineKind::Text }],
        }
    }

    #[test]
    fn prose_grows_with_its_length() {
        let short = block_height(m(), &para(40));
        let long = block_height(m(), &para(4000));
        assert!(short < 60.0, "a short paragraph is about a line: {short}");
        assert!(long > 300.0, "a long one is many lines: {long}");
    }

    #[test]
    fn a_heading_is_shorter_than_a_long_paragraph() {
        let h = block_height(m(), &Block::Heading { level: 2, text: "Measure".into() });
        assert!(h > 30.0 && h < 120.0, "a heading estimate stays near its own line: {h}");
    }

    #[test]
    fn a_code_block_counts_its_lines() {
        let one = block_height(m(), &Block::Code { text: "let x = 1;".into() });
        let ten = block_height(m(), &Block::Code { text: "let x = 1;\n".repeat(10) });
        assert!(ten > one * 5.0, "ten lines are much taller than one: {one} vs {ten}");
    }

    #[test]
    fn a_row_height_carries_no_gap_because_the_list_owns_it() {
        let row = block_height(m(), &para(40));
        let with_gap = super::Metrics { gap: 200.0, ..m() };
        assert_eq!(row, block_height(with_gap, &para(40)), "the gap is not part of a row");
    }

    #[test]
    fn the_estimate_is_a_plausible_row_and_never_zero() {
        let blocks = vec![para(600), Block::Heading { level: 2, text: "H".into() }, para(120)];
        let e = estimate(m(), &blocks);
        assert!(e > 20.0 && e < 400.0, "mean row height {e}");
        assert!(estimate(m(), &[]) > 0.0, "an empty article still has a row height");
    }
}
