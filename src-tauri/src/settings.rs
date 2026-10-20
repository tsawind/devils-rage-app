//! The app's own settings, kept in `settings.json` in the app's config folder
//! (Windows: %APPDATA%\com.tsawind.devilsrage\settings.json).
//! Phase 0.3 adds the settings screen; for now the file can be edited by hand.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
    Dev,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
            Channel::Dev => "dev",
        }
    }

    /// With no channel chosen, follow the build that was installed:
    /// 0.2.1-dev.14 stays on dev, 0.2.0-beta.1 on beta, 0.2.0 on stable.
    pub fn from_version(version: &str) -> Channel {
        if version.contains("-dev") {
            Channel::Dev
        } else if version.contains("-beta") {
            Channel::Beta
        } else {
            Channel::Stable
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Release channel to update from. Missing = follow the installed build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<Channel>,
    /// "mapper", "seat", "accounts" or "pi" = skip the start screen and open that. Missing = start screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_page: Option<String>,
    /// Where the scanner pop-out was last: [x, y, width, height] in screen pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanner_bounds: Option<[i32; 4]>,
    /// Each pop-out's spot, by the window it pops out of ("main", "pophost-<character>").
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub scanner_bounds_by: std::collections::BTreeMap<String, [i32; 4]>,
    /// Scanner pop-out stays on top of other windows (EVE). Missing = yes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pop_out_on_top: Option<bool>,
    /// Main window: [x, y, width, height] in screen pixels, and maximised or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_bounds: Option<[i32; 4]>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub main_maximized: bool,
    /// The last mapper / SeAT page you were on (Mapper and SeAT reopen it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_mapper_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seat_page: Option<String>,
    /// Instant location from EVE's Local chat logs. Missing = on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_log_location: Option<bool>,
    /// Paste probe scans copied in EVE into the right character's system. Missing = on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard_scans: Option<bool>,
    /// Patch 50: the pilot's EVE computer in Parsec (its ID), for "Connect to my EVE PC" on
    /// the PI screen. Missing = Parsec just opens and you pick the computer there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsec_peer_id: Option<String>,
    /// Show the developer section in Settings.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub developer: bool,
}

impl Settings {
    pub fn chat_log_location(&self) -> bool {
        self.chat_log_location.unwrap_or(true)
    }

    pub fn clipboard_scans(&self) -> bool {
        self.clipboard_scans.unwrap_or(true)
    }

    pub fn pop_out_on_top(&self) -> bool {
        self.pop_out_on_top.unwrap_or(true)
    }

    pub fn channel(&self, app: &AppHandle) -> Channel {
        self.channel
            .unwrap_or_else(|| Channel::from_version(&app.package_info().version.to_string()))
    }
}

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("settings.json"))
}

/// Reads the settings file. A missing or broken file gives the defaults, so a
/// bad edit can never stop the app from starting.
pub fn load(app: &AppHandle) -> Settings {
    path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Deletes the settings file (Settings → Reset the app).
pub fn delete(app: &AppHandle) {
    if let Some(p) = path(app) {
        let _ = std::fs::remove_file(p);
    }
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let p = path(app).ok_or("no config folder")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(p, text).map_err(|e| e.to_string())
}
