//! The slim dark bar at the top of the main window (ui/bar.html): the tabs, the ⋯ menu
//! and the window buttons. It replaces Windows' title bar and the old menu bar.

use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
pub struct BarState {
    /// Which tab is showing: home, mapper, seat, accounts, pi, settings, or other.
    page: &'static str,
    maximized: bool,
    version: String,
    channel: &'static str,
}

/// A bar button was pressed (home, mapper, seat, accounts, pi, settings, admin, reload,
/// updates, minimize, maximize, close, drag).
#[tauri::command]
pub fn bar_action(app: AppHandle, action: String) {
    crate::bar_action(&app, &action);
}

/// What the bar shows; it asks about once a second.
#[tauri::command]
pub fn bar_state(app: AppHandle) -> BarState {
    let url = crate::shown_view(&app).and_then(|view| view.url().ok());
    let page = match url.as_ref() {
        None => "other",
        Some(url) => {
            let host = url.host_str().unwrap_or_default();
            let path = url.path();
            if host == crate::host_of(crate::MAPPER_URL) {
                "mapper"
            } else if host == crate::host_of(crate::SEAT_URL) {
                if path.starts_with("/devilsrage/accounts") || path.starts_with("/devilsrage/your-accounts") {
                    "accounts"
                } else if path.starts_with("/devilsrage/pi") {
                    "pi"
                } else {
                    "seat"
                }
            } else if path.ends_with("settings.html") {
                "settings"
            } else if path.ends_with("index.html") || path == "/" {
                "home"
            } else {
                "other"
            }
        }
    };
    let maximized = app
        .get_window(crate::MAIN_WINDOW)
        .and_then(|w| w.is_maximized().ok())
        .unwrap_or(false);
    let s = crate::settings::load(&app);
    BarState {
        page,
        maximized,
        version: app.package_info().version.to_string(),
        channel: s.channel(&app).as_str(),
    }
}
