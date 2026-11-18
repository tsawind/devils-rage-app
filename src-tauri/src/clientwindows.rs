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
    /// Patch 74: whether each client's spot was remembered from a Fixed Window client (true) or
    /// a Window-mode one (false). A spot from the other mode is a different rectangle (the bare
    /// client vs. Windows' title bar and invisible borders), so it's only moved to, never sized to.
    #[serde(default)]
    pub fixed: BTreeMap<String, bool>,
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

/// Sizes this close are the same window (rounding and borders), so nothing is resized.
pub fn same_size(a: Spot, b: Spot) -> bool {
    (a.w - b.w).abs() <= 10 && (a.h - b.h).abs() <= 10
}

/// Characters, and accounts' own login screens (Patch 74: "Login 1" is always the first
/// account's). Other login screens change number, so they have no spot of their own.
pub fn keeps_a_spot(name: &str) -> bool {
    (!name.starts_with("Login ") || crate::clipboard::is_account_login(name)) && name != crate::cycle::LOGIN_SCREENS
}

#[cfg(windows)]
mod win {
    use super::Spot;
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};

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

    pub fn captioned(handle: isize) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_STYLE, WS_CAPTION};
        // SAFETY: reads the window's style bits.
        let style = unsafe { GetWindowLongPtrW(handle as HWND, GWL_STYLE) } as u32;
        style & WS_CAPTION == WS_CAPTION
    }

    pub fn place(handle: isize, to: Spot) -> bool {
        // Patch 121: only resize when the size really differs. A window's rectangle read back
        // comes out a few pixels off (invisible borders, display scaling), and setting it again
        // shrank the client a little each time (1918x1024 became 1914x1021).
        let keep_size = spot(handle).is_some_and(|now| super::same_size(now, to));
        // Patch 74: never resize a Fixed Window client (no title bar). A spot remembered while it
        // was in Window mode is another size: leave it be; dragging it by its bar remembers its new spot.
        if !keep_size && !captioned(handle) {
            return false;
        }
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | if keep_size { SWP_NOSIZE } else { 0 };
        // SAFETY: as above; no activation and no z-order change, so focus stays where it is.
        unsafe { SetWindowPos(handle as HWND, std::ptr::null_mut(), to.x, to.y, to.w, to.h, flags) != 0 }
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
    pub fn captioned(_handle: isize) -> bool {
        true
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

/// Whether a spot remembered for `name` can be used as it is for this window: it was remembered
/// in the window's display mode, or (an older spot, mode not noted) it's the same size.
fn same_mode(layout: &ClientLayout, name: &str, handle: isize, spot: Spot) -> bool {
    let fixed_now = !win::captioned(handle);
    match layout.fixed.get(name) {
        Some(fixed) => *fixed == fixed_now,
        None => window_spot(handle).is_some_and(|now| same_size(now, spot)),
    }
}

/// Put a client in its remembered spot. A spot from the other display mode: moved there and
/// snapped, keeping its own size (Tsa, 18 Nov: Tsawind's Fixed Window spot shrank his Window-mode
/// client to 1902x985).
fn place_named(app: &AppHandle, name: &str, handle: isize, spot: Spot) -> bool {
    if with(app, |layout| same_mode(layout, name, handle, spot)) {
        return win::place(handle, spot);
    }
    match crate::clientbar::near_spot(app, handle, spot) {
        Some(to) => {
            log::info!("{name}: spot is from the other display mode; moved there, size kept");
            win::place(handle, to)
        }
        None => false,
    }
}

/// Put a named client in its remembered spot (see `place_named`).
pub fn place_in_spot(app: &AppHandle, name: &str, handle: isize, spot: Spot) -> bool {
    place_named(app, name, handle, spot)
}

/// Note which display mode a client's spot was remembered in.
pub fn note_mode(app: &AppHandle, name: &str, fixed: bool) {
    with(app, |layout| {
        layout.fixed.insert(name.to_string(), fixed);
    });
    save(app);
}

fn mode_of_open(name: &str) -> Option<bool> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| !win::captioned(h))
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
        // Patch 74: a client whose bar is unlocked (or being dragged) is being moved on purpose.
        if crate::clientbar::unlocked(&name) {
            continue;
        }
        let Some(spot) = with(app, |layout| spot_now(layout, &name)) else { continue };
        let Some(now) = window_spot(handle) else { continue };
        // A spot from the other display mode isn't this window's rectangle: don't keep forcing it.
        if !with(app, |layout| same_mode(layout, &name, handle, spot)) {
            continue;
        }
        // Moved off its spot, or resized by more than rounding: put it back.
        if (now.x - spot.x).abs() > 4 || (now.y - spot.y).abs() > 4 || !same_size(now, spot) {
            win::place(handle, spot);
        }
    }
}

/// Patch 74: remember one client's new spot (dropped from its bar), in its stack's spot when it's in one.
pub fn remember_one(app: &AppHandle, name: &str, spot: Spot) {
    if !keeps_a_spot(name) {
        return;
    }
    let fixed = mode_of_open(name);
    with(app, |layout| {
        match layout.in_stack.get(name).cloned() {
            Some(stack) => {
                layout.stacks.entry(stack).or_default().insert(name.to_string(), spot);
            }
            None => {
                layout.spots.insert(name.to_string(), spot);
            }
        }
        if let Some(fixed) = fixed {
            layout.fixed.insert(name.to_string(), fixed);
        }
    });
    save(app);
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
            with(&app, |layout| {
                match layout.in_stack.get(&name).cloned() {
                    Some(stack) => {
                        layout.stacks.entry(stack).or_default().insert(name.clone(), spot);
                    }
                    None => {
                        layout.spots.insert(name.clone(), spot);
                    }
                }
                layout.fixed.insert(name.clone(), !win::captioned(handle));
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
            if place_named(app, &name, handle, spot) {
                moved += 1;
            }
        }
    }
    moved
}

/// Where a character's window belongs right now: its stack's spot while it's in one, else its own.
/// Patch 74: where a character's client is remembered (its stack's spot when it's in one).
pub fn remembered(app: &AppHandle, name: &str) -> Option<Spot> {
    with(app, |layout| spot_now(layout, name))
}

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
    let target = with(app, |layout| {
        let target = stack_move(layout, name, stack, joined, remember, still_on, now);
        if let (Some(handle), Some(_)) = (handle, now.filter(|_| remember)) {
            layout.fixed.insert(name.to_string(), !win::captioned(handle));
        }
        target
    });
    save(app);
    if let (Some(handle), Some(target)) = (handle, target) {
        if place_named(app, name, handle, target) {
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
    fn a_spot_from_the_other_display_mode_is_not_used_as_it_is() {
        let mut layout = ClientLayout::default();
        // What mode a made-up window handle counts as depends on the platform: ask.
        let fixed_now = !win::captioned(1);
        layout.fixed.insert("Tsawind".into(), !fixed_now);
        assert!(!same_mode(&layout, "Tsawind", 1, TOP_LEFT));
        layout.fixed.insert("Tsawind".into(), fixed_now);
        assert!(same_mode(&layout, "Tsawind", 1, TOP_LEFT));
        // An older spot with no mode noted, size unknown: not trusted for sizing.
        assert!(!same_mode(&layout, "Girl Shady", 1, TOP_LEFT));
    }

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
    fn a_few_pixels_is_the_same_size() {
        assert!(same_size(Spot { x: 0, y: 0, w: 1918, h: 1024 }, Spot { x: 5, y: 5, w: 1914, h: 1021 }));
        assert!(!same_size(TOP_LEFT, Spot { x: 0, y: 0, w: 1920, h: 1080 }));
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
