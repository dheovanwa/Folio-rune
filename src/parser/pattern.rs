//! Text patterns for per-site cleanup, without a regex dependency.
//!
//! A profile names the junk it wants gone — "Advertisement", "Read more at *", "© 2024 Example" —
//! as small glob patterns. Folio ships no regex crate, and `Cargo.toml` is owned elsewhere, so the
//! matcher here is deliberately tiny and total:
//!
//! * `|` separates alternatives: `Advertisement|Sponsored` matches either word.
//! * `*` matches any run of characters (including none): `Read more at *`.
//! * `?` matches exactly one character.
//!
//! Everything else is a literal. Matching is case-insensitive and runs against
//! [`normalise`]d text, so a pattern does not have to reproduce a block's exact spacing.
//! When the `regex` crate is approved, only this file needs to change.

use std::fmt;

/// A compiled set of alternatives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    source: String,
    alts: Vec<Vec<Tok>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Lit(char),
    /// `?`
    One,
    /// `*`
    Many,
}

impl Pattern {
    /// Compile `source`, or say exactly what is wrong with it.
    pub fn new(source: &str) -> Result<Pattern, String> {
        let source = source.trim();
        if source.is_empty() {
            return Err("pattern is empty".to_string());
        }
        let mut alts = Vec::new();
        for alt in source.split('|') {
            if alt.is_empty() {
                return Err(format!("pattern {source:?} has an empty alternative"));
            }
            let mut toks = Vec::with_capacity(alt.chars().count());
            let mut prev_many = false;
            for c in alt.chars() {
                let tok = match c {
                    '*' => {
                        // `**` would mean the same as `*`; collapse so crafted profiles cannot
                        // turn into a long backtrack.
                        if prev_many {
                            continue;
                        }
                        Tok::Many
                    }
                    '?' => Tok::One,
                    c => Tok::Lit(fold(c)),
                };
                prev_many = tok == Tok::Many;
                toks.push(tok);
            }
            if toks.is_empty() {
                return Err(format!("pattern {source:?} has an empty alternative"));
            }
            if toks.iter().all(|t| *t == Tok::Lit(' ')) {
                return Err(format!("pattern {source:?} has an empty alternative"));
            }
            alts.push(toks);
        }
        Ok(Pattern {
            source: source.to_string(),
            alts,
        })
    }

    /// Whether any alternative matches the whole of `text`.
    pub fn is_match(&self, text: &str) -> bool {
        let folded: Vec<char> = normalise(text).chars().map(fold).collect();
        self.alts.iter().any(|alt| glob_match(alt, &folded))
    }

    /// The literal this pattern stands for, when it has no wildcards.
    ///
    /// A `strip_text` rule must remove *part* of a block, and that needs a plain string to search
    /// for; a pattern with a wildcard cannot name one, so it is rejected at validation time.
    pub fn literal(&self) -> Option<&str> {
        if self.alts.len() != 1 {
            return None;
        }
        let alt = &self.alts[0];
        if alt.iter().any(|t| !matches!(t, Tok::Lit(_))) {
            return None;
        }
        Some(self.source.as_str())
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// Collapse whitespace and trim, so "Read more\n  at" and "Read more at" compare equal.
pub fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lowercase one character, without the multi-character expansions `char::to_lowercase` allows.
fn fold(c: char) -> char {
    let mut it = c.to_lowercase();
    it.next().unwrap_or(c)
}

/// Classic wildcard match: two pointers with backtracking to the last `*`, which is linear in
/// practice and needs no recursion depth cap on hostile text.
fn glob_match(pat: &[Tok], text: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_text = 0usize;
    while ti < text.len() {
        match pat.get(pi) {
            Some(Tok::Many) => {
                star = Some(pi);
                star_text = ti;
                pi += 1;
            }
            Some(Tok::One) => {
                pi += 1;
                ti += 1;
            }
            Some(Tok::Lit(' ')) => {
                // A space in a pattern is a word break, so it may be present or not: profiles
                // are written by hand, and "Read more at *" should match "Read more at" too.
                if text[ti] == ' ' {
                    ti += 1;
                }
                pi += 1;
            }
            Some(Tok::Lit(c)) if *c == text[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    star_text += 1;
                    ti = star_text;
                }
                None => return false,
            },
        }
    }
    while matches!(pat.get(pi), Some(Tok::Many) | Some(Tok::Lit(' '))) {
        pi += 1;
    }
    pi == pat.len()
}

/// Remove every case-insensitive occurrence of `needle` from `hay`.
///
/// Used for `strip_text` rules, where a credit line sits inside a paragraph rather than forming
/// one. Only ASCII case is folded, which covers the watermarks this exists for.
pub fn strip_literal(hay: &str, needle: &str) -> String {
    if needle.is_empty() || hay.is_empty() {
        return hay.to_string();
    }
    let n = needle.len();
    let bytes = hay.as_bytes();
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    while i < bytes.len() {
        let fits = i + n <= bytes.len()
            && bytes[i..i + n].eq_ignore_ascii_case(needle.as_bytes())
            && hay.is_char_boundary(i + n);
        if fits {
            i += n;
            continue;
        }
        let ch = match hay[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_match_case_and_spacing_insensitively() {
        let p = Pattern::new("Read more at").unwrap();
        assert!(p.is_match("read   MORE\nat"));
        assert!(!p.is_match("Read more at Example"));
    }

    #[test]
    fn wildcards_cover_the_rest_of_a_line() {
        let p = Pattern::new("Read more at *").unwrap();
        assert!(p.is_match("Read more at example.com/news"));
        assert!(p.is_match("READ MORE AT"));
        assert!(!p.is_match("Read this later"));
    }

    #[test]
    fn a_space_is_a_word_break_not_a_required_character() {
        let p = Pattern::new("Share this").unwrap();
        assert!(p.is_match("sharethis"), "a missing space is still a match");
        assert!(
            p.is_match("Share  this"),
            "repeated spacing is normalised first"
        );
        assert!(!p.is_match("Sharethat"));
    }

    #[test]
    fn alternatives_and_single_char_wildcards_work() {
        let p = Pattern::new("Advertisement|Spons?red").unwrap();
        assert!(p.is_match(" advertisement "));
        assert!(p.is_match("Sponsored"));
        assert!(!p.is_match("Spnsored"));
    }

    #[test]
    fn only_a_wildcard_free_pattern_has_a_literal() {
        assert_eq!(
            Pattern::new("© Example").unwrap().literal(),
            Some("© Example")
        );
        assert_eq!(Pattern::new("a*").unwrap().literal(), None);
        assert_eq!(Pattern::new("a|b").unwrap().literal(), None);
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        assert!(Pattern::new("").is_err());
        assert!(Pattern::new("a||b").is_err());
        assert!(Pattern::new("|").is_err());
    }

    #[test]
    fn stripping_removes_every_occurrence_and_keeps_the_rest() {
        let got = strip_literal(
            "Advertisement Read on. advertisement End.",
            "advertisement ",
        );
        assert_eq!(got, "Read on. End.");
    }

    #[test]
    fn a_long_text_does_not_run_away() {
        let p = Pattern::new("*x").unwrap();
        let hay = "a".repeat(4000);
        assert!(!p.is_match(&hay));
    }
}
