//! 22 Nov (Devil's Peepshow split): files both programs use.
//!
//! The Devils app keeps its own folders (identifier `com.tsawind.devilsrage`). Devil's Peepshow
//! has its own (`com.tsawind.peepshow`) for its previews, bars and window spots, and reads what
//! the app knows (SeAT's launch groups and accounts, this PC's group keys, incognito characters,
//! profile labels) straight from the app's folder, so a change on the app's pages reaches it
//! without any messages. Paths here are the same whichever program asks.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// The Devils app's identifier (its settings folders are named after it).
pub const APP_ID: &str = "com.tsawind.devilsrage";
/// Devil's Peepshow's identifier.
pub const PEEPSHOW_ID: &str = "com.tsawind.peepshow";

/// A file in the Devils app's settings folder (%APPDATA%\com.tsawind.devilsrage on Windows).
pub fn app_config(app: &AppHandle, name: &str) -> Option<PathBuf> {
    app.path().config_dir().ok().map(|dir| dir.join(APP_ID).join(name))
}

/// A file in the Devils app's settings folder, for code with no AppHandle at hand (the same
/// folder as `app_config`).
pub fn app_config_plain(name: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_ID).join(name))
}

/// The Devils app's local data folder (%LOCALAPPDATA%\com.tsawind.devilsrage).
pub fn app_local(name: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(APP_ID).join(name))
}

/// Devil's Peepshow's local data folder (%LOCALAPPDATA%\com.tsawind.peepshow).
pub fn peepshow_local(name: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(PEEPSHOW_ID).join(name))
}

/// Peepshow's settings folder (%APPDATA%\com.tsawind.peepshow).
pub fn peepshow_config(name: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(PEEPSHOW_ID).join(name))
}

/// Where each program notes its own exe, so the other can start it.
pub fn exe_note(peepshow: bool) -> Option<PathBuf> {
    if peepshow {
        peepshow_local("exe-path.txt")
    } else {
        app_local("exe-path.txt")
    }
}

/// Note this program's exe for the other one.
pub fn note_own_exe(peepshow: bool) {
    let (Some(file), Ok(me)) = (exe_note(peepshow), std::env::current_exe()) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, me.display().to_string());
}

/// The other program's exe: where it last noted itself, else the usual install spots.
pub fn find_exe(peepshow: bool) -> Option<PathBuf> {
    let noted = exe_note(peepshow).and_then(|f| std::fs::read_to_string(f).ok()).map(|t| PathBuf::from(t.trim()));
    let me_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let (exe, folder) = if peepshow { ("devils-peepshow.exe", "Devils Peepshow") } else { ("devils-rage-app.exe", "Devils Rage") };
    let mut tries: Vec<PathBuf> = noted.into_iter().collect();
    if let Some(dir) = &me_dir {
        tries.push(dir.join(exe));
        tries.push(dir.join("Peepshow").join(exe));
        if let Some(up) = dir.parent() {
            tries.push(up.join(exe));
        }
    }
    if let Some(local) = dirs::data_local_dir() {
        tries.push(local.join(folder).join(exe));
        tries.push(local.join("Programs").join(folder).join(exe));
    }
    tries.into_iter().find(|p| p.is_file())
}

/// Peepshow's first start: its previews, bars, window spots and frameless clients start as the
/// app had them (copied, never moved: an older app keeps working).
pub fn copy_app_settings_once(app: &AppHandle) {
    let Ok(own_config) = app.path().app_config_dir() else { return };
    let Ok(own_local) = app.path().app_local_data_dir() else { return };
    let marker = own_config.join("copied-from-app.txt");
    if marker.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&own_config);
    let _ = std::fs::create_dir_all(&own_local);
    let mut copied = Vec::new();
    for name in ["client-bars.json", "client-windows.json", "previews.json"] {
        let (Some(from), to) = (app_config(app, name), own_config.join(name)) else { continue };
        if from.is_file() && !to.exists() && std::fs::copy(&from, &to).is_ok() {
            copied.push(name);
        }
    }
    if let (Some(from), to) = (app_local("frameless-clients.json"), own_local.join("frameless-clients.json")) {
        if from.is_file() && !to.exists() && std::fs::copy(&from, &to).is_ok() {
            copied.push("frameless-clients.json");
        }
    }
    let _ = std::fs::write(&marker, format!("copied from the Devils app: {}\n", copied.join(", ")));
    log::info!("first start: copied the app's {} into Peepshow's own folders", if copied.is_empty() { "nothing (none found)".to_string() } else { copied.join(", ") });
}

/// The app's files Peepshow follows, with when each last changed (to notice edits).
pub fn followed_stamps(app: &AppHandle) -> Vec<Option<std::time::SystemTime>> {
    ["seat-groups.json", "seat-group-prefs.json", "launch-groups.json", "stack-order.json", "incognito.json", "pc-accounts.json", "pc-account-info.json"]
        .iter()
        .map(|name| app_config(app, name).and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()))
        .collect()
}
