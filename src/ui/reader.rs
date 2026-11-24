//! The reading surface: serif prose at a fixed measure, with Folio's block styles.
//!
//! The body is a *virtual* list. A chapter can be thousands of blocks long, and drawing all of
//! them costs a layout pass over text nobody is looking at, so only the rows inside (or just
//! outside) the viewport are built. Each row is keyed by its block index — `block-12` — which is
//! the same key the table of contents, the find bar and the selector tester use, so revealing a
//! paragraph is a key lookup rather than a guess at a scroll offset in points.
//!
//! Two consequences worth knowing:
//!
//! - A row that is not built has no elements. Text selection and `Cmd+C` cover the built rows;
//!   `Cmd+C` with nothing selected still copies the whole article, because that path reads the
//!   document rather than the tree.
//! - The article's headline and byline sit above the list as ordinary children of the same
//!   scroll viewport, not as rows. A virtual stack may be the viewport's content or sit among
//!   other content in it, and this keeps the headline out of the row estimate.

use std::borrow::Cow;

use rune::tok::radius;
use rune::{Align, El, GrowthSpec, ScrollSpec, Span, col, el, rich, row, text};

use crate::State;
use crate::article::{Block, Inline, InlineKind};
use crate::reader::virtuals;
use crate::theme::Palette;

pub fn view(s: &State, p: Palette, hair: f32) -> El {
    let Some(doc) = &s.doc else {
        return super::empty::view(s, p, hair);
    };
    let measure = s.cfg.max_width;
    let first = s.reader.first;

    // The headline block: the article's own title, byline and reading time. It is not a row,
    // so it keeps its own spacing and is never measured into the list's estimate.
    let head = col()
        .key("article-head")
        .w_full()
        .gap(10.0)
        .mb(12.0)
        .child(
            text(doc.title.clone())
                .key("headline")
                .w_full()
                .font(
                    // The leading must clear the type size, not the base body size: an
                    // `lh` below 1.0 collapses stacked headline lines into each other.
                    rune::TextStyle::new(s.cfg.font_size * 1.7, 700.0)
                        .lh(s.cfg.font_size * 1.7 * 1.25),
                )
                .color(p.fg)
                .wrap(),
        )
        .child(
            row()
                .key("byline")
                .w_full()
                .gap(10.0)
                .child(super::meta(
                    s,
                    p,
                    {
                        let mut parts = Vec::new();
                        if let Some(by) = &doc.byline {
                            parts.push(by.clone());
                        }
                        parts.push(format!("{} min read", doc.reading_minutes()));
                        parts.join(" · ")
                    },
                )),
        );

    // The rows are the blocks after the headline. `first` is 1 only when the document opens
    // with an H1, which the head above already draws.
    let count = doc.blocks.len().saturating_sub(first);
    let metrics = crate::reader::Reader::metrics(&s.cfg);
    let estimate = virtuals::estimate(metrics, &doc.blocks[first..]);

    let rows = col()
        .key("article-rows")
        .w_full()
        .gap(s.cfg.paragraph_spacing_px)
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            estimate,
            // Row `i` is block `first + i`, and its key says so: the article's block index is
            // the one stable name a paragraph has across a re-extract, a re-wrap and a resize.
            |s: &State, i| Cow::Owned(format!("block-{}", s.reader.first + i)),
            |s: &State, i| {
                let p = s.palette();
                let block = &s.doc.as_ref().expect("rows outlive their document").blocks[s.reader.first + i];
                block_view(s, p, block, s.reader.first + i)
            },
        )
        // The block range the list has built, for the selector tester's "not built" message and
        // its near-row probes. Only a changed answer the view reads asks for another run, so
        // scrolling a closed tester costs no view rebuild.
        .on_visible_rows(|s: &mut State, r: std::ops::Range<usize>| {
            let first = s.reader.first;
            s.reader.visible_rows = (first + r.start, first + r.end.saturating_sub(1));
            let frac = s.progress.get();
            s.reader.at_block = s.reader.block_at(s.reader.visible_rows, frac);
            s.reader.tester.open
        })
        // A different article is a different list: its keys and its measured heights must not
        // carry over from the one before.
        .virtual_rows_version(s.reader.generation);

    let scroller = col()
        .key("article")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .focusable(true)
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(super::watch_scroll(s))
        .on_key(|s: &mut State, _ev: &rune::KeyEvent, _tx: &mut rune::Tx| {
            s.reader.chrome.nudge();
            false
        })
        .child(
            row()
                .key("article-row")
                .w_full()
                .justify_center()
                .px(s.cfg.margin)
                .py(40.0)
                .child(
                    col()
                        .key("measure")
                        .max_w(measure)
                        .w_full()
                        .child(head)
                        .child(rows),
                ),
        );

    // A permanent scroll bar in its own gutter: the text keeps its full measure and the bar
    // says how much is left without waiting for a hover. Its thumb is draggable.
    let body = super::scrollbar::with_bar(p, "article", &s.scroll_bar, scroller);

    if !s.find_focus && s.hit_query.is_empty() {
        return body;
    }
    col()
        .key("reader")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(body)
        .child(find_bar(s, p, hair))
}

/// The find bar: a query field, the hit count, and step buttons.
///
/// It sits at the bottom of the reading surface rather than floating, so opening it does not
/// cover the line the current hit is on.
fn find_bar(s: &State, p: Palette, hair: f32) -> El {
    let count = if s.hit_query.is_empty() {
        String::new()
    } else if s.hits.is_empty() {
        "No results".to_string()
    } else {
        format!("{} of {}", s.hit_at + 1, s.hits.len())
    };

    let mut row = row()
        .key("find-row")
        .w_full()
        .h(52.0)
        .px(20.0)
        .gap(10.0)
        .center_y();

    row = row.child(
        rune::text_input(&s.find_edit, rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
            .key("find")
            .role("searchbox")
            .label("Find in article")
            .grow(1.0)
            .h(32.0)
            .px(10.0)
            .radius(radius::SM)
            .fill(p.field_bg)
            .border(2.0, p.accent)
            .caret(p.accent, p.accent.alpha(0.25))
            .placeholder("Find in article", p.meta)
            .single_line()
            .on_edit(|s: &mut State| &mut s.find_edit)
            // The query is recomputed as it is typed, so the count and the highlights follow
            // the field without a separate "search" key.
            .on_input_change(|s: &mut State, tx: &mut rune::Tx| {
                s.refresh_hits();
                s.reveal_current_hit(tx);
            })
            .on_input_key(|s: &mut State, ev: &rune::KeyEvent, tx: &mut rune::Tx| match ev.key {
                rune::Key::Escape => {
                    if let Some(hit) = s.current_hit() {
                        s.scroll_to = Some(Some(hit.block));
                        tx.scroll_into_view(format!("block-{}", hit.block));
                    }
                    s.find_focus = false;
                    s.find_edit = rune::TextEdit::new("");
                    s.hits.clear();
                    s.hit_at = 0;
                    s.hit_query.clear();
                    tx.focus("article", false);
                    true
                }
                rune::Key::Enter => {
                    s.step_hit_tx(if ev.mods.shift { -1 } else { 1 }, tx);
                    true
                }
                rune::Key::Down => {
                    s.step_hit_tx(1, tx);
                    true
                }
                rune::Key::Up => {
                    s.step_hit_tx(-1, tx);
                    true
                }
                _ => false,
            }),
    );

    if !count.is_empty() {
        row = row.child(
            text(count)
                .key("find-count")
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.8, 400.0))
                .color(p.meta)
                .truncate(),
        );
    }

    for (key, label, by) in [("find-prev", "↑", -1), ("find-next", "↓", 1)] {
        row = row.child(
            el()
                .key(key)
                .role("button")
                .label(if by < 0 { "Previous match" } else { "Next match" })
                .focusable(true)
                .h(28.0)
                .w(28.0)
                .center()
                .radius(radius::SM)
                .hover(p.hover)
                .on_press_tx(move |s: &mut State, tx: &mut rune::Tx| s.step_hit_tx(by, tx))
                .child(
                    text(label)
                        .key("g")
                        .font(s.ui_style(400.0))
                        .color(p.fg)
                        .a11y_hidden(),
                ),
        );
    }

    row = row.child(
        el().key("find-close")
            .role("button")
            .label("Close find")
            .focusable(true)
            .h(28.0)
            .px(10.0)
            .center_y()
            .radius(radius::SM)
            .hover(p.hover)
            .on_press_tx(|s: &mut State, tx: &mut rune::Tx| {
                if let Some(hit) = s.current_hit() {
                    s.scroll_to = Some(Some(hit.block));
                    tx.scroll_into_view(format!("block-{}", hit.block));
                }
                s.find_focus = false;
                s.find_edit = rune::TextEdit::new("");
                s.hits.clear();
                s.hit_at = 0;
                s.hit_query.clear();
                tx.focus("article", false);
            })
            .child(text("Esc").key("x").font(s.toc_style(400.0)).color(p.meta).a11y_hidden()),
    );

    // The rule sits above the field, not beside it: as a row child it would take the width the
    // field needs and push the whole bar off the right edge.
    col()
        .key("find-bar")
        .w_full()
        .fill(p.chrome_bg)
        .child(super::divider(hair, p.rule))
        .child(row)
}

/// One block, drawn as a row of the virtual list. `index` is the article's block index, which
/// is what a highlight and a selector name.
fn block_view(s: &State, p: Palette, block: &Block, index: usize) -> El {
    let key = format!("block-{index}");
    match block {
        Block::Heading { level, text: heading } => {
            let size = match level {
                1 => 1.55,
                2 => 1.3,
                3 => 1.12,
                _ => 1.0,
            };
            let mut node = rune::text(heading.clone())
                .key("h")
                .w_full()
                .font(
                    rune::TextStyle::new(s.cfg.font_size * size, 700.0)
                        .family(rune::Family::Named(crate::leak(&s.cfg.font_family)))
                        .lh(s.cfg.font_size * size * 1.25),
                )
                .color(p.fg)
                .wrap();
            for h in highlights_for(s, p, index, 0) {
                node = node.text_highlight(h);
            }
            el()
                .key(key)
                .w_full()
                .pt(if *level <= 2 { 18.0 } else { 10.0 })
                .child(node)
        }
        Block::Para { spans } => inline_block(s, p, spans, key, index, 0),
        Block::Quote { spans } => {
            let inner = inline_block(s, p, spans, "quote-body".into(), index, 0);
            row()
                .key(key)
                .w_full()
                .gap(16.0)
                .child(
                    el().key("quote-rule")
                        .w(3.0)
                        .fill(p.quote_bar)
                        .radius(2.0)
                        .self_stretch(),
                )
                .child(el().key("quote-text").grow(1.0).min_w(0.0).child(inner))
        }
        Block::List { ordered, start, items } => {
            let mut list = col().key(key).w_full().gap(8.0);
            for (n, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}.", start + n as u32)
                } else {
                    "•".to_string()
                };
                list = list.child(
                    row()
                        .key(format!("li-{n}"))
                        .w_full()
                        .gap(12.0)
                        .items_start()
                        .child(
                            text(marker)
                                .key("marker")
                                .w(24.0)
                                .font(s.body_style())
                                .color(p.meta)
                                .align(Align::End),
                        )
                        .child(el().key("li-body").grow(1.0).min_w(0.0).child(inline_block(
                            s,
                            p,
                            item,
                            format!("li-{n}-text"),
                            index,
                            n,
                        ))),
                );
            }
            list
        }
        Block::Code { text: code } => el()
            .key(key)
            .w_full()
            .padding(16.0)
            .radius(radius::MD)
            .fill(p.code_bg)
            .child(
                text(code.clone())
                    .key("code")
                    .w_full()
                    .font(rune::TextStyle::new(s.cfg.font_size * 0.82, 400.0).mono().lh(s.cfg.font_size * 1.35))
                    .color(p.fg)
                    .wrap_anywhere(),
            ),
        Block::Rule => el().key(key).w_full().py(12.0).child(super::divider(1.0, p.rule)),
    }
}

/// A paragraph of inline runs, flattened to one string with styling spans over it.
fn inline_block(s: &State, p: Palette, runs: &[Inline], key: String, block: usize, item: usize) -> El {
    let mut content = String::new();
    let mut spans = Vec::new();
    let mut links: Vec<(usize, usize, String)> = Vec::new();

    for run in runs {
        let start = content.len();
        content.push_str(&run.text);
        let end = content.len();
        if start == end {
            continue;
        }
        match &run.kind {
            InlineKind::Text => {}
            InlineKind::Bold => spans.push(Span::new(start, end).bold()),
            InlineKind::Italic => spans.push(Span::new(start, end).italic()),
            InlineKind::Code => spans.push(Span::new(start, end).mono().bg(p.code_bg, 3.0)),
            InlineKind::Link(href) => {
                spans.push(Span::new(start, end).color(p.accent).underline(p.accent));
                links.push((start, end, href.clone()));
            }
        }
    }

    let mut body = rich(content, spans)
        .key(key)
        .w_full()
        .font(s.body_style())
        .color(p.fg)
        .wrap()
        .selectable(true);

    if s.cfg.justified {
        body = body.justify();
    }
    // Search hits are paint-only, so adding them never reshapes the text or shifts a line.
    for h in highlights_for(s, p, block, item) {
        body = body.text_highlight(h);
    }
    for (start, end, href) in links {
        body = body.link(
            rune::Link::new(start..end, href.clone())
                .hover(rune::LinkPaint::new().underline(p.accent))
                .pressed(rune::LinkPaint::new().color(p.meta))
                .focused(rune::LinkPaint::new().ring(p.accent, 2.0)),
        );
    }
    if runs.iter().any(|r| matches!(r.kind, InlineKind::Link(_))) {
        body = body.on_link(|s: &mut State, href: &str, tx: &mut rune::Tx| {
            s.save_place();
            let _ = tx.open_url(href);
        });
    }
    body
}

/// The search highlights falling inside block `block`'s `item`-th drawn string.
///
/// The current hit is painted in the accent; the rest in a wash of it, so the reader can see how
/// many there are on screen without counting the find bar.
fn highlights_for(s: &State, p: Palette, block: usize, item: usize) -> Vec<rune::TextHighlight> {
    if s.hits.is_empty() {
        return Vec::new();
    }
    let current = s.current_hit();
    s.hits
        .iter()
        .filter(|h| h.block == block && h.item == item)
        .map(|h| {
            let bg = if current == Some(*h) {
                p.accent.alpha(0.55)
            } else {
                p.accent.alpha(0.20)
            };
            rune::TextHighlight::new(h.start..h.end, bg)
                .key(format!("hit-{}-{}-{}", h.block, h.item, h.start))
                .radius(2.0)
        })
        .collect()
}
