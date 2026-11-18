//! Patch 74: the client bar. A slim bar of ours laid over the top of an EVE client (Fixed Window
//! mode first: no title bar, so Windows can't resize it by accident): 🔒 lock / unlock, ✥ drag to
//! move the whole client at its exact size (let go near a monitor corner, edge or middle line and
//! it snaps there, never resized), 📹 snapshot / video of this client, ⚙ this client's spot,
//! profile and launch groups, — minimise, ✕ close.
//!
//! Like Arrange, this only moves, minimises or closes the game's window the way its title bar
//! would; nothing inside the game is clicked or typed. The drag follows the mouse by reading where
//! the pointer is while its button is held.
//!
//! The bar shows only while its client is the window you'd see at that spot (a stack shows the
//! bar of the client in front; another app on top hides it).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::clientwindows::Spot;

const PREFIX: &str = "clientbar-";
/// The bar's size in logical pixels (menus make it taller while open).
const BAR_W: f64 = 300.0;
const BAR_H: f64 = 34.0;
/// How close (physical pixels) a dropped client must be to a line to snap to it.
const SNAP: i32 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarSettings {
    /// The characters that get a bar.
    #[serde(default)]
    pub clients: BTreeSet<String>,
    /// "left" (past EVE's Neocom), "center" or "right".
    #[serde(default = "default_corner")]
    pub corner: String,
}

fn default_corner() -> String {
    "left".into()
}

impl Default for BarSettings {
    fn default() -> Self {
        Self { clients: BTreeSet::new(), corner: default_corner() }
    }
}

static SETTINGS: Mutex<Option<BarSettings>> = Mutex::new(None);
/// Clients whose bar is unlocked (drag allowed; Lock windows leaves them alone). Locked by default.
static UNLOCKED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Bars with a menu open: label -> height (logical).
static TALL: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);
static DRAGGING: Mutex<Option<String>> = Mutex::new(None);

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("client-bars.json"))
}

fn settings(app: &AppHandle) -> BarSettings {
    let mut guard = SETTINGS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    }
    guard.clone().unwrap_or_default()
}

fn change(app: &AppHandle, f: impl FnOnce(&mut BarSettings)) {
    let mut now = settings(app);
    f(&mut now);
    if let Some(p) = path(app) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(&now).unwrap_or_default());
    }
    *SETTINGS.lock().unwrap() = Some(now);
}

/// Whether Lock windows should leave this client alone (its bar is unlocked or it's being dragged).
pub fn unlocked(name: &str) -> bool {
    UNLOCKED.lock().unwrap().as_ref().is_some_and(|set| set.contains(name)) || DRAGGING.lock().unwrap().as_deref() == Some(name)
}

pub fn label_for(name: &str) -> String {
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{PREFIX}{slug}")
}

fn handle_of(name: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h)
}

/// Where the bar goes over a client (physical pixels), given the visible frame of the client.
pub fn bar_spot(frame: Spot, corner: &str, scale: f64) -> (i32, i32) {
    let w = (BAR_W * scale).round() as i32;
    let x = match corner {
        "center" => frame.x + (frame.w - w) / 2,
        "right" => frame.x + frame.w - w - (8.0 * scale) as i32,
        _ => frame.x + (64.0 * scale) as i32,
    };
    (x, frame.y)
}

/// Where a dropped client snaps: its visible frame's edges to the work area's edges and middle
/// lines, each axis on its own, only when within [`SNAP`]. Returns the frame's new top-left.
pub fn snap(frame: Spot, work: Spot) -> (i32, i32) {
    let pick = |now: i32, size: i32, start: i32, len: i32| {
        let mid = start + len / 2;
        let candidates = [start, mid, start + len - size, mid - size];
        let best = candidates.iter().map(|c| (c - now).abs()).min().unwrap_or(i32::MAX);
        if best > SNAP {
            return now;
        }
        // Lines a few pixels apart (a 1918-wide client on a 3840 screen): the screen edge wins.
        candidates.into_iter().find(|c| (c - now).abs() <= best + 8).unwrap_or(now)
    };
    (pick(frame.x, frame.w, work.x, work.w), pick(frame.y, frame.h, work.y, work.h))
}

#[cfg(windows)]
mod win {
    use super::Spot;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetCursorPos, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW, ShowWindow, GWL_EXSTYLE,
        SW_MINIMIZE, WM_CLOSE, WS_EX_TRANSPARENT,
    };

    fn rect_of(handle: isize) -> Option<Spot> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: plain Win32 call on a window handle; a stale handle just fails.
        (unsafe { GetWindowRect(handle as HWND, &mut r) } != 0).then(|| Spot { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
    }

    /// The part of the window you see (without Windows' invisible resize borders).
    pub fn frame(handle: isize) -> Option<Spot> {
        use windows::Win32::Foundation::{HWND as WHWND, RECT as WRECT};
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
        // SAFETY: as above.
        if unsafe { IsIconic(handle as HWND) } != 0 {
            return None;
        }
        let mut r = WRECT::default();
        // SAFETY: DWM fills a RECT for a window handle; on failure we fall back to the window rect.
        let ok = unsafe { DwmGetWindowAttribute(WHWND(handle as *mut core::ffi::c_void), DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut WRECT as *mut core::ffi::c_void, std::mem::size_of::<WRECT>() as u32) }.is_ok();
        if ok && r.right > r.left {
            Some(Spot { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
        } else {
            rect_of(handle)
        }
    }

    pub fn window_rect(handle: isize) -> Option<Spot> {
        rect_of(handle)
    }

    fn cloaked(handle: isize) -> bool {
        use windows::Win32::Foundation::HWND as WHWND;
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
        let mut value: u32 = 0;
        // SAFETY: DWM writes a u32 for DWMWA_CLOAKED.
        let ok = unsafe { DwmGetWindowAttribute(WHWND(handle as *mut core::ffi::c_void), DWMWA_CLOAKED, &mut value as *mut u32 as *mut core::ffi::c_void, 4) }.is_ok();
        ok && value != 0
    }

    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> i32 {
        // SAFETY: `list` is the &mut Vec<isize> front_at passed to EnumWindows, alive for the call.
        let list = unsafe { &mut *(list as *mut Vec<isize>) };
        list.push(hwnd as isize);
        1
    }

    /// The window you'd see at a point: the topmost visible window there that isn't ours and isn't
    /// see-through for clicks.
    pub fn front_at(x: i32, y: i32) -> Option<isize> {
        let mut all: Vec<isize> = Vec::new();
        // SAFETY: EnumWindows calls `collect` with our Vec for every top-level window, top first.
        unsafe { EnumWindows(Some(collect), &mut all as *mut Vec<isize> as LPARAM) };
        let me = std::process::id();
        for handle in all {
            let hwnd = handle as HWND;
            // SAFETY: plain queries on handles EnumWindows just gave us.
            unsafe {
                if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                    continue;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == me || GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0 {
                    continue;
                }
            }
            if cloaked(handle) {
                continue;
            }
            if let Some(r) = rect_of(handle) {
                if x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h {
                    return Some(handle);
                }
            }
        }
        None
    }

    /// The work area (screen minus taskbar) of the monitor a rectangle is mostly on.
    pub fn work_area(of: Spot) -> Option<Spot> {
        let r = RECT { left: of.x, top: of.y, right: of.x + of.w, bottom: of.y + of.h };
        // SAFETY: MonitorFromRect never fails with DEFAULTTONEAREST; GetMonitorInfoW fills `info`.
        unsafe {
            let monitor = MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let w = info.rcWork;
            Some(Spot { x: w.left, y: w.top, w: w.right - w.left, h: w.bottom - w.top })
        }
    }

    pub fn cursor() -> Option<(i32, i32)> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: GetCursorPos fills a POINT.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
    }

    pub fn button_down() -> bool {
        // SAFETY: reads the left mouse button's state; nothing is sent anywhere.
        unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 }
    }

    pub fn minimize(handle: isize) -> bool {
        // SAFETY: the window's own minimise, as its title bar button would.
        unsafe { ShowWindow(handle as HWND, SW_MINIMIZE) != 0 }
    }

    pub fn close(handle: isize) -> bool {
        // SAFETY: asks the window to close, as its ✕ (or Alt+F4) would; EVE may ask to confirm.
        unsafe { PostMessageW(handle as HWND, WM_CLOSE, 0, 0) != 0 }
    }
}

#[cfg(not(windows))]
mod win {
    use super::Spot;
    pub fn frame(_h: isize) -> Option<Spot> {
        None
    }
    pub fn window_rect(_h: isize) -> Option<Spot> {
        None
    }
    pub fn front_at(_x: i32, _y: i32) -> Option<isize> {
        None
    }
    pub fn work_area(_of: Spot) -> Option<Spot> {
        None
    }
    pub fn cursor() -> Option<(i32, i32)> {
        None
    }
    pub fn button_down() -> bool {
        false
    }
    pub fn minimize(_h: isize) -> bool {
        false
    }
    pub fn close(_h: isize) -> bool {
        false
    }
}

fn build_bar(app: &AppHandle, name: &str) -> Option<tauri::WebviewWindow> {
    let init = format!("window.__CLIENT_NAME__ = {};", serde_json::Value::String(name.to_string()));
    WebviewWindowBuilder::new(app, label_for(name), WebviewUrl::App("clientbar.html".into()))
        .initialization_script(init)
        .title(format!("{name} · bar"))
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .inner_size(BAR_W, BAR_H)
        .visible(false)
        .build()
        .map_err(|err| log::warn!("couldn't open the client bar for {name}: {err}"))
        .ok()
}

/// Keeps every bar over its client: made when its client opens, moved with it, shown only when
/// that client is what you see there, closed with the client.
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("client-bars".into())
        .spawn(move || {
            let mut shown: HashMap<String, (bool, i32, i32, f64)> = HashMap::new();
            loop {
                std::thread::sleep(Duration::from_millis(150));
                let wanted = settings(&app);
                let open: HashMap<String, isize> = crate::clipboard::open_clients().into_iter().filter(|(n, _)| wanted.clients.contains(n)).collect();
                // Bars whose client closed or was taken off the list.
                for (label, window) in app.webview_windows() {
                    if !label.starts_with(PREFIX) {
                        continue;
                    }
                    let keep = open.keys().any(|n| label_for(n) == label);
                    if !keep {
                        shown.remove(&label);
                        let _ = window.destroy();
                    }
                }
                for (name, handle) in &open {
                    let label = label_for(name);
                    let window = match app.get_webview_window(&label) {
                        Some(w) => w,
                        None => match build_bar(&app, name) {
                            Some(w) => w,
                            None => continue,
                        },
                    };
                    let scale = window.scale_factor().unwrap_or(1.0);
                    let tall = TALL.lock().unwrap().as_ref().and_then(|m| m.get(&label).copied());
                    let Some(frame) = win::frame(*handle) else {
                        if shown.get(&label).is_none_or(|s| s.0) && tall.is_none() {
                            let _ = window.hide();
                            shown.insert(label.clone(), (false, 0, 0, 0.0));
                        }
                        continue;
                    };
                    let (x, y) = bar_spot(frame, &wanted.corner, scale);
                    let (cx, cy) = (x + (BAR_W * scale / 2.0) as i32, y + (BAR_H * scale / 2.0) as i32);
                    let dragging = DRAGGING.lock().unwrap().as_deref() == Some(name.as_str());
                    let see = dragging || tall.is_some() || win::front_at(cx, cy) == Some(*handle);
                    let height = tall.unwrap_or(BAR_H);
                    let before = shown.get(&label).copied();
                    if before.is_none_or(|b| b.1 != x || b.2 != y) {
                        let _ = window.set_position(PhysicalPosition::new(x, y));
                    }
                    if before.is_none_or(|b| (b.3 - height).abs() > 0.5) {
                        let _ = window.set_size(LogicalSize::new(BAR_W, height));
                    }
                    if before.is_none_or(|b| b.0 != see) {
                        let _ = if see { window.show() } else { window.hide() };
                    }
                    shown.insert(label, (see, x, y, height));
                }
            }
        })
        .ok();
}

/// For the Home page and the bar's ⚙: who has a bar, where it sits, and the open clients.
#[tauri::command]
pub fn clientbar_state(app: AppHandle) -> serde_json::Value {
    let s = settings(&app);
    let mut open: Vec<String> = crate::clipboard::open_clients().into_iter().map(|(n, _)| n).filter(|n| crate::clientwindows::keeps_a_spot(n)).collect();
    open.sort();
    serde_json::json!({ "clients": s.clients, "corner": s.corner, "open": open })
}

/// Give a client a bar, or take it away.
#[tauri::command]
pub fn clientbar_set(app: AppHandle, name: String, on: bool) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 60 {
        return Err("not a character name".into());
    }
    change(&app, |s| {
        if on {
            s.clients.insert(name);
        } else {
            s.clients.remove(&name);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn clientbar_corner(app: AppHandle, corner: String) -> Result<(), String> {
    if !["left", "center", "right"].contains(&corner.as_str()) {
        return Err("left, center or right".into());
    }
    change(&app, |s| s.corner = corner);
    Ok(())
}

/// This client's details for the bar's ⚙.
#[tauri::command]
pub fn clientbar_info(name: String) -> serde_json::Value {
    let handle = handle_of(&name);
    let frame = handle.and_then(win::frame);
    serde_json::json!({
        "name": name,
        "open": handle.is_some(),
        "locked": !UNLOCKED.lock().unwrap().as_ref().is_some_and(|s| s.contains(&name)),
        "frame": frame.map(|f| serde_json::json!({ "x": f.x, "y": f.y, "w": f.w, "h": f.h })),
    })
}

#[tauri::command]
pub fn clientbar_lock(name: String, locked: bool) {
    let mut guard = UNLOCKED.lock().unwrap();
    let set = guard.get_or_insert_with(HashSet::new);
    if locked {
        set.remove(&name);
    } else {
        set.insert(name);
    }
}

/// A menu opened or closed: the bar grows to fit it.
#[tauri::command]
pub fn clientbar_menu(window: tauri::WebviewWindow, height: Option<f64>) {
    let label = window.label().to_string();
    let mut guard = TALL.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    match height {
        Some(h) => {
            map.insert(label, h.clamp(BAR_H, 600.0));
        }
        None => {
            map.remove(&label);
        }
    }
}

/// ✥ pressed: the client follows the mouse while its button is held, then snaps and is
/// remembered at its new spot. Never resized.
#[tauri::command]
pub fn clientbar_drag(app: AppHandle, name: String) -> Result<(), String> {
    if !unlocked(&name) {
        return Err("Unlock it first (🔒).".into());
    }
    let handle = handle_of(&name).ok_or("that client is closed")?;
    let start_rect = win::window_rect(handle).ok_or("that client is minimised")?;
    let start_mouse = win::cursor().ok_or("no mouse position")?;
    *DRAGGING.lock().unwrap() = Some(name.clone());
    std::thread::Builder::new()
        .name("client-drag".into())
        .spawn(move || {
            let mut last = start_mouse;
            // Up to 10 minutes of holding; stops the moment the button is let go.
            for _ in 0..(10 * 60 * 125) {
                if !win::button_down() {
                    break;
                }
                if let Some(now) = win::cursor() {
                    if now != last {
                        last = now;
                        let to = Spot { x: start_rect.x + now.0 - start_mouse.0, y: start_rect.y + now.1 - start_mouse.1, w: start_rect.w, h: start_rect.h };
                        crate::clientwindows::place_window(handle, to);
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            // Snap the part you see to the monitor's edges / middle lines.
            if let (Some(rect), Some(frame)) = (win::window_rect(handle), win::frame(handle)) {
                if let Some(work) = win::work_area(frame) {
                    let (fx, fy) = snap(frame, work);
                    let to = Spot { x: rect.x + (fx - frame.x), y: rect.y + (fy - frame.y), w: rect.w, h: rect.h };
                    crate::clientwindows::place_window(handle, to);
                    crate::clientwindows::remember_one(&app, &name, to);
                }
            }
            *DRAGGING.lock().unwrap() = None;
            crate::capture::focus_handle(handle);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn clientbar_minimize(name: String) -> Result<(), String> {
    let handle = handle_of(&name).ok_or("that client is closed")?;
    win::minimize(handle);
    Ok(())
}

#[tauri::command]
pub fn clientbar_close(name: String) -> Result<(), String> {
    let handle = handle_of(&name).ok_or("that client is already closed")?;
    win::close(handle);
    Ok(())
}

/// Back to the client after using the bar (the bar took the focus when clicked).
#[tauri::command]
pub fn clientbar_focus_client(name: String) {
    crate::capture::focus_client(&name);
}

/// Open one of the app's own pages in the main window (from the bar's ⚙).
#[tauri::command]
pub fn clientbar_open_page(app: AppHandle, page: String) -> Result<(), String> {
    const PAGES: &[&str] = &["setups.html", "profiles.html", "tuning.html", "launch.html", "newgroup.html", "index.html"];
    if !PAGES.contains(&page.as_str()) {
        return Err("not one of the app's pages".into());
    }
    let base = app.get_webview(crate::PAGES_VIEW).and_then(|v| v.url().ok()).ok_or("the app's pages aren't loaded")?;
    let url = base.join(&page).map_err(|e| e.to_string())?;
    crate::go(&app, url);
    if let Some(window) = app.get_window(crate::MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Spot = Spot { x: 0, y: 0, w: 3840, h: 2112 };

    #[test]
    fn snaps_to_corners_edges_and_middle_lines() {
        let client = |x, y| Spot { x, y, w: 1920, h: 1056 };
        assert_eq!(snap(client(30, -20), WORK), (0, 0));
        assert_eq!(snap(client(1890, 1080), WORK), (1920, 1056));
        // right edge: 3840 - 1920 = 1920 (same as the middle line here)
        assert_eq!(snap(client(1960, 0), WORK), (1920, 0));
        // nothing near: stays
        assert_eq!(snap(client(700, 500), WORK), (700, 500));
    }

    #[test]
    fn snaps_each_axis_on_its_own() {
        let small = Spot { x: 980, y: 300, w: 900, h: 600 };
        // x near the middle line minus its width (1920 - 900 = 1020); y near nothing.
        assert_eq!(snap(small, WORK), (1020, 300));
    }

    #[test]
    fn snaps_on_a_monitor_left_of_the_main_one() {
        let work = Spot { x: -3840, y: 0, w: 3840, h: 2112 };
        let c = Spot { x: -3800, y: 1040, w: 1918, h: 1024 };
        assert_eq!(snap(c, work), (-3840, 1056));
    }

    #[test]
    fn bar_sits_past_the_neocom_or_centered_or_right() {
        let f = Spot { x: 0, y: 0, w: 1920, h: 1056 };
        assert_eq!(bar_spot(f, "left", 1.0), (64, 0));
        assert_eq!(bar_spot(f, "center", 1.0), (810, 0));
        assert_eq!(bar_spot(f, "right", 1.0), (1612, 0));
        assert_eq!(bar_spot(f, "left", 1.5), (96, 0));
    }

    #[test]
    fn names_make_safe_labels() {
        assert_eq!(label_for("Darkness Holder"), "clientbar-DarknessHolder");
        assert_eq!(label_for("Tsawind"), "clientbar-Tsawind");
    }
}
