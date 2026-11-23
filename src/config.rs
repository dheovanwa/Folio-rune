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
    pub fn load() -> Config {
        let Some(path) = Self::config_path() else { return Config::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Config::default() };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::config_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}
