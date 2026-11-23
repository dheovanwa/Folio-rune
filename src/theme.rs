//! Folio's palette: the five reading themes from the original GTK app, as Rune colours.

use rune::Color;

/// sRGB transfer function, the inverse of what `Color` stores (linear light).
fn lin(c: f32) -> f32 {
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// `#rrggbb` in sRGB to a linear [`Color`].
fn hexc(s: &str) -> Color {
    let h = s.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0;
    Color { r: lin(p(0)), g: lin(p(2)), b: lin(p(4)), a: 1.0 }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Light,
    Sepia,
    Dark,
    Charcoal,
    Oled,
}

impl Preset {
    /// Folio's cycle order: `theme_presets()` in `config.rs`.
    pub const ALL: [Preset; 5] = [Preset::Light, Preset::Sepia, Preset::Dark, Preset::Charcoal, Preset::Oled];

    pub fn from_name(name: &str) -> Preset {
        match name {
            "sepia" => Preset::Sepia,
            "dark" => Preset::Dark,
            "charcoal" => Preset::Charcoal,
            "oled" => Preset::Oled,
            _ => Preset::Light,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Light => "light",
            Preset::Sepia => "sepia",
            Preset::Dark => "dark",
            Preset::Charcoal => "charcoal",
            Preset::Oled => "oled",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Preset::Light => "Paper",
            Preset::Sepia => "Sepia",
            Preset::Dark => "Dark",
            Preset::Charcoal => "Charcoal",
            Preset::Oled => "OLED",
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(self, Preset::Dark | Preset::Charcoal | Preset::Oled)
    }

    pub fn next(self) -> Preset {
        match self {
            Preset::Light => Preset::Sepia,
            Preset::Sepia => Preset::Dark,
            Preset::Dark => Preset::Charcoal,
            Preset::Charcoal => Preset::Oled,
            Preset::Oled => Preset::Light,
        }
    }

    /// The exact values from Folio's `bg_color`/`fg_color`/`accent_color`/`meta_color`/
    /// `headerbar_bg`, plus the derived tints the chrome needs.
    pub fn palette(self) -> Palette {
        let (bg, fg, accent, meta, chrome) = match self {
            Preset::Light => ("#faf8f5", "#1a1a1a", "#3584e4", "#6b7280", "#f0ede8"),
            Preset::Sepia => ("#f4ecd8", "#5b4636", "#8b6914", "#8b7355", "#e8dfc8"),
            Preset::Dark => ("#1e1e2e", "#cdd6f4", "#89b4fa", "#a6adc8", "#181825"),
            Preset::Charcoal => ("#161616", "#e4e4e4", "#5dade2", "#9a9a9a", "#1c1c1c"),
            Preset::Oled => ("#000000", "#e0e0e0", "#bb86fc", "#999999", "#000000"),
        };
        let bg = hexc(bg);
        let fg = hexc(fg);
        let accent = hexc(accent);
        Palette {
            bg,
            fg,
            accent,
            meta: hexc(meta),
            chrome_bg: hexc(chrome),
            chrome_fg: fg,
            rule: fg.alpha(0.14),
            code_bg: fg.alpha(0.07),
            field_bg: bg.mix(fg, 0.05),
            hover: fg.alpha(0.08),
            quote_bar: accent.alpha(0.55),
            bg_hex: hexc(bg_hex_of(self)),
        }
    }
}

fn bg_hex_of(p: Preset) -> &'static str {
    match p {
        Preset::Light => "#faf8f5",
        Preset::Sepia => "#f4ecd8",
        Preset::Dark => "#1e1e2e",
        Preset::Charcoal => "#161616",
        Preset::Oled => "#000000",
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub meta: Color,
    pub chrome_bg: Color,
    pub chrome_fg: Color,
    pub rule: Color,
    pub code_bg: Color,
    pub field_bg: Color,
    pub hover: Color,
    pub quote_bar: Color,
    /// The page colour again, opaque, for the chrome's own fills.
    pub bg_hex: Color,
}
