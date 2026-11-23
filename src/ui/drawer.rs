//! The settings drawer: Folio's reading controls, in the order the original groups them.

use rune::tok::radius;
use rune::{Align, El, ScrollSpec, col, el, row, text};

use crate::State;
use crate::theme::Palette;

pub fn overlay(s: &State, p: Palette, hair: f32) -> El {
    let panel = col()
        .key("settings-panel")
        .w(380.0)
        .h_full()
        .fill(p.chrome_bg)
        .border(hair, p.rule)
        .child(
            row()
                .key("settings-head")
                .w_full()
                .h(56.0)
                .px(20.0)
                .center_y()
                .justify_between()
                .child(
                    text("Reading")
                        .key("settings-title")
                        .font(s.ui_style(600.0))
                        .color(p.fg),
                )
                .child(
                    el().key("settings-close")
                        .role("button")
                        .label("Close settings")
                        .focusable(true)
                        .h(28.0)
                        .px(10.0)
                        .center_y()
                        .radius(radius::SM)
                        .hover(p.hover)
                        .on_press(|s: &mut State| {
                            s.show_settings = false;
                            s.cfg.save();
                        })
                        .child(text("Esc").key("x").font(s.toc_style(400.0)).color(p.meta).a11y_hidden()),
                ),
        )
        .child(super::divider(hair, p.rule))
        .child(
            col()
                .key("settings-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(20.0)
                .pb(32.0)
                .gap(22.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(theme_section(s, p, hair))
                .child(type_section(s, p, hair))
                .child(layout_section(s, p, hair))
                .child(behaviour_section(s, p, hair)),
        );

    row()
        .key("settings-overlay")
        .w_full()
        .h_full()
        .items_start()
        .justify_end()
        .child(
            el().key("settings-scrim")
                .grow(1.0)
                .h_full()
                .fill(rune::Color::BLACK.alpha(0.28))
                .on_press(|s: &mut State| {
                    s.show_settings = false;
                    s.cfg.save();
                }),
        )
        .child(panel)
}

fn section(p: Palette, title: &str, children: Vec<El>) -> El {
    let mut c = col().key(format!("section-{title}")).w_full().gap(10.0);
    c = c.child(
        text(title)
            .key("section-title")
            .font(rune::TextStyle::new(11.5, 600.0))
            .color(p.meta),
    );
    for child in children {
        c = c.child(child);
    }
    c
}

fn theme_section(s: &State, p: Palette, hair: f32) -> El {
    let current = s.cfg.preset();
    let mut swatches = row().key("themes").w_full().gap(8.0);
    for preset in crate::theme::Preset::ALL {
        let pal = preset.palette();
        let selected = preset == current;
        swatches = swatches.child(
            el()
                .key(format!("swatch-{}", preset.name()))
                .role("radio")
                .label(preset.title())
                .checked(selected)
                .focusable(true)
                .grow(1.0)
                .h(64.0)
                .center()
                .radius(radius::MD)
                .fill(pal.bg)
                .border(if selected { 2.0 } else { hair }, if selected { p.accent } else { p.rule })
                .hover(p.hover)
                .on_press(move |s: &mut State| {
                    s.cfg.theme = preset.name().to_string();
                    s.cfg.save();
                })
                .child(
                    text(preset.title())
                        .key("label")
                        .font(rune::TextStyle::new(12.0, 500.0))
                        .color(pal.fg)
                        .a11y_hidden(),
                ),
        );
    }
    section(p, "Theme", vec![swatches])
}

fn type_section(s: &State, p: Palette, hair: f32) -> El {
    section(
        p,
        "Type",
        vec![
            stepper_row(s, p, hair, "Size", &format!("{:.0} pt", s.cfg.font_size), "font-size"),
            stepper_row(s, p, hair, "Line height", &format!("{:.2}", s.cfg.line_height), "line-height"),
            stepper_row(s, p, hair, "Measure", &format!("{:.0} px", s.cfg.max_width), "measure"),
            stepper_row(s, p, hair, "Margin", &format!("{:.0} px", s.cfg.margin), "margin"),
            stepper_row(
                s,
                p,
                hair,
                "Paragraph gap",
                &format!("{:.0} px", s.cfg.paragraph_spacing_px),
                "paragraph",
            ),
            toggle_row(s, p, hair, "Justify text", s.cfg.justified, |s| {
                s.cfg.justified = !s.cfg.justified;
                s.cfg.save();
            }),
        ],
    )
}

fn layout_section(s: &State, p: Palette, hair: f32) -> El {
    section(
        p,
        "Reading",
        vec![
            toggle_row(s, p, hair, "Progress bar", s.cfg.show_progress_bar, |s| {
                s.cfg.show_progress_bar = !s.cfg.show_progress_bar;
                s.cfg.save();
            }),
            toggle_row(s, p, hair, "Smooth scrolling", s.cfg.smooth_scrolling, |s| {
                s.cfg.smooth_scrolling = !s.cfg.smooth_scrolling;
                s.cfg.save();
            }),
            toggle_row(s, p, hair, "Detect links on the clipboard", s.cfg.auto_detect_clipboard, |s| {
                s.cfg.auto_detect_clipboard = !s.cfg.auto_detect_clipboard;
                s.cfg.save();
            }),
        ],
    )
}

fn behaviour_section(s: &State, p: Palette, hair: f32) -> El {
    let recent = s.cfg.startup == crate::config::Startup::RecentPage;
    section(
        p,
        "On launch",
        vec![
            toggle_row(s, p, hair, "Reopen the last page", recent, |s| {
                s.cfg.startup = if s.cfg.startup == crate::config::Startup::RecentPage {
                    crate::config::Startup::BlankPage
                } else {
                    crate::config::Startup::RecentPage
                };
                s.cfg.save();
            }),
            el()
                .key("reset")
                .role("button")
                .label("Reset reading settings")
                .focusable(true)
                .w_full()
                .h(40.0)
                .px(12.0)
                .center_y()
                .radius(radius::SM)
                .hover(p.hover)
                .on_press(|s: &mut State| {
                    let startup = s.cfg.startup;
                    let theme = s.cfg.theme.clone();
                    s.cfg = crate::config::Config::default();
                    s.cfg.startup = startup;
                    s.cfg.theme = theme;
                    s.cfg.save();
                })
                .child(
                    text("Reset reading settings")
                        .key("label")
                        .font(rune::TextStyle::new(13.0, 400.0))
                        .color(p.meta)
                        .a11y_hidden(),
                ),
        ],
    )
}

/// A label with −/+ buttons that step a numeric setting.
fn stepper_row(s: &State, p: Palette, hair: f32, label: &'static str, value: &str, which: &'static str) -> El {
    row()
        .key(format!("step-{which}"))
        .w_full()
        .h(40.0)
        .center_y()
        .justify_between()
        .child(
            text(label)
                .key("label")
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.85, 400.0))
                .color(p.fg),
        )
        .child(
            row()
                .key("controls")
                .gap(6.0)
                .center_y()
                .child(
                    step_button(s, p, hair, "−", which, -1.0),
                )
                .child(
                    text(value.to_string())
                        .key("value")
                        .w(64.0)
                        .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.8, 500.0))
                        .color(p.meta)
                        .align(Align::Center),
                )
                .child(step_button(s, p, hair, "+", which, 1.0)),
        )
}

fn step_button(s: &State, p: Palette, hair: f32, glyph: &'static str, which: &'static str, dir: f32) -> El {
    el()
        .key(format!("step-{which}-{glyph}"))
        .role("button")
        .label(if dir > 0.0 { format!("Increase {which}") } else { format!("Decrease {which}") })
        .focusable(true)
        .size(28.0, 28.0)
        .center()
        .radius(radius::SM)
        .fill(p.field_bg)
        .border(hair, p.rule)
        .hover(p.hover)
        .on_press(move |s: &mut State| {
            let c = &mut s.cfg;
            match which {
                "font-size" => c.font_size = (c.font_size + dir).clamp(12.0, 34.0),
                "line-height" => c.line_height = ((c.line_height * 100.0 + dir * 5.0).round() / 100.0).clamp(1.2, 2.4),
                "measure" => c.max_width = (c.max_width + dir * 40.0).clamp(400.0, 1200.0),
                "margin" => c.margin = (c.margin + dir * 8.0).clamp(8.0, 120.0),
                "paragraph" => c.paragraph_spacing_px = (c.paragraph_spacing_px + dir * 4.0).clamp(0.0, 48.0),
                _ => {}
            }
            c.save();
        })
        .child(
            text(glyph)
                .key("glyph")
                .font(rune::TextStyle::new(s.cfg.ui_font_size * 0.95, 500.0))
                .color(p.fg)
                .a11y_hidden(),
        )
}

fn toggle_row(s: &State, p: Palette, hair: f32, label: &'static str, on: bool, flip: fn(&mut State)) -> El {
    let _ = s;
    row()
        .key(format!("toggle-{label}"))
        .role("switch")
        .label(label)
        .checked(on)
        .focusable(true)
        .w_full()
        .h(40.0)
        .center_y()
        .justify_between()
        .radius(radius::SM)
        .hover(p.hover)
        .on_press(flip)
        .child(
            text(label)
                .key("label")
                .font(rune::TextStyle::new(13.0, 400.0))
                .color(p.fg)
                .a11y_hidden(),
        )
        .child(toggle(p, hair, on).a11y_hidden())
}

fn toggle(p: Palette, hair: f32, on: bool) -> El {
    let (w, h, k) = (36.0, 20.0, 16.0);
    let inset = (h - k) / 2.0;
    el()
        .key("switch")
        .size(w, h)
        .radius(h / 2.0)
        .fill(if on { p.accent } else { p.field_bg })
        .border(hair, p.rule)
        .child(
            el()
                .key("knob")
                .size(k, k)
                .absolute(if on { w - k - inset } else { inset }, inset)
                .radius(k / 2.0)
                .fill(rune::Color::WHITE),
        )
}
