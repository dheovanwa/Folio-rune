//! The reading surface: serif prose at a fixed measure, with Folio's block styles.

use rune::tok::radius;
use rune::{Align, El, ScrollSpec, Span, col, el, rich, row, text};

use crate::State;
use crate::article::{Block, Inline, InlineKind};
use crate::theme::Palette;

pub fn view(s: &State, p: Palette, hair: f32) -> El {
    let Some(doc) = &s.doc else {
        return super::empty::view(s, p, hair);
    };
    let measure = s.cfg.max_width;

    let mut article = col().key("article-body").w_full().gap(s.cfg.paragraph_spacing_px);

    // The article's own header: title, byline, reading time.
    article = article.child(
        col()
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
            ),
    );

    for (i, block) in doc.blocks.iter().enumerate() {
        // The H1 is the headline already drawn above; skip the duplicate.
        if i == 0 && matches!(block, Block::Heading { level: 1, .. }) {
            continue;
        }
        article = article.child(block_view(s, p, block, i));
    }

    let scroller = col()
        .key("article")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(super::watch_scroll(s))
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
                        .child(article),
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
            .on_input_change(|s: &mut State, _tx: &mut rune::Tx| s.refresh_hits()),
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
                .on_press(move |s: &mut State| s.step_hit(by))
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
            .on_press(|s: &mut State| {
                s.find_focus = false;
                s.find_edit = rune::TextEdit::new("");
                s.hits.clear();
                s.hit_at = 0;
                s.hit_query.clear();
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
        body = body.on_link(|_: &mut State, href: &str, tx: &mut rune::Tx| {
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
