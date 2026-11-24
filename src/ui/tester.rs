//! The Live Selector Tester: type a Rune selector, see whether it resolves and where.
//!
//! The panel is deliberately small and lives over the reading surface rather than in the
//! settings drawer, because the thing it tests is the article's own element keys: a row a
//! virtual list has not built yet is exactly the case worth looking at, and that is invisible
//! from another window.
//!
//! The answer comes from `Cx::layout`, the same read `Tx::scroll_into_view` and `Cx::layout`
//! take, so the tester reports what the app itself would do with the string.

use rune::tok::radius;
use rune::{El, col, el, row, text};

use crate::reader::selector::{self, Selector};
use crate::State;
use crate::theme::Palette;

/// The panel's width. Narrow enough to sit over a narrow window, wide enough for a key path
/// without truncating it.
pub const PANEL_W: f32 = 300.0;

/// The tester panel, with the probe it was asked for resolved against the live layout.
///
/// `cx` is the only place a rect exists, so the probe is read here and reported in the panel
/// instead of being stored in state: a stored rect would be one commit stale and would re-run
/// the view on every scroll.
pub fn panel(s: &State, p: Palette, hair: f32, cx: &mut rune::Cx<State>) -> El {
    let tester = &s.reader.tester;
    let parsed = Selector::parse(&tester.edit.text);

    // The probe is the selector last asked about, not every keystroke: reading a rect on each
    // keystroke would make typing in the field re-run the view for a key nobody finished.
    let probe_sel = tester.probe.clone().unwrap_or_default();
    let rect = if probe_sel.is_empty() { None } else { cx.layout(&probe_sel) };

    let message = match (&parsed, tester.probe.is_some()) {
        (Err(e), false) => e.message().to_string(),
        (_, false) if tester.edit.text.trim().is_empty() => {
            "Press Probe, or Enter, to ask about a key.".to_string()
        }
        _ => selector::describe(rect, Some(s.reader.visible_rows)),
    };
    let found = rect.is_some();
    let scoped = parsed.as_ref().map(|q| q.is_path()).unwrap_or(false);

    let mut body = col().key("tester-body").w_full().gap(10.0).child(
        rune::text_input(&tester.edit, rune::TextStyle::new(s.cfg.ui_font_size * 0.75, 400.0))
            .key("tester-field")
            .role("searchbox")
            .label("Selector")
            .w_full()
            .h(30.0)
            .px(8.0)
            .radius(radius::SM)
            .fill(p.field_bg)
            .border(if found { 1.5 } else { hair }, if found { p.accent } else { p.rule })
            .caret(p.accent, p.accent.alpha(0.25))
            .placeholder("block-12/quote-body", p.meta)
            .single_line()
            .on_edit(|s: &mut State| &mut s.reader.tester.edit)
            .on_input_key(|s: &mut State, ev: &rune::KeyEvent, tx: &mut rune::Tx| match ev.key {
                rune::Key::Enter => {
                    probe(s);
                    tx.focus("article", false);
                    true
                }
                rune::Key::Escape => {
                    s.reader.tester.open = false;
                    tx.focus("article", false);
                    true
                }
                _ => false,
            }),
    );

    body = body.child(
        row()
            .key("tester-answer")
            .w_full()
            .gap(6.0)
            .items_start()
            .child(
                el().key("tester-dot")
                    .w(6.0)
                    .h(6.0)
                    .mt(7.0)
                    .radius(3.0)
                    .fill(if found { p.accent } else { p.meta.alpha(0.45) }),
            )
            .child(
                text(message)
                    .key("tester-message")
                    .grow(1.0)
                    .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.72, 400.0).mono())
                    .color(if found { p.fg } else { p.meta })
                    .wrap(),
            ),
    );

    if scoped {
        body = body.child(
            text("Scoped: the first segments name the nearest keyed ancestors.")
                .key("tester-scope-note")
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.66, 400.0))
                .color(p.meta)
                .wrap(),
        );
    }

    // One press for each row the list has built, so the tester is usable without typing a key
    // the reader cannot see. Bounded: a page of buttons is a page of layout.
    let nearby = selector::nearby_rows(
        s.reader.visible_rows.0,
        s.reader.visible_rows.1,
        s.doc.as_ref().map_or(0, |d| d.blocks.len()),
        4,
    );
    if !nearby.is_empty() {
        let mut chips = row().key("tester-nearby").w_full().gap(6.0);
        for key in nearby {
            let pressed = tester.probe.as_deref() == Some(key.as_str());
            let k = key.clone();
            chips = chips.child(
                el()
                    .key(format!("tester-chip-{key}"))
                    .role("button")
                    .label(format!("Probe {key}"))
                    .focusable(true)
                    .h(24.0)
                    .px(7.0)
                    .center_y()
                    .radius(radius::SM)
                    .fill(if pressed { p.hover } else { p.field_bg })
                    .hover(p.hover)
                    .on_press(move |s: &mut State| {
                        s.reader.tester.edit = rune::TextEdit::new(k.clone());
                        s.reader.tester.probe = Some(k.clone());
                    })
                    .child(
                        text(key)
                            .key("label")
                            .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.62, 400.0).mono())
                            .color(if pressed { p.accent } else { p.meta })
                            .a11y_hidden(),
                    ),
            );
        }
        body = body.child(chips);
    }

    col()
        .key("tester-panel")
        .w(PANEL_W)
        .padding(12.0)
        .gap(10.0)
        .radius(radius::MD)
        .fill(p.chrome_bg)
        .border(hair, p.rule)
        .shadow(6.0, 24.0, rune::Color::BLACK.alpha(0.28))
        .child(
            row()
                .key("tester-head")
                .w_full()
                .center_y()
                .justify_between()
                .child(
                    text("Selector Tester")
                        .key("tester-title")
                        .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.78, 600.0))
                        .color(p.fg),
                )
                .child(
                    el()
                        .key("tester-close")
                        .role("button")
                        .label("Close the selector tester")
                        .focusable(true)
                        .h(24.0)
                        .px(8.0)
                        .center_y()
                        .radius(radius::SM)
                        .hover(p.hover)
                        .on_press(|s: &mut State| s.reader.tester.open = false)
                        .child(text("Esc").key("x").font(s.toc_style(400.0)).color(p.meta).a11y_hidden()),
                ),
        )
        .child(body)
        .child(
            row().key("tester-actions").w_full().justify_end().child(
                el()
                    .key("tester-probe")
                    .role("button")
                    .label("Probe the selector")
                    .focusable(true)
                    .h(26.0)
                    .px(10.0)
                    .center_y()
                    .radius(radius::SM)
                    .fill(p.accent)
                    .hover(p.accent.alpha(0.85))
                    .on_press(|s: &mut State| probe(s))
                    .child(
                        text("Probe")
                            .key("label")
                            .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.7, 600.0))
                            .color(p.bg)
                            .a11y_hidden(),
                    ),
            ),
        )
}

/// Ask about the field's current text. A malformed selector is recorded too, so the panel can
/// print what is wrong with it instead of silently doing nothing.
pub fn probe(s: &mut State) {
    let raw = s.reader.tester.edit.text.trim().to_string();
    s.reader.tester.probe = if raw.is_empty() { None } else { Some(raw) };
}

/// Whether the keys that open the tester are held. One function, so the app's key handler and
/// the tester's own hint cannot drift apart: Ctrl+Shift+D (`Cmd+Shift+D` on macOS).
pub fn is_open_chord(ev: &rune::KeyEvent) -> bool {
    ev.mods.shift
        && (ev.is_command('d')
            || matches!(&ev.key, rune::Key::Command('d'))
            || matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("d") && (ev.mods.cmd || ev.mods.ctrl)))
}
