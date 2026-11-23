//! The TOC: a panel that slides in from the left over the reading surface.
//!
//! It always has something in it. The article's own title leads the list, because a serialised
//! chapter's prose carries no `<h*>` at all — the only structure it has is its section breaks,
//! which the extractor now promotes to headings. A list that can be empty is a list a reader
//! learns not to trust.

use rune::tok::radius;
use rune::{El, ScrollSpec, col, el, row, text};

use crate::State;
use crate::theme::Palette;

pub fn overlay(s: &State, p: Palette, hair: f32) -> El {
    let Some(doc) = &s.doc else {
        return el().key("toc-overlay");
    };
    let entries = doc.toc();
    let mut list = col().key("toc-list").w_full().gap(2.0);

    for (level, title, block) in entries {
        // Nested headings indent, so the structure reads at a glance.
        let indent = 12.0 + (level.saturating_sub(1) as f32) * 14.0;
        list = list.child(
            el()
                .key(match block {
                    Some(i) => format!("toc-{i}"),
                    None => "toc-top".to_string(),
                })
                .role("button")
                .label(title.clone())
                .focusable(true)
                .w_full()
                .py(7.0)
                .pl(indent)
                .pr(12.0)
                .radius(radius::SM)
                .hover(p.hover)
                .on_press(move |s: &mut State| {
                    s.scroll_to = Some(block);
                    s.show_toc = false;
                })
                .child({
                    let label = text(title)
                        .key("label")
                        .w_full()
                        .font(s.toc_style(if level <= 2 { 500.0 } else { 400.0 }))
                        .color(if level <= 2 { p.fg } else { p.meta })
                        .a11y_hidden();
                    // The article's own title is long by nature ("Chapter 4: The Present, the
                    // Past, and the Future"), so it wraps instead of truncating: a contents list
                    // whose first line cannot be read is not a contents list.
                    if block.is_none() { label.wrap() } else { label.truncate() }
                }),
        );
    }

    let panel = col()
        .key("toc-panel")
        .w(320.0)
        .fill(p.chrome_bg)
        .border(hair, p.rule)
        .child(
            row()
                .key("toc-head")
                .w_full()
                .h(56.0)
                .px(16.0)
                .center_y()
                .justify_between()
                .child(
                    text("Contents")
                        .key("toc-title")
                        .font(s.ui_style(600.0))
                        .color(p.fg),
                )
                .child(
                    el().key("toc-close")
                        .role("button")
                        .label("Close contents")
                        .focusable(true)
                        .h(28.0)
                        .px(10.0)
                        .center_y()
                        .radius(radius::SM)
                        .hover(p.hover)
                        .on_press(|s: &mut State| s.show_toc = false)
                        .child(text("Esc").key("x").font(s.toc_style(400.0)).color(p.meta).a11y_hidden()),
                ),
        )
        .child(super::divider(hair, p.rule))
        .child(
            col()
                .key("toc-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(8.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        );

    // The scrim eats presses outside the panel, so a tap anywhere closes the TOC. It is dark
    // enough to read as a layer over the article, which is the only cue that the text behind is
    // covered rather than clipped.
    // No explicit height: the pin gives this box the space between the titlebar and the window
    // bottom, and `h_full` here would resolve against the window instead, pushing the panel 56px
    // past the bottom edge.
    row()
        .key("toc-overlay")
        .w_full()
        .child(panel)
        .child(
            el().key("toc-scrim")
                .grow(1.0)
                .fill(rune::Color::BLACK.alpha(0.42))
                .on_press(|s: &mut State| s.show_toc = false),
        )
}
