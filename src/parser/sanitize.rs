//! Applying a site's cleanup rules to extracted blocks.
//!
//! Two kinds of rule, because sites leak junk in two ways:
//!
//! * `drop_blocks` — the whole block *is* the junk: "Advertisement", "Share this article".
//! * `strip_text` — the junk sits inside real prose: a credit line appended to a paragraph, or a
//!   watermark bracketed in the middle of a sentence.
//!
//! Code blocks are never touched: they are the one place where a string that looks like a
//! watermark may be the point.

use super::pattern::{Pattern, strip_literal};
use super::profile::Profile;
use crate::article::{Block, Inline};

/// What a cleanup pass removed, for logging and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleanup {
    /// Blocks dropped because their whole text matched a rule.
    pub dropped: usize,
    /// Inline runs that had text cut out of them.
    pub stripped: usize,
}

impl Cleanup {
    pub fn is_empty(&self) -> bool {
        self.dropped == 0 && self.stripped == 0
    }
}

/// A profile's rules, ready to run.
#[derive(Clone, Debug)]
pub struct Sanitizer<'a> {
    profile: &'a Profile,
}

impl<'a> Sanitizer<'a> {
    pub fn new(profile: &'a Profile) -> Self {
        Sanitizer { profile }
    }

    /// Clean `blocks` in place. Empty results are dropped rather than left as blank gaps.
    pub fn apply(&self, blocks: &mut Vec<Block>) -> Cleanup {
        let mut cleanup = Cleanup::default();
        if self.profile.is_empty_rules() {
            return cleanup;
        }

        if !self.profile.drop_blocks().is_empty() {
            cleanup.dropped += drop_junk(blocks, self.profile.drop_blocks());
        }
        if !self.profile.strip_text().is_empty() {
            cleanup.stripped += strip_junk(blocks, self.profile.strip_text());
        }
        // A block left with no words is a blank line the reader would have to scroll past.
        let before = blocks.len();
        blocks.retain(|b| !is_blank(b));
        cleanup.dropped += before - blocks.len();
        cleanup
    }
}

/// The article's plain text, rebuilt from its blocks.
///
/// Rebuilding, rather than patching the extractor's `text_content`, keeps search, the word count
/// and what Copy puts on the clipboard in step with what the reader is looking at.
pub fn plain_of(blocks: &[Block]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Heading { text, .. } => parts.push(text.clone()),
            Block::Para { spans } | Block::Quote { spans } => parts.push(spans_text(spans)),
            Block::List { items, .. } => {
                for item in items {
                    parts.push(spans_text(item));
                }
            }
            Block::Code { text } => parts.push(text.clone()),
            Block::Rule => {}
        }
    }
    parts.retain(|p| !p.trim().is_empty());
    parts.join("\n\n")
}

fn spans_text(spans: &[Inline]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn is_blank(block: &Block) -> bool {
    match block {
        Block::Heading { text, .. } => text.trim().is_empty(),
        Block::Para { spans } | Block::Quote { spans } => {
            spans.iter().all(|s| s.text.trim().is_empty())
        }
        Block::List { items, .. } => items.is_empty(),
        Block::Code { text } => text.trim().is_empty(),
        Block::Rule => false,
    }
}

/// Remove blocks whose whole text matches a junk pattern.
fn drop_junk(blocks: &mut Vec<Block>, patterns: &[Pattern]) -> usize {
    let mut dropped = 0;
    let mut kept: Vec<Block> = Vec::with_capacity(blocks.len());
    for block in blocks.drain(..) {
        match block {
            // A list is judged item by item: one junk bullet must not cost the whole list.
            Block::List {
                ordered,
                start,
                items,
            } => {
                let kept_items: Vec<Vec<Inline>> = items
                    .into_iter()
                    .filter(|item| {
                        let text = spans_text(item);
                        let junk = patterns.iter().any(|p| p.is_match(&text));
                        if junk {
                            dropped += 1;
                        }
                        !junk
                    })
                    .collect();
                if !kept_items.is_empty() {
                    kept.push(Block::List {
                        ordered,
                        start,
                        items: kept_items,
                    });
                }
            }
            other => {
                let text = block_text(&other);
                if patterns.iter().any(|p| p.is_match(&text)) {
                    dropped += 1;
                } else {
                    kept.push(other);
                }
            }
        }
    }
    *blocks = kept;
    dropped
}

fn block_text(block: &Block) -> String {
    match block {
        Block::Heading { text, .. } => text.clone(),
        Block::Para { spans } | Block::Quote { spans } => spans_text(spans),
        Block::List { items, .. } => items
            .iter()
            .map(|i| spans_text(i))
            .collect::<Vec<_>>()
            .join(" "),
        Block::Code { text } => text.clone(),
        Block::Rule => String::new(),
    }
}

/// Cut literal junk out of the text of every block, and count the runs it changed.
fn strip_junk(blocks: &mut [Block], rules: &[(Pattern, String)]) -> usize {
    let mut touched = 0;
    for block in blocks.iter_mut() {
        match block {
            Block::Para { spans } | Block::Quote { spans } => {
                touched += strip_spans(spans, rules);
            }
            Block::Heading { text, .. } => {
                let (new, removed) = strip_all(text, rules);
                if removed > 0 {
                    touched += 1;
                    *text = tidy(&new);
                }
            }
            Block::List { items, .. } => {
                for item in items.iter_mut() {
                    touched += strip_spans(item, rules);
                }
            }
            // Prose only: a code block's text is content, not a place for watermarks.
            Block::Code { .. } | Block::Rule => {}
        }
    }
    touched
}

fn strip_spans(spans: &mut Vec<Inline>, rules: &[(Pattern, String)]) -> usize {
    let mut touched = 0;
    let mut out: Vec<Inline> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        let (new, removed) = strip_all(&span.text, rules);
        if removed == 0 {
            push_run(&mut out, span);
            continue;
        }
        touched += 1;
        let text = tidy(&new);
        if text.is_empty() {
            continue;
        }
        push_run(
            &mut out,
            Inline {
                text,
                kind: span.kind,
            },
        );
    }
    *spans = out;
    touched
}

/// Remove every rule's literal from `text`, in rule order.
fn strip_all(text: &str, rules: &[(Pattern, String)]) -> (String, usize) {
    let mut out = text.to_string();
    let mut removed = 0;
    for (_, literal) in rules {
        // `strip_literal` folds ASCII case; compare lengths to count what went.
        let after = strip_literal(&out, literal);
        if after.len() != out.len() {
            removed += out.len() - after.len();
            out = after;
        }
    }
    (out, removed)
}

/// Join a run onto the previous one when it has the same formatting, so cutting text out does not
/// leave two adjacent plain runs that the reader's search offsets would then split across.
fn push_run(out: &mut Vec<Inline>, run: Inline) {
    match out.last_mut() {
        Some(last) if last.kind == run.kind => last.text.push_str(&run.text),
        _ => out.push(run),
    }
}

/// Collapse runs of spaces and trim, as the extractor's own whitespace pass does.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article::InlineKind;

    fn profile(rules: &str) -> Profile {
        Profile::parse(&format!("domain = \"example.com\"\n{rules}"), "test.toml").unwrap()
    }

    fn para(text: &str) -> Block {
        Block::Para {
            spans: vec![Inline {
                text: text.into(),
                kind: InlineKind::Text,
            }],
        }
    }

    fn text_of(block: &Block) -> String {
        block_text(block)
    }

    #[test]
    fn drop_rules_remove_the_whole_block() {
        let p = profile("[sanitize]\ndrop_blocks = [\"Advertisement\", \"Sponsored *\"]");
        let mut blocks = vec![
            para("Real prose that survives."),
            para("Advertisement"),
            para("Sponsored by Example Co"),
            para("More prose."),
        ];
        let cleanup = Sanitizer::new(&p).apply(&mut blocks);
        assert_eq!(cleanup.dropped, 2);
        let texts: Vec<String> = blocks.iter().map(text_of).collect();
        assert_eq!(texts, vec!["Real prose that survives.", "More prose."]);
    }

    #[test]
    fn junk_list_items_go_but_the_list_stays() {
        let p = profile("[sanitize]\ndrop_blocks = [\"Advertisement\"]");
        let mut blocks = vec![Block::List {
            ordered: false,
            start: 1,
            items: vec![
                vec![Inline {
                    text: "Keep me".into(),
                    kind: InlineKind::Text,
                }],
                vec![Inline {
                    text: "Advertisement".into(),
                    kind: InlineKind::Text,
                }],
            ],
        }];
        Sanitizer::new(&p).apply(&mut blocks);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            Block::List {
                ordered: false,
                start: 1,
                items: vec![vec![Inline {
                    text: "Keep me".into(),
                    kind: InlineKind::Text
                }]],
            }
        );
    }

    #[test]
    fn strip_rules_cut_text_out_of_prose_and_keep_formatting() {
        let p = profile("[sanitize]\nstrip_text = [\"(Advertisement)\"]");
        let mut blocks = vec![Block::Para {
            spans: vec![
                Inline {
                    text: "He waited (Advertisement) for the train.".into(),
                    kind: InlineKind::Text,
                },
                Inline {
                    text: "Then he left.".into(),
                    kind: InlineKind::Bold,
                },
            ],
        }];
        let cleanup = Sanitizer::new(&p).apply(&mut blocks);
        assert_eq!(cleanup.stripped, 1);
        assert_eq!(
            blocks[0],
            Block::Para {
                spans: vec![
                    Inline {
                        text: "He waited for the train.".into(),
                        kind: InlineKind::Text
                    },
                    Inline {
                        text: "Then he left.".into(),
                        kind: InlineKind::Bold
                    },
                ]
            }
        );
    }

    #[test]
    fn a_block_stripped_to_nothing_disappears() {
        let p = profile("[sanitize]\nstrip_text = [\"Read more at example.com\"]");
        let mut blocks = vec![para("Read more at example.com"), para("Prose.")];
        Sanitizer::new(&p).apply(&mut blocks);
        assert_eq!(blocks.len(), 1);
        assert_eq!(text_of(&blocks[0]), "Prose.");
    }

    #[test]
    fn code_is_never_stripped() {
        let p = profile("[sanitize]\nstrip_text = [\"foo\"]");
        let mut blocks = vec![Block::Code {
            text: "let foo = 1;".into(),
        }];
        Sanitizer::new(&p).apply(&mut blocks);
        assert_eq!(
            blocks[0],
            Block::Code {
                text: "let foo = 1;".into()
            }
        );
    }

    #[test]
    fn a_profile_without_rules_changes_nothing() {
        let p = profile("");
        let mut blocks = vec![para("Advertisement"), para("Prose.")];
        let cleanup = Sanitizer::new(&p).apply(&mut blocks);
        assert!(cleanup.is_empty());
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn plain_is_rebuilt_from_the_cleaned_blocks() {
        let blocks = vec![
            Block::Heading {
                level: 2,
                text: "A Part".into(),
            },
            para("First."),
            Block::List {
                ordered: true,
                start: 1,
                items: vec![
                    vec![Inline {
                        text: "One".into(),
                        kind: InlineKind::Text,
                    }],
                    vec![Inline {
                        text: "Two".into(),
                        kind: InlineKind::Text,
                    }],
                ],
            },
            Block::Rule,
        ];
        assert_eq!(plain_of(&blocks), "A Part\n\nFirst.\n\nOne\n\nTwo");
    }
}
