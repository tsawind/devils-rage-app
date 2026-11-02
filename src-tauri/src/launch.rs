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
    /// Patch 63b: a hotkey (e.g. the pedal's "F13") that cycles this group forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_hotkey: Option<String>,
    /// And one that cycles it backward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backward_hotkey: Option<String>,
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
            forward_hotkey: g.forward_hotkey.filter(|h| !h.trim().is_empty()),
            backward_hotkey: g.backward_hotkey.filter(|h| !h.trim().is_empty()),
        })
        .collect();
    let text = serde_json::to_string_pretty(&Groups { groups: cleaned }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    #[cfg(desktop)]
    register_cycle_hotkeys(&app);
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

/// Patch 63b: the foot-pedal / hotkey cycling. Registers every group's cycle hotkeys as global
/// shortcuts; when one fires, brings the next open client in that group to the front.
#[cfg(desktop)]
mod hotkeys {
    use super::{load, Group};
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::Mutex;
    use tauri::AppHandle;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    /// Each registered shortcut -> (group id, forward?).
    static BOUND: Mutex<Vec<(Shortcut, String, bool)>> = Mutex::new(Vec::new());

    pub fn register(app: &AppHandle) {
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let mut bound = BOUND.lock().unwrap();
        bound.clear();
        for group in load(app).groups {
            for (hotkey, forward) in [(group.forward_hotkey.as_deref(), true), (group.backward_hotkey.as_deref(), false)] {
                let Some(hotkey) = hotkey.filter(|h| !h.trim().is_empty()) else { continue };
                match Shortcut::from_str(hotkey) {
                    Ok(shortcut) => {
                        if gs.register(shortcut).is_ok() {
                            bound.push((shortcut, group.id.clone(), forward));
                        } else {
                            log::warn!("couldn't register cycle hotkey {hotkey} (in use by another app?)");
                        }
                    }
                    Err(_) => log::warn!("cycle hotkey {hotkey} isn't a key we understand"),
                }
            }
        }
        log::info!("cycle hotkeys registered: {}", bound.len());
    }

    /// A registered cycle shortcut fired: advance that group and bring the next client forward.
    pub fn fired(app: &AppHandle, shortcut: &Shortcut) {
        let hit = BOUND.lock().unwrap().iter().find(|(s, _, _)| s == shortcut).map(|(_, id, fwd)| (id.clone(), *fwd));
        let Some((group_id, forward)) = hit else { return };
        let group: Option<Group> = load(app).groups.into_iter().find(|g| g.id == group_id);
        let Some(group) = group else { return };
        let open: HashSet<String> = crate::clipboard::open_eve_windows().into_iter().map(|(name, _)| name).collect();
        if let Some(next) = crate::cycle::advance(&group_id, &group.members, &open, forward) {
            crate::capture::focus_client(&next);
            log::info!("cycle {}: brought {next} forward", group.name);
        }
    }
}

#[cfg(desktop)]
pub fn register_cycle_hotkeys(app: &AppHandle) {
    hotkeys::register(app);
}

#[cfg(desktop)]
pub fn cycle_shortcut_fired(app: &AppHandle, shortcut: &tauri_plugin_global_shortcut::Shortcut) {
    hotkeys::fired(app, shortcut);
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
