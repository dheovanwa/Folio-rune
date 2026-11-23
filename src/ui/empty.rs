//! The two states with no article open: the paste-a-link page and the library list.

use rune::tok::radius;
use rune::{Align, El, ScrollSpec, col, el, row, text};

use crate::State;
use crate::theme::Palette;

/// `BlankPage`: Folio's startup. One field, one line of instruction, nothing else.
pub fn view(s: &State, p: Palette, hair: f32) -> El {
    let mut body = col()
        .key("blank")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(20.0)
        .px(40.0);

    body = body.child(
        text("Folio")
            .key("wordmark")
            .font(rune::TextStyle::new(s.cfg.font_size * 2.0, 700.0).lh(s.cfg.font_size * 2.2))
            .color(p.fg)
            .align(Align::Center),
    );
    body = body.child(
        text("Paste a link to read it without the rest of the page.")
            .key("tagline")
            .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.9, 400.0))
            .color(p.meta)
            .align(Align::Center),
    );
    body = body.child(address_field(s, p, hair, 520.0));

    if s.loading {
        body = body.child(
            text("Fetching…")
                .key("loading")
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
                .color(p.accent)
                .align(Align::Center),
        );
    }
    if let Some(err) = &s.error {
        body = body.child(
            text(err.clone())
                .key("error")
                .w(520.0)
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
                .color(rune::Color { r: 0.75, g: 0.15, b: 0.12, a: 1.0 })
                .align(Align::Center)
                .wrap(),
        );
    }
    if !s.library.entries.is_empty() {
        body = body.child(
            el()
                .key("to-library")
                .role("button")
                .label("Open the library")
                .focusable(true)
                .h(34.0)
                .px(14.0)
                .center_y()
                .radius(radius::SM)
                .hover(p.hover)
                .on_press(|s: &mut State| s.view = crate::View::Library)
                .child(
                    text(format!("Library · {} saved", s.library.entries.len()))
                        .key("label")
                        .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 500.0))
                        .color(p.accent)
                        .a11y_hidden(),
                ),
        );
    }

    col()
        .key("blank-surface")
        .w_full()
        .h_full()
        .fill(p.bg)
        .child(body)
        .child(shortcuts(s, p, hair))
}

/// The address field. Rune's `text_input` binds to a `TextEdit`; Folio keeps the raw string in
/// state so the drawer and keys can read it, and mirrors edits back into it.
pub fn address_field(s: &State, p: Palette, hair: f32, width: f32) -> El {
    let focused = s.address_focus;
    let style = rune::TextStyle::new(s.cfg.ui_font_size * 0.9, 400.0);
    let field = rune::text_input(&s.edit, style)
        .key("address")
        .role("searchbox")
        .label("Article address")
        .w(width)
        .h(44.0)
        .padding(14.0)
        .radius(radius::MD)
        .fill(p.field_bg)
        .border(if focused { 2.0 } else { hair }, if focused { p.accent } else { p.rule })
        .caret(p.accent, p.accent.alpha(0.25))
        .placeholder("https://example.com/article", p.meta)
        .single_line()
        .on_edit(|s: &mut State| &mut s.edit)
        .on_submit(|s: &mut State, tx: &mut rune::Tx| {
            crate::submit_address(s);
            crate::kick_off(s);
            s.address_focus = false;
            tx.focus("article", false);
        });

    row()
        .key("address-row")
        .w(width + 2.0)
        .center_y()
        .child(field)
}

/// The keyboard hints Folio shows under the field on the empty page.
fn shortcuts(s: &State, p: Palette, hair: f32) -> El {
    let _ = hair;
    let hint = |keys: &'static str, what: &'static str| -> El {
        row()
            .key(format!("hint-{keys}"))
            .gap(8.0)
            .center_y()
            .child(
                el()
                    .key("keys")
                    .h(24.0)
                    .px(8.0)
                    .center_y()
                    .radius(radius::SM)
                    .fill(p.field_bg)
                    .child(
                        text(keys)
                            .key("k")
                            .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.75, 500.0).mono())
                            .color(p.meta),
                    ),
            )
            .child(
                text(what)
                    .key("w")
                    .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.75, 400.0))
                    .color(p.meta),
            )
    };

    col()
        .key("shortcuts")
        .w_full()
        .py(20.0)
        .gap(10.0)
        .items_center()
        .child(
            row()
                .key("row-1")
                .gap(20.0)
                .center_y()
                .child(hint("/", "address"))
                .child(hint("t", "theme"))
                .child(hint("c", "contents"))
                .child(hint("g", "library")),
        )
        .child(
            row()
                .key("row-2")
                .gap(20.0)
                .center_y()
                .child(hint("j / k", "scroll"))
                .child(hint("s", "settings"))
                .child(hint("esc", "back")),
        )
}

/// The library: saved articles, newest first, with their progress.
pub fn library_view(s: &State, p: Palette, hair: f32) -> El {
    let entries = s.library.sorted();
    let mut list = col().key("library-list").w_full().gap(4.0);

    if entries.is_empty() {
        return col()
            .key("library-empty")
            .w_full()
            .h_full()
            .center()
            .gap(14.0)
            .fill(p.bg)
            .child(
                text("Nothing saved yet")
                    .key("title")
                    .font(rune::TextStyle::new(s.cfg.font_size * 1.3, 600.0))
                    .color(p.fg)
                    .align(Align::Center),
            )
            .child(
                text("Articles you read are kept here, with your place in them.")
                    .key("sub")
                    .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
                    .color(p.meta)
                    .align(Align::Center),
            )
            .child(
                el()
                    .key("start")
                    .role("button")
                    .label("Read something")
                    .focusable(true)
                    .h(38.0)
                    .px(16.0)
                    .center_y()
                    .radius(radius::MD)
                    .fill(p.accent)
                    .on_press(|s: &mut State| s.view = crate::View::Blank)
                    .child(
                        text("Read something")
                            .key("label")
                            .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 500.0))
                            .color(p.bg)
                            .a11y_hidden(),
                    ),
            );
    }

    for entry in entries {
        let url = entry.url.clone();
        let progress = entry.progress;
        let when = crate::library::relative_time(entry.saved_at);
        let mut meta = Vec::new();
        if let Some(site) = &entry.site {
            meta.push(site.clone());
        }
        meta.push(when);
        meta.push(format!("{:.0}% read", progress * 100.0));

        list = list.child(
            el()
                .key(format!("entry-{url}"))
                .role("button")
                .label(entry.title.clone())
                .focusable(true)
                .w_full()
                .py(14.0)
                .px(16.0)
                .radius(radius::MD)
                .hover(p.hover)
                .on_press(move |s: &mut State| {
                    if let Some(e) = s.library.find(&url) {
                        let doc = crate::article::ArticleDoc::from(e.doc.clone());
                        let at = e.progress;
                        s.open(doc);
                        s.progress.set(at);
                    }
                })
                .child(
                    col()
                        .key("entry-body")
                        .w_full()
                        .gap(6.0)
                        .child(
                            text(entry.title.clone())
                                .key("entry-title")
                                .w_full()
                                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.95, 500.0))
                                .color(p.fg)
                                .truncate()
                                .a11y_hidden(),
                        )
                        .child(
                            row()
                                .key("entry-meta")
                                .w_full()
                                .gap(8.0)
                                .child(
                                    text(meta.join(" · "))
                                        .key("meta")
                                        .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.75, 400.0))
                                        .color(p.meta)
                                        .truncate()
                                        .a11y_hidden(),
                                ),
                        )
                        .child(
                            el()
                                .key("entry-track")
                                .w_full()
                                .h(2.0)
                                .radius(1.0)
                                .fill(p.rule)
                                .a11y_hidden()
                                .child(
                                    el()
                                        .key("entry-fill")
                                        .w_frac(progress.clamp(0.0, 1.0))
                                        .h(2.0)
                                        .radius(1.0)
                                        .fill(p.accent)
                                        .a11y_hidden(),
                                ),
                        ),
                ),
        );
    }

    col()
        .key("library-surface")
        .w_full()
        .h_full()
        .fill(p.bg)
        .child(
            row()
                .key("library-head")
                .w_full()
                .h(52.0)
                .px(24.0)
                .center_y()
                .justify_between()
                .child(
                    text("Library")
                        .key("title")
                        .font(rune::TextStyle::new(s.cfg.ui_font_size, 600.0))
                        .color(p.fg),
                )
                .child(
                    el()
                        .key("new")
                        .role("button")
                        .label("New article")
                        .focusable(true)
                        .h(30.0)
                        .px(12.0)
                        .center_y()
                        .radius(radius::SM)
                        .hover(p.hover)
                        .on_press(|s: &mut State| s.view = crate::View::Blank)
                        .child(
                            text("New")
                                .key("label")
                                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.8, 500.0))
                                .color(p.accent)
                                .a11y_hidden(),
                        ),
                ),
        )
        .child(super::divider(hair, p.rule))
        .child(
            col()
                .key("library-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(12.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
}
