//! The reader's frame: titlebar, progress bar, and the small shared controls.
//!
//! The titlebar is an overlay pinned to the top of the window rather than the first child of the
//! page's column. That is what makes distraction-free mode honest: the bar fades on its own
//! [`Live`] opacity while the article reclaims its 56 points, instead of leaving an empty strip
//! where the chrome used to be.

pub mod drawer;
pub mod empty;
pub mod reader;
pub mod scrollbar;
pub mod tester;
pub mod toc;

use rune::tok::radius;
use rune::{Align, El, ScrollPos, col, el, row, text};

use crate::State;
use crate::theme::Palette;

/// The titlebar's height, in points. The window's content starts below it unless the
/// distraction-free bar has stepped out of the way.
pub const BAR_H: f32 = 56.0;

/// The header bar: the address field on the left (as Folio's GTK header holds a URL box), and
/// Folio's controls on the right.
pub fn titlebar(s: &State, p: Palette, hair: f32) -> El {
    let left = match (s.view, &s.doc) {
        // Reading: the address, editable in place so a new URL can be typed without going back.
        (crate::View::Reading, Some(_)) => {
            let mut row = row().key("title").center_y().gap(10.0).grow(1.0);
            if s.address_focus {
                row = row.child(
                    rune::text_input(&s.edit, rune::TextStyle::new(s.cfg.ui_font_size * 0.8, 400.0))
                        .key("address")
                        .role("searchbox")
                        .label("Article address")
                        .grow(1.0)
                        .h(32.0)
                        .px(10.0)
                        .radius(radius::SM)
                        .fill(p.field_bg)
                        .border(2.0, p.accent)
                        .caret(p.accent, p.accent.alpha(0.25))
                        .placeholder("https://example.com/article", p.meta)
                        .single_line()
                        .on_edit(|s: &mut State| &mut s.edit)
                        // While the field owns the keyboard, Folio's vim keys must not fire on
                        // letters the reader is typing; only Escape and Enter mean anything.
                        .on_input_key(|s: &mut State, ev: &rune::KeyEvent, tx: &mut rune::Tx| {
                            match ev.key {
                                rune::Key::Escape => {
                                    s.address_focus = false;
                                    s.chrome_panel(false);
                                    tx.focus("article", false);
                                    true
                                }
                                rune::Key::Enter => {
                                    crate::submit_address(s);
                                    crate::kick_off(s);
                                    s.address_focus = false;
                                    s.chrome_panel(false);
                                    tx.focus("article", false);
                                    true
                                }
                                _ => false,
                            }
                        }),
                );
            } else {
                // Not editing: a pressable chip showing the address, which takes the keyboard.
                // It carries a faint field fill so it reads as an input, not a breadcrumb.
                row = row.child(
                    el()
                        .key("address")
                        .role("searchbox")
                        .label("Article address")
                        .focusable(true)
                        .grow(1.0)
                        .h(32.0)
                        .px(10.0)
                        .center_y()
                        .radius(radius::SM)
                        .fill(p.field_bg.alpha(0.5))
                        .hover(p.field_bg)
                        .on_press(|s: &mut State| {
                            s.address_focus = true;
                            s.chrome_panel(true);
                        })
                        .child(
                            text(short_url(&s.edit.text))
                                .key("title-text")
                                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.8, 400.0))
                                .color(p.meta)
                                .truncate()
                                .a11y_hidden(),
                        ),
                );
            }
            row
        }
        _ => row().key("title").center_y().gap(10.0).child(
            text(match s.view {
                crate::View::Library => "Library",
                _ => "Folio",
            })
            .key("title-text")
            .font(s.ui_style(600.0))
            .color(p.chrome_fg)
            .truncate(),
        ),
    };

    let mut right = row().key("controls").center_y().gap(6.0);
    if s.view == crate::View::Reading {
        right = right
            .child(chip(s, p, "toc", "Contents", s.show_toc, |s| {
                let open = !s.show_toc;
                s.show_toc = open;
                s.chrome_panel(open);
            }))
            // The pin is what keeps the bar while the reader looks away. It reads as pressed
            // while distraction-free mode is holding the bar, so the state is visible.
            .child(chip(s, p, "pin", "Pin", s.reader.chrome.is_pinned(), |s| {
                s.chrome_pin();
            }));
    }
    right = right
        .child(chip(s, p, "library", "Library", s.view == crate::View::Library, |s| {
            s.save_place();
            s.view = crate::View::Library;
            s.show_toc = false;
        }))
        .child(chip(s, p, "settings", "Aa", s.show_settings, |s| {
            let open = !s.show_settings;
            s.show_settings = open;
            s.chrome_panel(open);
        }));

    col()
        .key("titlebar")
        .w_full()
        .h(BAR_H)
        .fill(p.chrome_bg)
        .window_drag()
        .child(
            row()
                .key("titlebar-row")
                .w_full()
                .grow(1.0)
                .px(20.0)
                .justify_between()
                .center_y()
                .child(left)
                .child(right),
        )
        .child(divider(hair, p.rule))
}

/// The address without its scheme or trailing slash, which is what a reader wants to see.
fn short_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// A small pressable label in the titlebar.
fn chip(s: &State, p: Palette, key: &'static str, label: &'static str, active: bool, f: fn(&mut State)) -> El {
    el()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(32.0)
        .px(12.0)
        .center_y()
        .radius(radius::SM)
        .hover(p.hover)
        .press(p.hover)
        .fill(if active { p.hover } else { rune::Color::TRANSPARENT })
        .on_press(f)
        .child(
            text(label)
                .key("label")
                .font(s.ui_style(500.0))
                .color(if active { p.accent } else { p.chrome_fg })
                .a11y_hidden(),
        )
}

/// The 3 pt progress bar Folio keeps at the bottom of the window.
///
/// The fill's geometry is a live value: the viewport reports its scroll offset and the track
/// reports its own width, and between them the bar moves without rebuilding the view.
pub fn progress(s: &State, p: Palette) -> El {
    let rect = s.bar_rect.clone();
    let progress = s.progress.clone();
    let track_w = s.track_w.clone();
    el()
        .key("progress")
        .w_full()
        .h(3.0)
        .fill(rune::Color::TRANSPARENT)
        .a11y_hidden()
        .child(
            el().key("progress-track")
                .w_full()
                .h(3.0)
                .fill(rune::Color::TRANSPARENT)
                .on_layout(move |_: &mut State, r: [f32; 4]| {
                    track_w.set(r[2]);
                    rect.set([0.0, 0.0, r[2] * progress.get(), 3.0]);
                }),
        )
        .child(
            el().key("progress-fill")
                .pin(Some(0.0), Some(0.0), None, None)
                .size(0.0, 3.0)
                .fill(p.accent)
                .live_rect(&s.bar_rect),
        )
}

/// The scroll watcher every viewport in the app shares: it feeds the progress bar from the one
/// place the offset is known.
pub fn watch_scroll(s: &State) -> impl Fn(ScrollPos) + 'static {
    let progress = s.progress.clone();
    let rect = s.bar_rect.clone();
    let track_w = s.track_w.clone();
    let offset = s.offset.clone();
    let bar = s.scroll_bar.clone();
    move |at: ScrollPos| {
        offset.set(at.offset[1]);
        let max = at.max[1].max(1.0);
        let frac = (at.offset[1] / max).clamp(0.0, 1.0);
        progress.set(frac);
        rect.set([0.0, 0.0, track_w.get() * frac, 3.0]);
        bar.watch(at);
    }
}

/// A hairline divider.
pub fn divider(hair: f32, c: rune::Color) -> El {
    el().key("divider").h(hair).w_full().fill(c).a11y_hidden()
}

/// Small metadata text, used by the reader header and library rows.
pub fn meta(s: &State, p: Palette, label: impl Into<String>) -> El {
    text(label.into())
        .key("meta")
        .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
        .color(p.meta)
        .align(Align::Start)
        .truncate()
}
