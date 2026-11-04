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
    /// Patch 116: each stack's spot for each character (stack id → character → spot), learnt
    /// when the character leaves the stack.
    #[serde(default)]
    pub stacks: BTreeMap<String, BTreeMap<String, Spot>>,
    /// Which stack's spot a character's window is in now (none: its own spot above).
    #[serde(default)]
    pub in_stack: BTreeMap<String, String>,
    /// Patch 117: lock windows: a client moved off its spot (windowed mode) goes straight back.
    #[serde(default)]
    pub lock: bool,
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

/// A client window's spot now (None while minimised).
pub fn window_spot(handle: isize) -> Option<Spot> {
    win::spot(handle).filter(|s| s.w > 100 && s.h > 100)
}

/// Move a client window (its title-bar drag, done for it).
pub fn place_window(handle: isize, spot: Spot) -> bool {
    win::place(handle, spot)
}

/// Patch 117: Lock windows on / off.
#[tauri::command]
pub fn client_windows_lock(app: AppHandle, on: bool) {
    with(&app, |layout| layout.lock = on);
    save(&app);
    if on {
        hold(&app);
    }
}

/// With Lock on: every open client that has drifted off its spot goes back to it.
pub fn hold(app: &AppHandle) {
    if !with(app, |layout| layout.lock) {
        return;
    }
    for (name, handle) in crate::clipboard::open_clients() {
        let Some(spot) = with(app, |layout| spot_now(layout, &name)) else { continue };
        let Some(now) = window_spot(handle) else { continue };
        if (now.x - spot.x).abs() > 2 || (now.y - spot.y).abs() > 2 || (now.w - spot.w).abs() > 2 || (now.h - spot.h).abs() > 2 {
            win::place(handle, spot);
        }
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
            // A client sitting in a stack: that's its spot in the stack.
            with(&app, |layout| match layout.in_stack.get(&name).cloned() {
                Some(stack) => {
                    layout.stacks.entry(stack).or_default().insert(name.clone(), spot);
                }
                None => {
                    layout.spots.insert(name.clone(), spot);
                }
            });
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
    with(&app, |layout| serde_json::json!({ "auto": layout.auto, "lock": layout.lock, "remembered": layout.spots.keys().collect::<Vec<_>>() }))
}

/// Move the named clients (all when None) to their spots; how many moved.
pub fn place_these(app: &AppHandle, only: Option<&[String]>) -> usize {
    let mut moved = 0;
    for (name, handle) in crate::clipboard::open_clients() {
        if only.is_some_and(|names| !names.contains(&name)) {
            continue;
        }
        if let Some(spot) = with(app, |layout| spot_now(layout, &name)) {
            if win::place(handle, spot) {
                moved += 1;
            }
        }
    }
    moved
}

/// Where a character's window belongs right now: its stack's spot while it's in one, else its own.
fn spot_now(layout: &ClientLayout, name: &str) -> Option<Spot> {
    layout
        .in_stack
        .get(name)
        .and_then(|stack| layout.stacks.get(stack))
        .and_then(|spots| spots.get(name))
        .or_else(|| layout.spots.get(name))
        .copied()
}

/// Patch 116: a character joined or left a stack (its square on the preview). With the stack's
/// "remember window locations": joining keeps where the window is now (as its own spot, or its
/// spot in the stack it was in) and moves it to its spot in this stack; leaving keeps its spot
/// here and moves it back (to another stack it's still in, else its own spot). The first time,
/// there's no spot in the stack yet: drag the window there, and leaving remembers it.
pub fn stack_changed(app: &AppHandle, name: &str, stack: &str, joined: bool, remember: bool, still_on: &[String]) {
    if !keeps_a_spot(name) {
        return;
    }
    let handle = crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h);
    let now = handle.and_then(win::spot).filter(|s| s.w > 100 && s.h > 100);
    let target = with(app, |layout| stack_move(layout, name, stack, joined, remember, still_on, now));
    save(app);
    if let (Some(handle), Some(target)) = (handle, target) {
        if win::place(handle, target) {
            log::info!("{name} {} stack {stack}: window moved", if joined { "joined" } else { "left" });
        }
    }
}

/// The bookkeeping of `stack_changed`: updates the spots, returns where the window should go.
fn stack_move(layout: &mut ClientLayout, name: &str, stack: &str, joined: bool, remember: bool, still_on: &[String], now: Option<Spot>) -> Option<Spot> {
    let current = layout.in_stack.get(name).cloned();
    if joined {
        if !remember {
            return None;
        }
        if let Some(now) = now {
            match &current {
                Some(other) => {
                    layout.stacks.entry(other.clone()).or_default().insert(name.to_string(), now);
                }
                None => {
                    layout.spots.insert(name.to_string(), now);
                }
            }
        }
        layout.in_stack.insert(name.to_string(), stack.to_string());
        layout.stacks.get(stack).and_then(|s| s.get(name)).copied()
    } else {
        if current.as_deref() != Some(stack) {
            return None;
        }
        if let Some(now) = now.filter(|_| remember) {
            layout.stacks.entry(stack.to_string()).or_default().insert(name.to_string(), now);
        }
        match still_on.first() {
            Some(next) => {
                layout.in_stack.insert(name.to_string(), next.clone());
            }
            None => {
                layout.in_stack.remove(name);
            }
        }
        spot_now(layout, name)
    }
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
    use super::*;

    const TOP_LEFT: Spot = Spot { x: 0, y: 0, w: 1280, h: 720 };
    const BOTTOM_LEFT: Spot = Spot { x: 0, y: 720, w: 1280, h: 720 };
    const BOTTOM_RIGHT: Spot = Spot { x: 1280, y: 720, w: 1280, h: 720 };

    #[test]
    fn joining_and_leaving_a_stack_moves_the_window_there_and_back() {
        let mut layout = ClientLayout::default();
        // First time: no spot in the stack yet, so it stays; its own spot is kept.
        assert_eq!(stack_move(&mut layout, "Tsawind", "dps", true, true, &[], Some(TOP_LEFT)), None);
        assert_eq!(layout.spots["Tsawind"], TOP_LEFT);
        // Dragged down into the stack by hand, then leaves: that spot is learnt, back to the top left.
        assert_eq!(stack_move(&mut layout, "Tsawind", "dps", false, true, &[], Some(BOTTOM_LEFT)), Some(TOP_LEFT));
        assert!(!layout.in_stack.contains_key("Tsawind"));
        // Next time it joins, it goes straight down.
        assert_eq!(stack_move(&mut layout, "Tsawind", "dps", true, true, &[], Some(TOP_LEFT)), Some(BOTTOM_LEFT));
        assert_eq!(layout.in_stack["Tsawind"], "dps");
    }

    #[test]
    fn leaving_one_stack_goes_to_another_it_is_still_in() {
        let mut layout = ClientLayout::default();
        layout.spots.insert("Tsawind".into(), TOP_LEFT);
        layout.stacks.entry("dps".into()).or_default().insert("Tsawind".into(), BOTTOM_LEFT);
        layout.stacks.entry("logi".into()).or_default().insert("Tsawind".into(), BOTTOM_RIGHT);
        assert_eq!(stack_move(&mut layout, "Tsawind", "dps", true, true, &[], Some(TOP_LEFT)), Some(BOTTOM_LEFT));
        assert_eq!(stack_move(&mut layout, "Tsawind", "logi", true, true, &["dps".into()], Some(BOTTOM_LEFT)), Some(BOTTOM_RIGHT));
        // Leaving logi while still in dps: back down to the dps spot.
        assert_eq!(stack_move(&mut layout, "Tsawind", "logi", false, true, &["dps".into()], Some(BOTTOM_RIGHT)), Some(BOTTOM_LEFT));
        assert_eq!(layout.in_stack["Tsawind"], "dps");
    }

    #[test]
    fn stacks_without_remember_never_move_it() {
        let mut layout = ClientLayout::default();
        layout.spots.insert("MyThai".into(), TOP_LEFT);
        assert_eq!(stack_move(&mut layout, "MyThai", "dps", true, false, &[], Some(BOTTOM_LEFT)), None);
        assert_eq!(stack_move(&mut layout, "MyThai", "dps", false, false, &[], Some(BOTTOM_LEFT)), None);
        assert_eq!(layout.spots["MyThai"], TOP_LEFT);
    }

    #[test]
    fn login_screens_have_no_spot() {
        assert!(keeps_a_spot("Darkness Holder"));
        assert!(!keeps_a_spot("Login 3"));
    }
}
