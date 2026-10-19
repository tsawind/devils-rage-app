//! The app's settings for everyone, set by directors on the mapper's App admin page
//! (mapper Patch 46). Read from MAPPER_URL/app/config at start and every 30 minutes;
//! nothing in it is secret, so no login is needed.
//!
//! - motd: shown on the start screen
//! - min_version: older apps are asked to update
//! - features: switches; off turns that app feature off for everyone

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub motd: Option<String>,
    pub min_version: Option<String>,
    #[serde(default)]
    pub features: HashMap<String, bool>,
}

static CONFIG: Mutex<Option<AppConfig>> = Mutex::new(None);
static ASKED_TO_UPDATE: AtomicBool = AtomicBool::new(false);
const EVERY: Duration = Duration::from_secs(30 * 60);

pub fn current() -> Option<AppConfig> {
    CONFIG.lock().unwrap().clone()
}

/// A feature directors haven't switched off (on when the mapper can't be reached).
pub fn feature_on(name: &str) -> bool {
    CONFIG
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.features.get(name).copied())
        .unwrap_or(true)
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            match fetch().await {
                Ok(config) => {
                    log::info!(
                        "app settings from the mapper: min version {:?}, features {:?}",
                        config.min_version,
                        config.features
                    );
                    *CONFIG.lock().unwrap() = Some(config.clone());
                    check_min_version(&app, &config);
                }
                Err(err) => log::warn!("couldn't read the app settings from the mapper: {err}"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

async fn fetch() -> Result<AppConfig, String> {
    let url = format!("{}/app/config", crate::MAPPER_URL);
    let response = reqwest::Client::new()
        .get(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the mapper answered {}", response.status()));
    }
    response
        .json::<AppConfig>()
        .await
        .map_err(|e| e.to_string())
}

/// Below the directors' minimum: say so once per run and look for the update.
/// Test builds (0.1.3-dev.20) count as 0.1.3, so dev testers aren't nagged.
fn check_min_version(app: &AppHandle, config: &AppConfig) {
    let Some(min) = config.min_version.as_deref().and_then(core_version) else {
        return;
    };
    let Some(mine) = core_version(&app.package_info().version.to_string()) else {
        return;
    };
    if mine >= min || ASKED_TO_UPDATE.swap(true, Ordering::Relaxed) {
        return;
    }
    log::warn!("this app is older than the corp's minimum version {min:?}");
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
        let app2 = app.clone();
        app.dialog()
            .message(format!(
                "Your corp asks everyone to use Devil's Rage {} or newer. You have {}.\n\nThe app will look for the update now.",
                config.min_version.clone().unwrap_or_default(),
                app.package_info().version
            ))
            .title("Please update")
            .kind(MessageDialogKind::Warning)
            .show(move |_| crate::updates::check_now(app2));
    }
}

fn core_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[cfg(test)]
mod tests {
    use super::core_version;

    #[test]
    fn compares_versions_without_test_build_suffixes() {
        assert_eq!(core_version("0.1.3-dev.20"), Some((0, 1, 3)));
        assert_eq!(core_version("v0.2.0"), Some((0, 2, 0)));
        assert!(core_version("0.1.10") > core_version("0.1.9"));
        assert_eq!(core_version("latest"), None);
    }
}
