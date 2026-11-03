//! Where each EVE client's window sits on screen, kept per character (like EVE-O Preview's
//! "client layout"): **Remember** stores every open client's spot and size; **Put back**
//! moves them there; with **Put back by themselves** on, a client that starts (or logs in)
//! is moved to its spot. This moves the game's window the way dragging its title bar would;
//! nothing inside the game is clicked or typed.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Spot {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientLayout {
    #[serde(default)]
    pub spots: BTreeMap<String, Spot>,
    #[serde(default)]
    pub auto: bool,
}

static LAYOUT: Mutex<Option<ClientLayout>> = Mutex::new(None);

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("client-windows.json"))
}

fn with<T>(app: &AppHandle, f: impl FnOnce(&mut ClientLayout) -> T) -> T {
    let mut guard = LAYOUT.lock().unwrap();
    if guard.is_none() {
        *guard = Some(path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    }
    f(guard.as_mut().expect("loaded"))
}

fn save(app: &AppHandle) {
    let text = with(app, |layout| serde_json::to_string_pretty(layout).unwrap_or_default());
    if let Some(p) = path(app) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, text);
    }
}

/// Characters only: login screens ("Login 3") change number, so they have no spot of their own.
pub fn keeps_a_spot(name: &str) -> bool {
    !name.starts_with("Login ") && name != crate::cycle::LOGIN_SCREENS
}

#[cfg(windows)]
mod win {
    use super::Spot;
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

    pub fn spot(handle: isize) -> Option<Spot> {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: plain Win32 calls on a window handle we were given by EnumWindows.
        unsafe {
            if IsIconic(handle as HWND) != 0 || GetWindowRect(handle as HWND, &mut rect) == 0 {
                return None;
            }
        }
        Some(Spot { x: rect.left, y: rect.top, w: rect.right - rect.left, h: rect.bottom - rect.top })
    }

    pub fn place(handle: isize, spot: Spot) -> bool {
        // SAFETY: as above; no activation and no z-order change, so focus stays where it is.
        unsafe { SetWindowPos(handle as HWND, std::ptr::null_mut(), spot.x, spot.y, spot.w, spot.h, SWP_NOZORDER | SWP_NOACTIVATE) != 0 }
    }
}

#[cfg(not(windows))]
mod win {
    use super::Spot;
    pub fn spot(_handle: isize) -> Option<Spot> {
        None
    }
    pub fn place(_handle: isize, _spot: Spot) -> bool {
        false
    }
}

/// Remember where every open client is now.
#[tauri::command]
pub fn client_windows_remember(app: AppHandle) -> Result<String, String> {
    let mut count = 0;
    for (name, handle) in crate::clipboard::open_clients() {
        if !keeps_a_spot(&name) {
            continue;
        }
        if let Some(spot) = win::spot(handle).filter(|s| s.w > 100 && s.h > 100) {
            with(&app, |layout| layout.spots.insert(name.clone(), spot));
            count += 1;
        }
    }
    save(&app);
    if count == 0 {
        return Err("No logged-in clients to remember (minimised ones are skipped).".into());
    }
    Ok(format!("Remembered where {count} client{} sit.", if count == 1 { "" } else { "s" }))
}

/// Move every open client to its remembered spot.
#[tauri::command]
pub fn client_windows_place(app: AppHandle) -> Result<String, String> {
    let moved = place_these(&app, None);
    if moved == 0 {
        return Err("No open client has a remembered spot yet: press Remember first.".into());
    }
    Ok(format!("Put {moved} client{} back.", if moved == 1 { "" } else { "s" }))
}

/// On / off: move clients to their spot as they start or log in.
#[tauri::command]
pub fn client_windows_auto(app: AppHandle, on: bool) {
    with(&app, |layout| layout.auto = on);
    save(&app);
}

#[tauri::command]
pub fn client_windows_state(app: AppHandle) -> serde_json::Value {
    with(&app, |layout| serde_json::json!({ "auto": layout.auto, "remembered": layout.spots.keys().collect::<Vec<_>>() }))
}

/// Move the named clients (all when None) to their spots; how many moved.
pub fn place_these(app: &AppHandle, only: Option<&[String]>) -> usize {
    let spots = with(app, |layout| layout.spots.clone());
    let mut moved = 0;
    for (name, handle) in crate::clipboard::open_clients() {
        if only.is_some_and(|names| !names.contains(&name)) {
            continue;
        }
        if let Some(spot) = spots.get(&name) {
            if win::place(handle, *spot) {
                moved += 1;
            }
        }
    }
    moved
}

/// For the preview watcher: clients that just appeared go to their spot when Auto is on.
pub fn on_new_clients(app: &AppHandle, names: &[String]) {
    if names.is_empty() || !with(app, |layout| layout.auto) {
        return;
    }
    let names: Vec<String> = names.iter().filter(|n| keeps_a_spot(n)).cloned().collect();
    if !names.is_empty() {
        let moved = place_these(app, Some(&names));
        if moved > 0 {
            log::info!("put {moved} new client window(s) in their spots");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::keeps_a_spot;

    #[test]
    fn login_screens_have_no_spot() {
        assert!(keeps_a_spot("Darkness Holder"));
        assert!(!keeps_a_spot("Login 3"));
    }
}
