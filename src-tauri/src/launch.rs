//! Patch 63: launch groups, defined right in the app. A group is a named set of characters;
//! "Open launcher" (option A) starts the official EVE launcher, which does all the sign-in
//! itself — the app never holds any EVE password or token. (Direct launch, option B, is a
//! separate, experimental path discussed with Tsa; it is not wired here.)
//!
//! The list lives in launch-groups.json next to settings.json; characters are the EVE clients
//! open now plus whatever a group already names, so you build groups from who you actually run.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Groups {
    #[serde(default)]
    groups: Vec<Group>,
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("launch-groups.json"))
}

fn load(app: &AppHandle) -> Groups {
    store_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Your launch groups.
#[tauri::command]
pub fn launch_groups(app: AppHandle) -> Vec<Group> {
    load(&app).groups
}

/// Save the whole list (the editor sends it back after any change).
#[tauri::command]
pub fn save_launch_groups(app: AppHandle, groups: Vec<Group>) -> Result<(), String> {
    let path = store_path(&app).ok_or("no config folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let cleaned: Vec<Group> = groups
        .into_iter()
        .take(50)
        .map(|g| Group {
            id: g.id,
            name: g.name.chars().take(60).collect(),
            members: g.members.into_iter().take(40).collect(),
        })
        .collect();
    let text = serde_json::to_string_pretty(&Groups { groups: cleaned }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Characters to build groups from: the EVE clients open now (so you pick who you actually run).
#[tauri::command]
pub fn launch_known_characters() -> Vec<String> {
    crate::clipboard::open_eve_windows().into_iter().map(|(name, _)| name).collect()
}

/// The official EVE launcher's exe: the saved path, else the usual install spots.
fn find_launcher(app: &AppHandle) -> Option<PathBuf> {
    if let Some(saved) = crate::settings::load(app).launcher_path {
        let p = PathBuf::from(saved);
        if p.is_file() {
            return Some(p);
        }
    }
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let candidates = [
        local.as_ref().map(|d| d.join(r"eve-online\eve-online.exe")),
        local.as_ref().map(|d| d.join(r"Programs\eve-online\eve-online.exe")),
        Some(PathBuf::from(r"C:\Program Files\EVE\eve-online.exe")),
        Some(PathBuf::from(r"C:\Program Files (x86)\EVE\eve-online.exe")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// Option A: open the official EVE launcher (it signs in and launches). The group is for your
/// reference — click its clients in the launcher. Returns the exe used.
#[tauri::command]
pub fn open_launcher(app: AppHandle) -> Result<String, String> {
    let exe = find_launcher(&app).ok_or_else(|| {
        "Couldn't find the EVE launcher. Set its path in Settings (point to eve-online.exe).".to_string()
    })?;
    Command::new(&exe)
        .spawn()
        .map_err(|e| format!("couldn't start the launcher: {e}"))?;
    log::info!("opened the EVE launcher: {}", exe.display());
    Ok(exe.display().to_string())
}

/// Whether we can find the launcher (so the UI can nudge you to set the path).
#[tauri::command]
pub fn launcher_ready(app: AppHandle) -> bool {
    find_launcher(&app).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_round_trip_through_json() {
        let groups = Groups {
            groups: vec![Group { id: "a".into(), name: "Roll fleet".into(), members: vec!["Tsawind".into(), "LuigiSquirrel".into()] }],
        };
        let text = serde_json::to_string(&groups).unwrap();
        let back: Groups = serde_json::from_str(&text).unwrap();
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].members, vec!["Tsawind", "LuigiSquirrel"]);
    }

    #[test]
    fn a_missing_members_field_is_an_empty_group() {
        let back: Groups = serde_json::from_str(r#"{"groups":[{"id":"x","name":"Empty"}]}"#).unwrap();
        assert_eq!(back.groups[0].members.len(), 0);
    }
}
