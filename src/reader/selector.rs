//! The Live Selector Tester's model: what a typed selector means and how a probe reads.
//!
//! Rune addresses an element by its key, or by a key path such as `"block-12/quote-body"` whose
//! leading segments are its nearest keyed ancestors. That is the same syntax `Tx::scroll_into_view`
//! and `Cx::layout` take, so a tester built on it tells the reader what the app itself would do
//! with the string, not a lookalike parser's opinion.

/// The longest key path the tester accepts. A real one is two or three segments; a longer one is
/// almost always a paste of a full element path, which is not what the request syntax takes.
const MAX_PARTS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    parts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectorError {
    Empty,
    BlankSegment,
    TooManyParts,
    Whitespace,
}

impl SelectorError {
    pub fn message(self) -> &'static str {
        match self {
            SelectorError::Empty => "Type a key, such as block-3",
            SelectorError::BlankSegment => "A key path has an empty segment: use key/child",
            SelectorError::TooManyParts => "That is deeper than any key path in Folio",
            SelectorError::Whitespace => "Keys have no spaces in them",
        }
    }
}

impl Selector {
    pub fn parse(raw: &str) -> Result<Selector, SelectorError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorError::Empty);
        }
        if raw.chars().any(char::is_whitespace) {
            return Err(SelectorError::Whitespace);
        }
        let parts: Vec<String> = raw.split('/').map(str::to_string).collect();
        if parts.len() > MAX_PARTS {
            return Err(SelectorError::TooManyParts);
        }
        if parts.iter().any(|p| p.is_empty()) {
            return Err(SelectorError::BlankSegment);
        }
        Ok(Selector { parts })
    }

    /// The selector exactly as Rune's request APIs take it.
    #[allow(dead_code)]
    pub fn raw(&self) -> String {
        self.parts.join("/")
    }

    /// The element's own key: the last segment.
    #[allow(dead_code)]
    pub fn key(&self) -> &str {
        self.parts.last().map(String::as_str).unwrap_or("")
    }

    /// Whether the selector is scoped to an ancestor, so the tester can say the scope matters.
    pub fn is_path(&self) -> bool {
        self.parts.len() > 1
    }
}

/// What the tester prints for a probe: a rect in window points, or why there is nothing to show.
///
/// A miss is the normal answer for a row that a virtual list has not built yet, so the message
/// says that rather than "not found": the reader then knows to scroll a little and probe again.
pub fn describe(rect: Option<[f32; 4]>, visible: Option<(usize, usize)>) -> String {
    match rect {
        Some([x, y, w, h]) => format!("x {x:.0}  y {y:.0}  w {w:.0}  h {h:.0}"),
        None => match visible {
            Some((from, to)) => format!("Not built. Rows {from}–{to} are on screen; scroll closer."),
            None => "Not built or not in the tree.".to_string(),
        },
    }
}

/// Keys worth offering as one-press probes: the rows around the reader's current place.
///
/// A virtual list builds only nearby rows, so a key far from the viewport is exactly the case a
/// tester is for; offering the near ones first is what makes it usable without typing.
pub fn nearby_rows(from: usize, to: usize, count: usize, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let span = to.saturating_sub(from).saturating_add(1).max(1);
    let first = from.saturating_sub(1);
    let last = (first + span + 1).min(count.saturating_sub(1));
    for i in first..=last {
        if i < count && out.len() < limit {
            out.push(format!("block-{i}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_key_and_a_key_path_both_parse() {
        assert_eq!(Selector::parse("block-12").unwrap().raw(), "block-12");
        let scoped = Selector::parse("block-12/quote-body").unwrap();
        assert!(scoped.is_path());
        assert_eq!(scoped.key(), "quote-body");
        assert_eq!(Selector::parse("  block-3  ").unwrap().raw(), "block-3");
    }

    #[test]
    fn malformed_selectors_say_what_is_wrong() {
        assert_eq!(Selector::parse(""), Err(SelectorError::Empty));
        assert_eq!(Selector::parse("a//b"), Err(SelectorError::BlankSegment));
        assert_eq!(Selector::parse("a b"), Err(SelectorError::Whitespace));
        assert_eq!(Selector::parse("a/".repeat(9).as_str()), Err(SelectorError::TooManyParts));
    }

    #[test]
    fn a_probe_reports_a_rect_or_why_it_is_missing() {
        assert!(describe(Some([1.0, 2.0, 30.0, 40.0]), None).starts_with("x 1"));
        assert!(describe(None, Some((4, 9))).contains("Rows 4–9"));
    }

    #[test]
    fn nearby_rows_stay_inside_the_article() {
        let rows = nearby_rows(0, 2, 5, 8);
        assert_eq!(rows, vec!["block-0", "block-1", "block-2", "block-3", "block-4"]);
        let tail = nearby_rows(40, 44, 45, 8);
        assert_eq!(tail.last().unwrap(), "block-44", "never past the last row");
    }
}
