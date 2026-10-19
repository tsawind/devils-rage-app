//! Commands for the app's own pages (start screen, settings). Only the app's
//! own pages can call these: remote pages (mapper, SeAT) get no IPC access.

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::settings::{self, Channel};

#[derive(Serialize)]
pub struct AppInfo {
    version: String,
    channel: &'static str,
    start_page: Option<String>,
    motd: Option<String>,
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let s = settings::load(&app);
    AppInfo {
        version: app.package_info().version.to_string(),
        channel: s.channel(&app).as_str(),
        start_page: s.start_page,
        motd: crate::appconfig::current().and_then(|c| c.motd),
    }
}

#[tauri::command]
pub fn open_site(app: AppHandle, site: String, remember: bool) -> Result<(), String> {
    let url = crate::site_url(&app, &site).ok_or("unknown site")?;
    let mut s = settings::load(&app);
    let wanted = remember.then(|| site.clone());
    if s.start_page != wanted {
        s.start_page = wanted;
        settings::save(&app, &s)?;
    }
    crate::go(&app, url);
    Ok(())
}

#[tauri::command]
pub fn check_updates(app: AppHandle) {
    #[cfg(desktop)]
    crate::updates::check_now(app);
    #[cfg(not(desktop))]
    let _ = app;
}

// ---------------------------------------------------------------- settings page

#[derive(Serialize)]
pub struct SettingsView {
    version: String,
    channel: &'static str,
    start_page: String,
    autostart: bool,
    pop_out_on_top: bool,
    has_pop_out_position: bool,
    developer: bool,
    chat_log_location: bool,
    chat_log_switched_off: bool,
    chat_log_folder: Option<String>,
    chat_log_characters: usize,
    last_check_secs_ago: Option<u64>,
    last_check: Option<String>,
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    let s = settings::load(&app);
    #[cfg(desktop)]
    let autostart = {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().unwrap_or(false)
    };
    #[cfg(not(desktop))]
    let autostart = false;
    #[cfg(desktop)]
    let last = crate::updates::last_check();
    #[cfg(not(desktop))]
    let last: Option<(u64, String)> = None;

    SettingsView {
        version: app.package_info().version.to_string(),
        channel: s.channel(&app).as_str(),
        start_page: s.start_page.clone().unwrap_or_else(|| "home".into()),
        autostart,
        pop_out_on_top: s.pop_out_on_top(),
        has_pop_out_position: s.scanner_bounds.is_some() || !s.scanner_bounds_by.is_empty(),
        developer: s.developer,
        chat_log_location: s.chat_log_location(),
        chat_log_switched_off: !crate::appconfig::feature_on("chat_log_location"),
        chat_log_folder: crate::location::status().0,
        chat_log_characters: crate::location::status().1,
        last_check_secs_ago: last.as_ref().map(|(ago, _)| *ago),
        last_check: last.map(|(_, text)| text),
    }
}

/// One setting changed on the settings page; saved straight away.
#[tauri::command]
pub fn set_setting(app: AppHandle, key: String, value: Value) -> Result<(), String> {
    let mut s = settings::load(&app);
    match key.as_str() {
        "start_page" => {
            s.start_page = match value.as_str() {
                Some("mapper") => Some("mapper".into()),
                Some("seat") => Some("seat".into()),
                _ => None,
            }
        }
        "channel" => {
            s.channel = match value.as_str() {
                Some("stable") => Some(Channel::Stable),
                Some("beta") => Some(Channel::Beta),
                Some("dev") => Some(Channel::Dev),
                _ => return Err("unknown channel".into()),
            }
        }
        "pop_out_on_top" => {
            let on = value.as_bool().unwrap_or(true);
            s.pop_out_on_top = Some(on);
            crate::scanner::set_on_top(&app, on);
        }
        "developer" => s.developer = value.as_bool().unwrap_or(false),
        "chat_log_location" => s.chat_log_location = Some(value.as_bool().unwrap_or(true)),
        "autostart" => {
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                let launcher = app.autolaunch();
                let result = if value.as_bool().unwrap_or(false) {
                    launcher.enable()
                } else {
                    launcher.disable()
                };
                result.map_err(|e| e.to_string())?;
            }
            return Ok(()); // kept by Windows, not in settings.json
        }
        _ => return Err(format!("unknown setting {key}")),
    }
    log::info!("setting changed: {key} = {value}");
    settings::save(&app, &s)
}

#[tauri::command]
pub fn reset_pop_out_position(app: AppHandle) -> Result<(), String> {
    let mut s = settings::load(&app);
    s.scanner_bounds = None;
    s.scanner_bounds_by.clear();
    settings::save(&app, &s)
}

// ---------------------------------------------------------------- developer

fn log_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|dir| dir.join(format!("{}.log", crate::LOG_NAME)))
}

/// The end of the app log (the last ~200 KB), for "Copy".
#[tauri::command]
pub fn read_log(app: AppHandle) -> String {
    let Some(path) = log_file(&app) else {
        return String::new();
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let keep = 200_000;
    if text.len() > keep {
        let cut = text.len() - keep;
        let start = text[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
        text[start..].to_string()
    } else {
        text
    }
}

#[tauri::command]
pub fn open_log(app: AppHandle) -> Result<(), String> {
    let path = log_file(&app).ok_or("no log folder")?;
    let target = if path.exists() {
        path
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .ok_or("no log folder")?
    };
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Settings → Admin: the mapper's App admin page (the mapper checks who may open it).
#[tauri::command]
pub fn open_admin(app: AppHandle) {
    if let Ok(url) = format!("{}/desktop-app/admin", crate::MAPPER_URL).parse() {
        crate::go(&app, url);
    }
}

#[tauri::command]
pub fn open_devtools(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        win.open_devtools();
    }
}

/// Logs out of everything, forgets all settings, restarts the app.
#[tauri::command]
pub fn reset_app(app: AppHandle) -> Result<(), String> {
    log::warn!("resetting the app");
    for (_, window) in app.webview_windows() {
        let _ = window.clear_all_browsing_data();
    }
    settings::delete(&app);
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let _ = app.autolaunch().disable();
    }
    app.restart();
}
