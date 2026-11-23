//! Folio's settings: the same fields, defaults and preset lists as the original GTK app,
//! persisted as TOML under the XDG config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::Preset;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub light_theme: String,
    pub dark_theme: String,
    pub font_family: String,
    pub font_size: f32,
    pub ui_font_size: f32,
    pub toc_font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub max_width: f32,
    pub margin: f32,
    pub justified: bool,
    pub auto_scroll_speed: f32,
    pub auto_detect_clipboard: bool,
    pub show_progress_bar: bool,
    pub smooth_scrolling: bool,
    pub scroll_step_px: f32,
    pub paragraph_spacing_px: f32,
    pub show_toc: bool,
    pub window_width: f32,
    pub window_height: f32,
    pub startup: Startup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Startup {
    BlankPage,
    RecentPage,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "light".into(),
            light_theme: "light".into(),
            dark_theme: "dark".into(),
            // The original asks for Times New Roman; this machine has no such face, so the
            // list falls through to the serif families that are installed.
            font_family: "Liberation Serif, DejaVu Serif".into(),
            font_size: 18.0,
            ui_font_size: 20.0,
            toc_font_size: 18.0,
            line_height: 1.7,
            letter_spacing: 0.0,
            max_width: 720.0,
            margin: 32.0,
            justified: false,
            auto_scroll_speed: 30.0,
            auto_detect_clipboard: true,
            show_progress_bar: true,
            smooth_scrolling: true,
            scroll_step_px: 120.0,
            paragraph_spacing_px: 16.0,
            show_toc: false,
            window_width: 900.0,
            window_height: 720.0,
            startup: Startup::BlankPage,
        }
    }
}

impl Config {
    /// `theme_presets()`: every preset, in the order the T cycles through them.
    pub const PRESETS: [&'static str; 5] = ["light", "sepia", "dark", "charcoal", "oled"];

    pub fn preset(&self) -> Preset {
        Preset::from_name(&self.theme)
    }

    pub fn cycle_theme(&mut self) {
        self.theme = self.preset().next().name().to_string();
    }

    pub fn font_pt(&self) -> f32 {
        self.font_size
    }

    /// Line height in points, resolved from the multiplier the way CSS `line-height` is.
    pub fn line_px(&self) -> f32 {
        self.font_size * self.line_height
    }

    pub fn config_path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("folio").join("config.toml"))
    }

    /// Missing or unreadable settings fall back to the defaults rather than failing to start.
    ///
    /// A file that parses can still hold values the reader cannot use — a font size of 0, a
    /// measure wider than the window — so it is validated before it is returned.
    pub fn load() -> Config {
        let Some(path) = Self::config_path() else {
            return Config::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        let mut cfg: Config = toml::from_str(&text).unwrap_or_default();
        cfg.validate();
        cfg
    }

    /// Clamp every value into the range the reader can draw, and report what had to move.
    ///
    /// The settings drawer keeps its own controls in range, but `config.toml` is a plain file a
    /// reader may edit by hand, and one bad value there should cost one setting rather than the
    /// whole layout. Each note names the setting, the value it held, and what replaced it.
    pub fn validate(&mut self) -> Vec<String> {
        let mut notes = Vec::new();

        // The theme name indexes a fixed preset list, so an unknown one draws with no palette.
        if !Self::PRESETS.contains(&self.theme.as_str()) {
            notes.push(format!(
                "theme {:?} is not a preset; using \"light\"",
                self.theme
            ));
            self.theme = "light".into();
        }
        if !Self::PRESETS.contains(&self.light_theme.as_str()) {
            notes.push(format!(
                "light_theme {:?} is not a preset; using \"light\"",
                self.light_theme
            ));
            self.light_theme = "light".into();
        }
        if !Self::PRESETS.contains(&self.dark_theme.as_str()) {
            notes.push(format!(
                "dark_theme {:?} is not a preset; using \"dark\"",
                self.dark_theme
            ));
            self.dark_theme = "dark".into();
        }
        if self.font_family.trim().is_empty() {
            notes.push("font_family is empty; using the default serif list".into());
            self.font_family = Config::default().font_family;
        }

        clamp(&mut self.font_size, 8.0, 72.0, "font_size", &mut notes);
        clamp(
            &mut self.ui_font_size,
            8.0,
            72.0,
            "ui_font_size",
            &mut notes,
        );
        clamp(
            &mut self.toc_font_size,
            8.0,
            72.0,
            "toc_font_size",
            &mut notes,
        );
        clamp(&mut self.line_height, 1.0, 3.0, "line_height", &mut notes);
        clamp(
            &mut self.letter_spacing,
            -2.0,
            8.0,
            "letter_spacing",
            &mut notes,
        );
        clamp(&mut self.max_width, 320.0, 2400.0, "max_width", &mut notes);
        clamp(&mut self.margin, 0.0, 240.0, "margin", &mut notes);
        clamp(
            &mut self.auto_scroll_speed,
            0.0,
            1000.0,
            "auto_scroll_speed",
            &mut notes,
        );
        clamp(
            &mut self.scroll_step_px,
            16.0,
            2000.0,
            "scroll_step_px",
            &mut notes,
        );
        clamp(
            &mut self.paragraph_spacing_px,
            0.0,
            200.0,
            "paragraph_spacing_px",
            &mut notes,
        );
        clamp(
            &mut self.window_width,
            520.0,
            10000.0,
            "window_width",
            &mut notes,
        );
        clamp(
            &mut self.window_height,
            400.0,
            10000.0,
            "window_height",
            &mut notes,
        );

        notes
    }

    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

/// Clamp one setting into range, recording a note when it had to move.
///
/// A non-finite value (NaN from a hand-edited `f32`) cannot be clamped by comparison, so it is
/// replaced by the range's low end rather than being left to draw nothing.
fn clamp(value: &mut f32, lo: f32, hi: f32, name: &str, notes: &mut Vec<String>) {
    let before = *value;
    let fixed = if before.is_finite() {
        before.clamp(lo, hi)
    } else {
        lo
    };
    if fixed != before {
        notes.push(format!(
            "{name} {before} is outside {lo}..{hi}; using {fixed}"
        ));
        *value = fixed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_validate_without_changing_anything() {
        let mut cfg = Config::default();
        assert!(
            cfg.validate().is_empty(),
            "defaults must already be in range"
        );
    }

    #[test]
    fn a_hand_edited_value_is_clamped_and_reported() {
        let mut cfg = Config::default();
        cfg.font_size = 0.0;
        cfg.line_height = 9.0;
        cfg.theme = "solarized".into();
        cfg.max_width = -10.0;
        let notes = cfg.validate();
        assert_eq!(cfg.font_size, 8.0);
        assert_eq!(cfg.line_height, 3.0);
        assert_eq!(cfg.theme, "light");
        assert_eq!(cfg.max_width, 320.0);
        assert_eq!(notes.len(), 4, "one note per value: {notes:?}");
        assert!(notes.iter().any(|n| n.contains("font_size")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("solarized")), "{notes:?}");
    }

    #[test]
    fn a_nan_value_falls_to_the_low_end() {
        let mut cfg = Config::default();
        cfg.letter_spacing = f32::NAN;
        cfg.validate();
        assert_eq!(cfg.letter_spacing, -2.0);
    }

    #[test]
    fn an_empty_font_list_returns_to_the_default() {
        let mut cfg = Config::default();
        cfg.font_family = "   ".into();
        cfg.validate();
        assert_eq!(cfg.font_family, Config::default().font_family);
    }

    #[test]
    fn a_valid_file_round_trips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let mut back: Config = toml::from_str(&text).unwrap();
        assert!(
            back.validate().is_empty(),
            "what save writes must load cleanly"
        );
        assert_eq!(back.font_size, cfg.font_size);
        assert_eq!(back.theme, cfg.theme);
    }
}
