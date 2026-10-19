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
    pub channel: Option<Channel>,
}

impl Settings {
    pub fn channel(&self, app: &AppHandle) -> Channel {
        self.channel
            .unwrap_or_else(|| Channel::from_version(&app.package_info().version.to_string()))
    }
}

/// Reads the settings file. A missing or broken file gives the defaults, so a
/// bad edit can never stop the app from starting.
pub fn load(app: &AppHandle) -> Settings {
    let Ok(dir) = app.path().app_config_dir() else {
        return Settings::default();
    };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
