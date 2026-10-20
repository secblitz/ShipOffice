//! User settings, kept as TOML in the platform config folder
//! (`~/.config/shipoffice`, `~/Library/Application Support/dev.shipoffice.ShipOffice`, `%APPDATA%\shipoffice\ShipOffice\config`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// A session gets a tab while its files changed within this many minutes.
    pub active_minutes: u64,
    pub max_tabs: usize,
    /// Sprite animation rate. The sheets have 16 frames per loop.
    pub anim_fps: u32,
    /// Speech bubbles and thought clouds over the agents.
    pub bubbles: bool,
    /// Fill the office with pretend agents when no harness has a live session.
    pub demo_when_empty: bool,
    pub show_fps: bool,
    /// Per harness: on/off and extra folders to watch, keyed by adapter id (`claude-code`, `codex`...).
    pub harness: BTreeMap<String, HarnessConfig>,
    /// Pin an office layout: session key (or, for every session of it, a project folder name) -> layout seed.
    pub layouts: BTreeMap<String, u32>,
    pub appearance: Appearance,
    /// The live feed panel is open.
    pub feed_open: bool,
    /// After a drag, ease the view to the nearest corner angle, where the art reads cleanly.
    pub snap_view: bool,
    /// A desktop notification when an agent waits on you, while the window is in the background.
    pub notify_needs: bool,
    /// And when a session finishes its turn.
    pub notify_done: bool,
    /// Minutes of nothing before an agent that should be working counts as stuck.
    pub stuck_minutes: u64,
    /// Characters and props you added (see docs/custom.md).
    pub custom: Custom,
}

/// What of your own content is in use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Custom {
    /// The built-in crew joins yours; off, only your characters work here.
    pub builtin_characters: bool,
    /// The character the lead is, by its folder name.
    pub lead: Option<String>,
    /// Characters and props you switched off, by folder or file name.
    pub off: Vec<String>,
}

impl Default for Custom {
    fn default() -> Self {
        Self { builtin_characters: true, lead: None, off: Vec::new() }
    }
}

/// How the HUD looks: a preset, plus any colours you changed on top of it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    /// Preset id (`graphite`, `paper`, `midnight`...).
    pub theme: String,
    /// Colour overrides as `#rrggbb`, keyed by role (`accent`, `surface`, `text`...).
    pub colors: BTreeMap<String, String>,
    /// Use one office style for every project instead of a procedural one (theme name).
    pub office_style: Option<String>,
    /// What surrounds the office.
    pub backdrop: Backdrop,
}

/// The office as a floor of a city tower, or on its own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backdrop {
    /// the city, by day on light themes and by night on dark ones
    #[default]
    City,
    Day,
    Night,
    None,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { theme: "graphite".into(), colors: BTreeMap::new(), office_style: None, backdrop: Backdrop::City }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HarnessConfig {
    pub enabled: bool,
    /// Watched in addition to the harness's default folders.
    pub extra_paths: Vec<PathBuf>,
    /// Watch only these instead of the defaults.
    pub replace_defaults: bool,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self { enabled: true, extra_paths: Vec::new(), replace_defaults: false }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            active_minutes: 45,
            max_tabs: 8,
            anim_fps: 16,
            bubbles: true,
            demo_when_empty: true,
            show_fps: false,
            harness: BTreeMap::new(),
            layouts: BTreeMap::new(),
            appearance: Appearance::default(),
            feed_open: false,
            snap_view: true,
            notify_needs: true,
            notify_done: false,
            stuck_minutes: 8,
            custom: Custom::default(),
        }
    }
}

impl Config {
    /// `SHIPOFFICE_CONFIG` points somewhere else (handy for demos and tests).
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("SHIPOFFICE_CONFIG").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        directories::ProjectDirs::from("dev", "shipoffice", "ShipOffice").map(|d| d.config_dir().join("config.toml"))
    }

    /// Reads the config, falling back to defaults when it is missing or broken (a broken file is left alone).
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(p) = Self::path() else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, p)
    }

    pub fn harness(&self, id: &str) -> HarnessConfig {
        self.harness.get(id).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let c: Config = toml::from_str("anim_fps = 24\n[harness.codex]\nenabled = false\n").unwrap();
        assert_eq!(c.anim_fps, 24);
        assert_eq!(c.max_tabs, 8);
        assert!(!c.harness("codex").enabled);
        assert!(c.harness("claude-code").enabled);
        let round: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(round, c);
    }
}
