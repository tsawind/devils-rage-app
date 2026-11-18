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
use std::time::{Duration, Instant};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::clientwindows::Spot;

const PREFIX: &str = "clientbar-";
/// The bar's size in logical pixels (menus make it taller while open).
const BAR_W: f64 = 300.0;
/// A shrunk bar: just ‹, ⚙ and ✕.
const BAR_MINI_W: f64 = 100.0;
const BAR_H: f64 = 34.0;
/// How close (physical pixels) a dropped client must be to a line to snap to it.
const SNAP: i32 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarSettings {
    /// Every logged-in client gets a bar (like previews), except those in `off`.
    #[serde(default = "yes")]
    pub every: bool,
    #[serde(default)]
    pub off: BTreeSet<String>,
    /// When `every` is off: only these clients get a bar.
    #[serde(default)]
    pub clients: BTreeSet<String>,
    /// Fixed Window clients that keep the small floating bar instead of our title bar.
    #[serde(default)]
    pub floating: BTreeSet<String>,
    /// Small bars dragged to a spot of their own: name -> offset from the client's top-left (px).
    #[serde(default)]
    pub offsets: std::collections::BTreeMap<String, (i32, i32)>,
    /// Bars shrunk (›) to just ⚙ and ✕.
    #[serde(default)]
    pub mini: BTreeSet<String>,
    /// "right" (default), "center" or "left" (past EVE's Neocom).
    #[serde(default = "default_corner")]
    pub corner: String,
    /// Which buttons the bar shows besides ⚙ (and › on the small bar): lock, move, camera,
    /// minimize, close. Default move and close (Tsa, 17 Nov); the rest live in ⚙.
    #[serde(default = "default_buttons")]
    pub buttons: Vec<String>,
    #[serde(default)]
    pub version: u32,
}

fn yes() -> bool {
    true
}

fn default_corner() -> String {
    "right".into()
}

pub const BUTTONS: &[&str] = &["lock", "move", "camera", "popout", "minimize", "close"];

fn default_buttons() -> Vec<String> {
    vec!["move".into(), "close".into()]
}

/// The small bar's width for the buttons shown (logical px): ⋮⋮, ›, ⚙, the buttons, gaps, padding.
pub fn bar_width(buttons: &[String]) -> f64 {
    let each: f64 = buttons.iter().map(|b| if b == "move" { 46.0 } else { 30.0 }).sum();
    12.0 + 16.0 + 20.0 + 30.0 + each + 2.0 * (buttons.len() as f64 + 3.0)
}

impl Default for BarSettings {
    fn default() -> Self {
        Self { every: true, off: BTreeSet::new(), clients: BTreeSet::new(), floating: BTreeSet::new(), offsets: Default::default(), mini: BTreeSet::new(), corner: default_corner(), buttons: default_buttons(), version: 1 }
    }
}

impl BarSettings {
    /// Whether this client gets a bar (login screens never do).
    pub fn wants(&self, name: &str) -> bool {
        // Login screens ("Login 2") get one too when every client does (Tsa, 17 Nov); their name
        // changes at login, so they can't be picked one by one.
        if name.starts_with("Login ") {
            return self.every;
        }
        crate::clientwindows::keeps_a_spot(name) && if self.every { !self.off.contains(name) } else { self.clients.contains(name) }
    }

    /// Settings from dev.189 (one client, bar on the left) move to every client, bar top right.
    fn upgrade(mut self) -> Self {
        if self.version < 1 {
            self.every = true;
            self.corner = default_corner();
            self.version = 1;
        }
        self
    }
}

static SETTINGS: Mutex<Option<BarSettings>> = Mutex::new(None);
/// Clients locked in place from ⚙ (their bar can't move them). Unlocked by default.
static LOCKED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Bars with a menu open: label -> height (logical).
static TALL: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);
static DRAGGING: Mutex<Option<String>> = Mutex::new(None);
/// The bar (label) being dragged to a spot of its own.
static BAR_MOVING: Mutex<Option<String>> = Mutex::new(None);

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("client-bars.json"))
}

fn settings(app: &AppHandle) -> BarSettings {
    let mut guard = SETTINGS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<BarSettings>(&t).ok()).map(BarSettings::upgrade).unwrap_or_default());
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

/// Whether Lock windows should leave this client alone right now (it's being dragged by its bar).
pub fn unlocked(name: &str) -> bool {
    DRAGGING.lock().unwrap().as_deref() == Some(name)
}

fn locked(name: &str) -> bool {
    LOCKED.lock().unwrap().as_ref().is_some_and(|set| set.contains(name))
}

pub fn label_for(name: &str) -> String {
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{PREFIX}{slug}")
}

fn handle_of(name: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h)
}

/// Where the bar goes over a client (physical pixels), given the visible frame of the client and
/// the bar's width (logical).
pub fn bar_spot(frame: Spot, corner: &str, scale: f64, width: f64) -> (i32, i32) {
    let w = (width * scale).round() as i32;
    let x = match corner {
        "center" => frame.x + (frame.w - w) / 2,
        "left" => frame.x + (64.0 * scale) as i32,
        _ => frame.x + frame.w - w - (8.0 * scale) as i32,
    };
    (x, frame.y)
}

/// Windows' own title bar at 100%: 31 px, plus a 1 px edge round the window. A Fixed Window
/// client of 1918×1024 with our title bar and edge is 1920×1056: the same spot a window-mode
/// client snapped into a 4K quarter takes (Tsa, 17 Nov), so a client can switch between the two.
const TITLE: f64 = 31.0;
/// Windows' three title bar buttons (46 px each at 100%), and room for "EVE - name" on the left.
const CAPTION_BUTTONS: f64 = 140.0;
const CAPTION_TITLE: f64 = 180.0;

/// Our title bar (its top edge included, like Windows': 1024 + 31 + 1 = 1056), and the 1 px side /
/// bottom edge (physical pixels).
pub fn chrome(scale: f64) -> (i32, i32) {
    ((TITLE * scale).round() as i32, 1)
}

/// Where the bar window goes and how big it is (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// true: our title bar above the client and a 1 px edge round it (a Fixed Window client with
    /// room above it); false: the small bar laid over the client's top edge.
    pub frame: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub top: i32,
}

pub fn place_for(frame: Spot, captioned: bool, floating: bool, work: Option<Spot>, corner: &str, scale: f64, width: f64, height: f64) -> Place {
    let (top, side) = chrome(scale);
    if !captioned && !floating {
        if let Some(work) = work {
            if frame.y - top >= work.y {
                return Place { frame: true, x: frame.x - side, y: frame.y - top, w: frame.w + 2 * side, h: frame.h + top + side, top };
            }
        }
    }
    let (mut x, y) = bar_spot(frame, corner, scale, width);
    if captioned {
        // Window mode: stay clear of Windows' own –, ☐, ✕ (right) and the title (left).
        match corner {
            "left" => x += (CAPTION_TITLE * scale) as i32,
            "center" => {}
            _ => x -= (CAPTION_BUTTONS * scale) as i32,
        }
    }
    Place { frame: false, x, y, w: (width * scale).round() as i32, h: (height * scale).round() as i32, top: 0 }
}

/// The client with our title bar and edge round it (what snaps when it's dragged).
pub fn outer_of(frame: Spot, scale: f64) -> Spot {
    let (top, side) = chrome(scale);
    Spot { x: frame.x - side, y: frame.y - top, w: frame.w + 2 * side, h: frame.h + top + side }
}

/// Fixed Window windows already looked at for the nudge (by handle; a login screen keeps its
/// window when it logs in).
static SEEN_FIXED: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

fn first_sight(handle: isize) -> bool {
    let mut guard = SEEN_FIXED.lock().unwrap();
    let seen = guard.get_or_insert_with(HashSet::new);
    if seen.len() > 500 {
        seen.clear();
    }
    seen.insert(handle)
}

/// Where a Fixed Window client with no room above goes so our title bar fits: the client plus
/// title bar and edge, snapped on its monitor. Only when that still fits a quarter / half (a
/// 1918×1024 client) or the full height (a tall one); a 1920×1056 client is left alone.
pub fn nudge_for_title_bar(frame: Spot, work: Spot, scale: f64) -> Option<Spot> {
    let (top, side) = chrome(scale);
    let outer_h = frame.h + top + side;
    let fits = outer_h <= work.h / 2 + 1 || (frame.h > work.h / 2 && outer_h <= work.h);
    if !fits || frame.y - top >= work.y {
        return None;
    }
    let outer = Spot { x: frame.x - side, y: frame.y, w: frame.w + 2 * side, h: outer_h };
    let (sx, sy) = snap(outer, work);
    Some(Spot { x: sx + side, y: sy + top, w: frame.w, h: frame.h })
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
    #[allow(dead_code)]
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

    /// Move and resize a window as asked (no focus change).
    pub fn resize(handle: isize, to: Spot) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
        // SAFETY: SetWindowPos on a client window handle.
        unsafe { SetWindowPos(handle as HWND, std::ptr::null_mut(), to.x, to.y, to.w, to.h, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    /// Puts our bar window just above its client in the stacking order (no move, no focus).
    pub fn stack_above(ours: isize, client: isize) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindow, SetWindowPos, GW_HWNDPREV, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE};
        // SAFETY: z-order queries / changes on our own window and the client's handle.
        unsafe {
            let above = GetWindow(client as HWND, GW_HWNDPREV);
            if above as isize == ours {
                return;
            }
            let after = if above.is_null() { HWND_TOP } else { above };
            SetWindowPos(ours as HWND, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// The game's own drawing area (EVE's "Window Size"), without Windows' title bar and edges.
    pub fn client_size(handle: isize) -> Option<(i32, i32)> {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: fills a RECT for a window handle.
        (unsafe { GetClientRect(handle as HWND, &mut r) } != 0 && r.right > 0).then_some((r.right - r.left, r.bottom - r.top))
    }

    /// Whether the window has Windows' own title bar (EVE's Window mode; Fixed Window has none).
    pub fn captioned(handle: isize) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, WS_CAPTION};
        // SAFETY: reads the window's style bits.
        let style = unsafe { GetWindowLongPtrW(handle as HWND, GWL_STYLE) } as u32;
        style & WS_CAPTION == WS_CAPTION
    }

    /// Shapes our bar window: the title bar and edge only, with the client showing (and clickable)
    /// through the hole, plus an open menu's box. None = the whole window (the small bar).
    pub fn shape(ours: isize, w: i32, h: i32, ring: Option<(i32, i32)>, menu: Option<(i32, i32, i32, i32)>) {
        use windows_sys::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF, RGN_OR};
        // SAFETY: regions are made, combined and handed to SetWindowRgn (which then owns the final
        // one); the temporary ones are deleted. A failed call just leaves the window unshaped.
        unsafe {
            let Some((top, side)) = ring else {
                SetWindowRgn(ours as HWND, std::ptr::null_mut(), 1);
                return;
            };
            let all = CreateRectRgn(0, 0, w, h);
            let hole = CreateRectRgn(side, top, w - side, h - side);
            CombineRgn(all, all, hole, RGN_DIFF);
            DeleteObject(hole);
            if let Some((x, y, mw, mh)) = menu {
                let box_ = CreateRectRgn(x, y, x + mw, y + mh);
                CombineRgn(all, all, box_, RGN_OR);
                DeleteObject(box_);
            }
            SetWindowRgn(ours as HWND, all, 1);
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
    #[allow(dead_code)]
    pub fn front_at(_x: i32, _y: i32) -> Option<isize> {
        None
    }
    pub fn work_area(_of: Spot) -> Option<Spot> {
        None
    }
    pub fn cursor() -> Option<(i32, i32)> {
        None
    }
    pub fn captioned(_h: isize) -> bool {
        true
    }
    pub fn stack_above(_ours: isize, _client: isize) {}
    pub fn resize(_h: isize, _to: Spot) {}
    pub fn client_size(_h: isize) -> Option<(i32, i32)> {
        None
    }
    pub fn shape(_ours: isize, _w: i32, _h: i32, _ring: Option<(i32, i32)>, _menu: Option<(i32, i32, i32, i32)>) {}
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
        .always_on_top(false)
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

/// Each bar's frame mode, for the page (clientbar_info): name -> (frame, top in CSS px).
static MODES: Mutex<Option<HashMap<String, (bool, f64)>>> = Mutex::new(None);

fn our_hwnd(window: &tauri::WebviewWindow) -> Option<isize> {
    #[cfg(windows)]
    {
        window.hwnd().ok().map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Where a client lands when it switches display mode, so it keeps its spot (Tsa, 17 Nov):
/// Window mode → Fixed Window: the client goes inside where the old window was, leaving room for
/// our title bar and edge (1920×1056 window → 1918×1024 client at +1, +31); Fixed Window → Window
/// mode: the new window's visible frame goes where our title bar and edge were.
/// `old` = the visible frame before (window incl. Windows' title bar, or the fixed client);
/// `now_rect` / `now_frame` = the window's rectangle and visible frame after the switch.
pub fn after_switch(to_fixed: bool, old: Spot, now_rect: Spot, now_frame: Spot, scale: f64) -> Spot {
    let (top, side) = chrome(scale);
    let frame_to = if to_fixed { Spot { x: old.x + side, y: old.y + top, ..now_frame } } else { Spot { x: old.x - side, y: old.y - top, ..now_frame } };
    Spot { x: now_rect.x + (frame_to.x - now_frame.x), y: now_rect.y + (frame_to.y - now_frame.y), ..now_rect }
}

/// Watches every client's display mode; when one switches, puts it back in its spot (twice: EVE
/// can move the window again just after) and remembers it there.
/// Login screens already put into a converted client's spot (by window handle).
static PLACED_LOGINS: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

fn follow_switches(app: &AppHandle, seen: &mut HashMap<String, (bool, Spot)>) {
    // A client launched again after the Devil's window mode switch shows its login screen first:
    // a new Fixed Window login screen of exactly the converted size goes into that spot at once.
    let logins: Vec<isize> = crate::clipboard::open_clients().into_iter().filter(|(n, _)| n.starts_with("Login ")).map(|(_, h)| h).collect();
    {
        let mut placed = PLACED_LOGINS.lock().unwrap();
        let placed = placed.get_or_insert_with(HashSet::new);
        placed.retain(|h| logins.contains(h));
        for handle in logins {
            if placed.contains(&handle) || win::captioned(handle) {
                continue;
            }
            let Some(rect) = win::window_rect(handle) else { continue };
            if let Some(to) = crate::windowfix::pending_fixed_of_size(rect.w, rect.h) {
                placed.insert(handle);
                crate::clientwindows::place_window(handle, Spot { w: rect.w, h: rect.h, ..to });
            }
        }
    }
    let clients: Vec<(String, isize)> = crate::clipboard::open_clients().into_iter().filter(|(n, _)| crate::clientwindows::keeps_a_spot(n)).collect();
    seen.retain(|name, _| clients.iter().any(|(n, _)| n == name));
    for (name, handle) in clients {
        let Some(frame) = win::frame(handle) else { continue };
        let captioned = win::captioned(handle);
        // Launched again after the Devil's window mode switch: into its spot, once.
        if let Some((fixed, to)) = crate::windowfix::pending(&name) {
            if fixed != captioned {
                crate::windowfix::done(&name);
                std::thread::spawn(move || {
                    for wait in [400, 1500] {
                        std::thread::sleep(Duration::from_millis(wait));
                        let (Some(rect), Some(now)) = (win::window_rect(handle), win::frame(handle)) else { return };
                        let target = if fixed { to } else { Spot { x: rect.x + (to.x - now.x), y: rect.y + (to.y - now.y), ..rect } };
                        crate::clientwindows::place_window(handle, target);
                    }
                });
                seen.remove(&name);
                continue;
            }
        }
        let before = seen.insert(name.clone(), (captioned, frame));
        let Some((was_captioned, old)) = before else { continue };
        if was_captioned == captioned || DRAGGING.lock().unwrap().is_some() {
            continue;
        }
        let scale = app.get_webview_window(crate::MAIN_WINDOW).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
        // The spot it ends up at is the one to remember for the next switch.
        seen.remove(&name);
        let app = app.clone();
        log::info!("{name} switched to {}", if captioned { "Window mode" } else { "Fixed Window" });
        std::thread::spawn(move || {
            for wait in [300, 900] {
                std::thread::sleep(Duration::from_millis(wait));
                let (Some(rect), Some(now)) = (win::window_rect(handle), win::frame(handle)) else { return };
                let to = after_switch(!captioned, old, rect, now, scale);
                crate::clientwindows::place_window(handle, to);
                crate::clientwindows::remember_one(&app, &name, to);
            }
        });
    }
}

/// Keeps every bar over its client: made when its client opens, moved with it, shown only when
/// that client is what you see there, closed with the client.
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("client-bars".into())
        .spawn(move || {
            // label -> (shown, place, region key)
            let mut shown: HashMap<String, (bool, Place, (bool, i32, i32, i32))> = HashMap::new();
            let mut modes: HashMap<String, (bool, Spot)> = HashMap::new();
            let mut logins: HashMap<isize, LoginWatch> = HashMap::new();
            loop {
                std::thread::sleep(Duration::from_millis(150));
                follow_switches(&app, &mut modes);
                remember_moved_logins(&app, &mut logins);
                let wanted = settings(&app);
                // Window-mode clients get the small bar left of Windows' own –, ☐, ✕; Fixed Window
                // clients get our title bar (or the small bar when floating).
                let open: HashMap<String, isize> = crate::clipboard::open_clients().into_iter().filter(|(n, _)| wanted.wants(n)).collect();
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
                            if let Some(entry) = shown.get_mut(&label) {
                                entry.0 = false;
                            }
                        }
                        continue;
                    };
                    let width = match tall {
                        Some(_) => BAR_W,
                        None if wanted.mini.contains(name) => BAR_MINI_W,
                        None => bar_width(&wanted.buttons).max(BAR_MINI_W),
                    };
                    let mut place = place_for(frame, win::captioned(*handle), wanted.floating.contains(name), win::work_area(frame), &wanted.corner, scale, width, tall.unwrap_or(BAR_H));
                    // A Fixed Window client (login screens too) that opened flush against the top
                    // has no room for our title bar: the first time, nudge it into its snapped spot.
                    if !place.frame && !win::captioned(*handle) && !wanted.floating.contains(name) && first_sight(*handle) {
                        if let Some(work) = win::work_area(frame) {
                            if let Some(to) = nudge_for_title_bar(frame, work, scale) {
                                crate::clientwindows::place_window(*handle, to);
                                continue;
                            }
                        }
                    }
                    if !place.frame {
                        if let Some((dx, dy)) = wanted.offsets.get(name) {
                            place.x = frame.x + dx;
                            place.y = frame.y + dy;
                        }
                    }
                    if BAR_MOVING.lock().unwrap().as_deref() == Some(label.as_str()) {
                        continue;
                    }
                    // Shown whenever the client is; kept just above its client in Windows' stacking
                    // order (not on top of everything), so whatever covers the client covers it too.
                    let see = true;
                    if let Some(ours) = our_hwnd(&window) {
                        if tall.is_none() {
                            win::stack_above(ours, *handle);
                        }
                    }
                    let before = shown.get(&label).copied();
                    if before.is_none_or(|b| b.1.x != place.x || b.1.y != place.y) {
                        let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
                    }
                    if before.is_none_or(|b| b.1.w != place.w || b.1.h != place.h) {
                        let _ = window.set_size(tauri::PhysicalSize::new(place.w.max(1) as u32, place.h.max(1) as u32));
                    }
                    // Frame mode: only the title bar and edge are ours (plus an open menu's box).
                    let menu_h = tall.map_or(0, |t| ((t - BAR_H + 8.0) * scale).round() as i32);
                    let region = (place.frame, place.w, place.h, menu_h);
                    if before.is_none_or(|b| b.2 != region) {
                        if let Some(ours) = our_hwnd(&window) {
                            let menu_w = (BAR_W * scale).round() as i32;
                            let menu = (place.frame && menu_h > 0).then_some((place.w - menu_w, place.top, menu_w, menu_h));
                            win::shape(ours, place.w, place.h, place.frame.then_some((place.top, 1)), menu);
                        }
                    }
                    if before.is_none_or(|b| b.1.frame != place.frame || b.1.top != place.top) {
                        let top_css = place.top as f64 / scale;
                        MODES.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.clone(), (place.frame, top_css));
                        let _ = window.eval(format!("window.setMode && window.setMode({}, {top_css})", place.frame));
                    }
                    if before.is_none_or(|b| b.0 != see) {
                        let _ = if see { window.show() } else { window.hide() };
                    }
                    shown.insert(label, (see, place, region));
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
    serde_json::json!({ "every": s.every, "off": s.off, "clients": s.clients, "mini": s.mini, "corner": s.corner, "buttons": s.buttons, "open": open })
}

/// Give a client a bar, or take it away.
#[tauri::command]
pub fn clientbar_set(app: AppHandle, name: String, on: bool) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 60 {
        return Err("not a character name".into());
    }
    change(&app, |s| {
        if s.every {
            if on {
                s.off.remove(&name);
            } else {
                s.off.insert(name);
            }
        } else if on {
            s.clients.insert(name);
        } else {
            s.clients.remove(&name);
        }
    });
    Ok(())
}

/// A bar on every client (like previews), or only on the ones picked.
#[tauri::command]
pub fn clientbar_every(app: AppHandle, on: bool) {
    change(&app, |s| s.every = on);
}

/// Shrink a bar to just ⚙ and ✕, or show it whole again.
#[tauri::command]
pub fn clientbar_mini(app: AppHandle, name: String, on: bool) {
    change(&app, |s| {
        if on {
            s.mini.insert(name);
        } else {
            s.mini.remove(&name);
        }
    });
}

/// 📹 Camera on / off for this client: tells the mapper's Share a camera page (the cameras
/// window), which turns on / off every camera and watch area set up for this client there.
#[tauri::command]
pub fn clientbar_camera(app: AppHandle, name: String, on: bool) -> Result<String, String> {
    if !crate::incognito::is_public_name(&name) {
        return Err(format!("{name} is incognito (not on your SeAT accounts or mapper login), so it isn't shared with the mapper's cameras."));
    }
    if !crate::camera_share(&app)? {
        return Ok("Opening Share a camera… press again once it has loaded.".into());
    }
    let view = app.get_webview("cameras").ok_or("Share a camera closed")?;
    let detail = serde_json::json!({ "client": name, "on": on });
    view.eval(format!("window.dispatchEvent(new CustomEvent('devils-rage-app:camera', {{ detail: {detail} }}));")).map_err(|e| e.to_string())?;
    Ok(if on {
        format!("Asked Share a camera to turn on {name}'s cameras. If nothing starts, share {name}'s EVE window there once.")
    } else {
        format!("Asked Share a camera to turn off {name}'s cameras.")
    })
}

#[tauri::command]
pub fn clientbar_corner(app: AppHandle, corner: String, name: Option<String>) -> Result<(), String> {
    if !["left", "center", "right"].contains(&corner.as_str()) {
        return Err("left, center or right".into());
    }
    change(&app, |s| {
        s.corner = corner;
        // Left / Middle / Right also bring a bar dragged to a spot of its own back.
        if let Some(name) = &name {
            s.offsets.remove(name);
        }
    });
    Ok(())
}

/// ⋮⋮ held on the small bar: the bar follows the mouse; let go and it keeps that spot on its client.
#[tauri::command]
pub fn clientbar_move_bar(app: AppHandle, window: tauri::WebviewWindow, name: String) -> Result<(), String> {
    let handle = handle_of(&name).ok_or("that client is closed")?;
    let frame = win::frame(handle).ok_or("that client is minimised")?;
    let start = window.outer_position().map_err(|e| e.to_string())?;
    let mouse0 = win::cursor().ok_or("no mouse position")?;
    *BAR_MOVING.lock().unwrap() = Some(window.label().to_string());
    std::thread::Builder::new()
        .name("bar-move".into())
        .spawn(move || {
            let mut at = (start.x, start.y);
            for _ in 0..(10 * 60 * 125) {
                if !win::button_down() {
                    break;
                }
                if let Some(now) = win::cursor() {
                    let to = (start.x + now.0 - mouse0.0, start.y + now.1 - mouse0.1);
                    if to != at {
                        at = to;
                        let _ = window.set_position(PhysicalPosition::new(to.0, to.1));
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            change(&app, |s| {
                s.offsets.insert(name, (at.0 - frame.x, at.1 - frame.y));
            });
            *BAR_MOVING.lock().unwrap() = None;
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A login screen's settings profile (from its start-up line), so the bar can name its account.
#[tauri::command]
pub fn clientbar_login_profile(name: String) -> Option<String> {
    handle_of(&name).and_then(crate::startline::profile_of_window)
}

/// Put a login screen where one of its account's characters is remembered (first one found).
#[tauri::command]
pub fn clientbar_place_like(app: AppHandle, name: String, like: Vec<String>) -> bool {
    let Some(handle) = handle_of(&name) else { return false };
    // Its own spot first (an account's login screen remembers where you put it).
    if crate::clientwindows::keeps_a_spot(&name) {
        if let Some(spot) = crate::clientwindows::remembered(&app, &name) {
            log::info!("{name}: to its own spot");
            return crate::clientwindows::place_in_spot(&app, &name, handle, spot);
        }
    }
    // Else near one of its account's characters. Their spot may be from the other display mode
    // (a Fixed Window spot is the bare client; a Window-mode one has Windows' title bar and
    // invisible borders), so only move it there and snap it; never take that size.
    let (Some(rect), Some(frame)) = (win::window_rect(handle), win::frame(handle)) else { return false };
    for character in like.iter().take(10) {
        let Some(spot) = crate::clientwindows::remembered(&app, character) else { continue };
        let scale = app.get_webview_window(crate::MAIN_WINDOW).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
        let to = like_spot(rect, frame, spot, win::captioned(handle), scale, win::work_area(Spot { x: spot.x, y: spot.y, w: frame.w, h: frame.h }));
        log::info!("{name}: near {character}'s spot");
        return crate::clientwindows::place_window(handle, to);
    }
    false
}

/// Where a client goes for a spot remembered in the other display mode: moved there and snapped,
/// its own size kept (see `like_spot`).
pub fn near_spot(app: &AppHandle, handle: isize, spot: Spot) -> Option<Spot> {
    let (rect, frame) = (win::window_rect(handle)?, win::frame(handle)?);
    let scale = app.get_webview_window(crate::MAIN_WINDOW).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
    Some(like_spot(rect, frame, spot, win::captioned(handle), scale, win::work_area(Spot { x: spot.x, y: spot.y, w: frame.w, h: frame.h })))
}

/// Where a login screen goes near a character's remembered `spot`: its own size kept, its
/// visible frame (with our title bar when it's Fixed Window) snapped on that screen.
pub fn like_spot(rect: Spot, frame: Spot, spot: Spot, captioned: bool, scale: f64, work: Option<Spot>) -> Spot {
    let at = Spot { x: spot.x, y: spot.y, w: frame.w, h: frame.h };
    let (fx, fy) = match work {
        Some(work) if captioned => snap(at, work),
        Some(work) => {
            let (top, side) = chrome(scale);
            let (ox, oy) = snap(outer_of(at, scale), work);
            (ox + side, oy + top)
        }
        None => (at.x, at.y),
    };
    Spot { x: rect.x + (fx - frame.x), y: rect.y + (fy - frame.y), w: rect.w, h: rect.h }
}

/// Patch 74: an account's login screen remembers where you put it (snapped or dragged by Windows'
/// own title bar too) once it has sat still a second. Not in its first seconds (while it's being
/// put in its spot) and not while a mouse button is down.
struct LoginWatch {
    first: Instant,
    last: Spot,
    changed: Instant,
    saved: bool,
}

const LOGIN_SETTLE_GRACE: Duration = Duration::from_secs(6);

fn remember_moved_logins(app: &AppHandle, watch: &mut HashMap<isize, LoginWatch>) {
    let logins: Vec<(String, isize)> = crate::clipboard::open_clients().into_iter().filter(|(n, _)| n.starts_with("Login ") && crate::clientwindows::keeps_a_spot(n)).collect();
    watch.retain(|h, _| logins.iter().any(|(_, handle)| handle == h));
    let now = Instant::now();
    for (name, handle) in logins {
        let Some(rect) = crate::clientwindows::window_spot(handle) else { continue };
        let entry = watch.entry(handle).or_insert(LoginWatch { first: now, last: rect, changed: now, saved: true });
        if now.duration_since(entry.first) < LOGIN_SETTLE_GRACE {
            entry.last = rect;
            continue;
        }
        if rect != entry.last {
            entry.last = rect;
            entry.changed = now;
            entry.saved = false;
            continue;
        }
        if entry.saved || now.duration_since(entry.changed) < Duration::from_secs(1) || win::button_down() || DRAGGING.lock().unwrap().is_some() {
            continue;
        }
        entry.saved = true;
        crate::clientwindows::remember_one(app, &name, rect);
        log::info!("{name}: remembered its spot {}x{} at {},{}", rect.w, rect.h, rect.x, rect.y);
    }
}

/// The mapper's "Pop out (on top of EVE)" for this client's character.
#[tauri::command]
pub fn clientbar_popout(app: AppHandle, id: i64, name: String) -> Result<String, String> {
    if !crate::incognito::is_public_name(&name) || !crate::incognito::is_public_id(id.max(0) as u64) {
        return Err(format!("{name} is incognito (not on your SeAT accounts or mapper login), so the mapper doesn't get it."));
    }
    let view = crate::main_view(&app).ok_or("the mapper isn't open in the app")?;
    let on_mapper = view.url().ok().is_some_and(|u| u.path().starts_with("/maps/"));
    if !on_mapper {
        return Err("Open your map in the app first, then try again.".into());
    }
    let detail = serde_json::json!({ "id": id, "name": name });
    view.eval(format!("window.dispatchEvent(new CustomEvent('devils-rage-app:popout', {{ detail: {detail} }}));")).map_err(|e| e.to_string())?;
    Ok(format!("Asked the mapper for {name}'s pop-out."))
}

/// Experiment (Tsa, 17 Nov): resize a Fixed Window client to `width`×`height` without restarting,
/// keeping its top-left. EVE may or may not keep that size; nothing is written to its files.
#[tauri::command]
pub fn clientbar_resize_live(name: String, width: i32, height: i32) -> Result<String, String> {
    if !(640..=7680).contains(&width) || !(480..=4320).contains(&height) {
        return Err("That size is outside 640×480 – 7680×4320.".into());
    }
    let handle = handle_of(&name).ok_or("that client isn't open")?;
    if win::captioned(handle) {
        return Err("That's for Fixed Window clients.".into());
    }
    let rect = win::window_rect(handle).ok_or("restore the client first (it's minimised)")?;
    win::resize(handle, Spot { x: rect.x, y: rect.y, w: width, h: height });
    std::thread::sleep(Duration::from_millis(600));
    let now = win::client_size(handle).unwrap_or((0, 0));
    Ok(if now == (width, height) {
        format!("{name} is {width}×{height} now. If EVE keeps it after a while (and after a restart), we don't need the restart step.")
    } else {
        format!("Windows says {}×{} now: EVE didn't take {width}×{height}.", now.0, now.1)
    })
}

/// This client's details for the bar's ⚙.
#[tauri::command]
pub fn clientbar_info(app: AppHandle, name: String) -> serde_json::Value {
    let handle = handle_of(&name);
    let floating = settings(&app).floating.contains(&name);
    let frame = handle.and_then(win::frame);
    serde_json::json!({
        "name": name,
        "open": handle.is_some(),
        "locked": locked(&name),
        "frame": frame.map(|f| serde_json::json!({ "x": f.x, "y": f.y, "w": f.w, "h": f.h })),
        "windowMode": handle.map(win::captioned),
        "size": handle.and_then(win::client_size).map(|(w, h)| serde_json::json!({ "w": w, "h": h })),
        "floating": floating,
        "titleBar": MODES.lock().unwrap().as_ref().and_then(|m| m.get(&name).copied()).map(|(on, top)| serde_json::json!({ "on": on, "top": top })),
    })
}

#[tauri::command]
pub fn clientbar_lock(name: String, locked: bool) {
    let mut guard = LOCKED.lock().unwrap();
    let set = guard.get_or_insert_with(HashSet::new);
    if locked {
        set.insert(name);
    } else {
        set.remove(&name);
    }
}

/// Which buttons the bars show (⚙ → Buttons on the bar).
#[tauri::command]
pub fn clientbar_buttons(app: AppHandle, buttons: Vec<String>) -> Result<(), String> {
    let mut picked: Vec<String> = BUTTONS.iter().filter(|b| buttons.iter().any(|x| x == *b)).map(|b| b.to_string()).collect();
    picked.dedup();
    change(&app, |s| s.buttons = picked);
    Ok(())
}

/// A menu opened or closed: the bar grows to fit it.
#[tauri::command]
pub fn clientbar_menu(window: tauri::WebviewWindow, height: Option<f64>) {
    let label = window.label().to_string();
    let mut guard = TALL.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    match height {
        Some(h) => {
            map.insert(label, h.clamp(BAR_H, 900.0));
        }
        None => {
            map.remove(&label);
        }
    }
}

/// The see-through outline shown while dragging: where the client will land (grey and filled
/// when it will snap there, like Windows' snap preview). Moving it is cheap, so it keeps up with
/// the mouse; the client itself moves once, when let go.
const GHOST: &str = "dragghost";

fn ghost(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window(GHOST) {
        return Some(window);
    }
    let window = WebviewWindowBuilder::new(app, GHOST, WebviewUrl::App("ghost.html".into()))
        .title("move preview")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|err| log::warn!("couldn't open the move preview: {err}"))
        .ok()?;
    let _ = window.set_ignore_cursor_events(true);
    Some(window)
}

/// ✥ pressed: an outline follows the mouse while its button is held (snapping shown in grey);
/// let go and the client moves there once, at its exact size, and is remembered at its new spot.
#[tauri::command]
pub fn clientbar_drag(app: AppHandle, name: String) -> Result<(), String> {
    if locked(&name) {
        return Err("This client is locked in place (⚙ → Unlock).".into());
    }
    let handle = handle_of(&name).ok_or("that client is closed")?;
    let rect0 = win::window_rect(handle).ok_or("that client is minimised")?;
    let frame0 = win::frame(handle).ok_or("that client is minimised")?;
    let mouse0 = win::cursor().ok_or("no mouse position")?;
    // A Fixed Window client snaps with our title bar and edge round it (1918×1024 → 1920×1056).
    let scale = app.get_webview_window(&label_for(&name)).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
    let framed = !win::captioned(handle) && !settings(&app).floating.contains(&name);
    let outer0 = if framed { outer_of(frame0, scale) } else { frame0 };
    *DRAGGING.lock().unwrap() = Some(name.clone());
    std::thread::Builder::new()
        .name("client-drag".into())
        .spawn(move || {
            let outline = ghost(&app);
            let mut last = mouse0;
            let mut target = outer0;
            let mut moved = false;
            let mut snapping: Option<bool> = None;
            let mut placed: Option<Spot> = None;
            // Up to 10 minutes of holding; stops the moment the button is let go.
            for _ in 0..(10 * 60 * 250) {
                if !win::button_down() {
                    break;
                }
                if let Some(now) = win::cursor() {
                    if now != last {
                        last = now;
                        let (dx, dy) = (now.0 - mouse0.0, now.1 - mouse0.1);
                        moved |= dx.abs() > 3 || dy.abs() > 3;
                        if moved {
                            let raw = Spot { x: outer0.x + dx, y: outer0.y + dy, ..outer0 };
                            let (sx, sy) = win::work_area(raw).map_or((raw.x, raw.y), |work| snap(raw, work));
                            target = Spot { x: sx, y: sy, ..outer0 };
                            if let Some(window) = &outline {
                                let snaps = (sx, sy) != (raw.x, raw.y);
                                if snapping != Some(snaps) {
                                    snapping = Some(snaps);
                                    let _ = window.eval(format!("window.mode && window.mode({snaps}, {})", serde_json::Value::String(name.clone())));
                                }
                                if placed != Some(target) {
                                    if placed.is_none() || placed.is_some_and(|p| p.w != target.w || p.h != target.h) {
                                        let _ = window.set_size(tauri::PhysicalSize::new(target.w.max(1) as u32, target.h.max(1) as u32));
                                    }
                                    let _ = window.set_position(PhysicalPosition::new(target.x, target.y));
                                    if placed.is_none() {
                                        let _ = window.show();
                                    }
                                    placed = Some(target);
                                }
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(4));
            }
            if let Some(window) = &outline {
                let _ = window.hide();
            }
            if moved {
                let to = Spot { x: rect0.x + (target.x - outer0.x), y: rect0.y + (target.y - outer0.y), w: rect0.w, h: rect0.h };
                crate::clientwindows::place_window(handle, to);
                crate::clientwindows::remember_one(&app, &name, to);
            }
            *DRAGGING.lock().unwrap() = None;
            crate::capture::focus_handle(handle);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ask a client window to close (its ✕ / Alt+F4); EVE may ask to confirm.
pub fn close_window(handle: isize) -> bool {
    win::close(handle)
}

/// A Fixed Window client: our title bar (false) or the small floating bar (true).
#[tauri::command]
pub fn clientbar_floating(app: AppHandle, name: String, on: bool) {
    change(&app, |s| {
        if on {
            s.floating.insert(name);
        } else {
            s.floating.remove(&name);
        }
    });
}

/// The Devil's window mode (windowfix.rs): close the client, switch its profile to Fixed Window at
/// `width`×`height` (or back to Window mode), and put it in the same spot when it opens again.
#[tauri::command]
pub fn clientbar_window_mode(app: AppHandle, name: String, to_fixed: bool, width: i64, height: i64, floating: bool, profile: Option<String>) -> Result<String, String> {
    if !(640..=7680).contains(&width) || !(480..=4320).contains(&height) {
        return Err("That size is outside 640×480 – 7680×4320.".into());
    }
    let handle = handle_of(&name).ok_or("that client isn't open")?;
    let frame = win::frame(handle).ok_or("restore the client first (it's minimised)")?;
    let scale = app.get_webview_window(&label_for(&name)).and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
    let (top, side) = chrome(scale);
    let captioned = win::captioned(handle);
    let framed_now = !captioned && !settings(&app).floating.contains(&name);
    let to = if to_fixed {
        // The window's spot (title bar included) becomes our title bar + the client inside it.
        let (x, y) = if floating { (frame.x, frame.y) } else { (frame.x + side, frame.y + top) };
        Spot { x, y, w: width as i32, h: height as i32 }
    } else {
        // Windows' title bar goes where ours was. w/h 0: a visible-frame spot, sized by EVE.
        let (x, y) = if framed_now { (frame.x - side, frame.y - top) } else { (frame.x, frame.y) };
        Spot { x, y, w: 0, h: 0 }
    };
    change(&app, |s| {
        if floating {
            s.floating.insert(name.clone());
        } else {
            s.floating.remove(&name);
        }
    });
    let profile = profile.filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    crate::windowfix::run(app, name.clone(), handle, to_fixed, width, height, to, profile)?;
    Ok(format!("Closing {name}… (if EVE asks, pick Quit). The app changes its profile once it's closed and tells you when to launch it again."))
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
    fn a_login_screen_near_a_fixed_characters_spot_keeps_its_size() {
        let work = Spot { x: 0, y: 0, w: 3840, h: 2112 };
        // Tsawind is Fixed Window: his spot is the bare 1918x1024 client under our title bar.
        let tsawind = Spot { x: 1, y: 31, w: 1918, h: 1024 };
        // A Window-mode login screen (Windows' 7 px invisible borders round its frame) goes to the
        // corner as it is, not squeezed into Tsawind's rectangle (that gave 1902x985).
        let rect = Spot { x: 493, y: 500, w: 1934, h: 1063 };
        let frame = Spot { x: 500, y: 500, w: 1920, h: 1056 };
        assert_eq!(like_spot(rect, frame, tsawind, true, 1.0, Some(work)), Spot { x: -7, y: 0, w: 1934, h: 1063 });
        // A Fixed Window login screen goes under room for our title bar.
        let fixed = Spot { x: 500, y: 500, w: 1918, h: 1024 };
        assert_eq!(like_spot(fixed, fixed, tsawind, false, 1.0, Some(work)), Spot { x: 1, y: 31, w: 1918, h: 1024 });
    }

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
        assert_eq!(bar_spot(f, "left", 1.0, BAR_W), (64, 0));
        assert_eq!(bar_spot(f, "center", 1.0, BAR_W), (810, 0));
        assert_eq!(bar_spot(f, "right", 1.0, BAR_W), (1612, 0));
        assert_eq!(bar_spot(f, "right", 1.0, BAR_MINI_W), (1812, 0));
        assert_eq!(bar_spot(f, "left", 1.5, BAR_W), (96, 0));
    }

    #[test]
    fn every_client_by_default_and_old_settings_move_to_the_right() {
        let fresh = BarSettings::default();
        assert!(fresh.every && fresh.corner == "right");
        let old: BarSettings = serde_json::from_str(r#"{"clients":["Tsawind"],"corner":"left"}"#).unwrap();
        let old = old.upgrade();
        assert!(old.every);
        assert_eq!(old.corner, "right");
        let kept: BarSettings = serde_json::from_str(r#"{"every":false,"clients":["Tsawind"],"corner":"left","version":1}"#).unwrap();
        assert_eq!(kept.clone().upgrade().corner, "left");
        assert!(!kept.upgrade().every);
    }

    #[test]
    fn fixed_window_1918_gets_our_title_bar_and_fills_a_4k_quarter() {
        let work = Spot { x: 0, y: 0, w: 3840, h: 2112 };
        let client = Spot { x: 1921, y: 1087, w: 1918, h: 1024 };
        let p = place_for(client, false, false, Some(work), "right", 1.0, BAR_W, BAR_H);
        assert!(p.frame);
        assert_eq!((p.x, p.y, p.w, p.h, p.top), (1920, 1056, 1920, 1056, 31));
        // dropped near the bottom-right quarter: the whole 1920×1056 snaps there
        let outer = outer_of(Spot { x: 1950, y: 1070, ..client }, 1.0);
        assert_eq!(snap(outer, work), (1920, 1056));
    }

    #[test]
    fn small_bar_when_there_is_no_room_above_or_a_windows_title_bar() {
        let work = Spot { x: 0, y: 0, w: 3840, h: 2112 };
        let tsawind = Spot { x: 0, y: 0, w: 1920, h: 1056 };
        let p = place_for(tsawind, false, false, Some(work), "right", 1.0, BAR_W, BAR_H);
        assert!(!p.frame);
        assert_eq!((p.x, p.y), (1612, 0));
        let windowed = Spot { x: 0, y: 40, w: 1920, h: 1056 };
        let w = place_for(windowed, true, false, Some(work), "right", 1.0, BAR_W, BAR_H);
        assert!(!w.frame);
        // Window mode: left of Windows' own –, ☐, ✕
        assert_eq!(w.x, 1612 - 140);
    }

    #[test]
    fn default_bar_is_move_and_close() {
        let s = BarSettings::default();
        assert_eq!(s.buttons, vec!["move".to_string(), "close".to_string()]);
        assert_eq!(bar_width(&s.buttons), 164.0);
        let old: BarSettings = serde_json::from_str(r#"{"corner":"right","version":1}"#).unwrap();
        assert_eq!(old.buttons, s.buttons);
    }

    #[test]
    fn switching_modes_keeps_the_spot() {
        // Window mode in the bottom-right 4K quarter: visible frame 1920×1056 at 1920,1056.
        let window_frame = Spot { x: 1920, y: 1056, w: 1920, h: 1056 };
        // EVE switches to Fixed Window 1918×1024 and drops it at the monitor's top left.
        let fixed = Spot { x: 0, y: 0, w: 1918, h: 1024 };
        assert_eq!(after_switch(true, window_frame, fixed, fixed, 1.0), Spot { x: 1921, y: 1087, w: 1918, h: 1024 });
        // And back: Window mode's rect has 7 px invisible borders round the visible frame.
        let rect = Spot { x: -7, y: 0, w: 1934, h: 1063 };
        let frame = Spot { x: 0, y: 0, w: 1920, h: 1056 };
        let to = after_switch(false, Spot { x: 1921, y: 1087, w: 1918, h: 1024 }, rect, frame, 1.0);
        assert_eq!((to.x, to.y), (1913, 1056));
    }

    #[test]
    fn a_fixed_login_screen_in_the_corner_moves_down_for_our_title_bar() {
        let work = Spot { x: 0, y: 0, w: 3840, h: 2112 };
        let login = Spot { x: 0, y: 0, w: 1918, h: 1024 };
        assert_eq!(nudge_for_title_bar(login, work, 1.0), Some(Spot { x: 1, y: 31, w: 1918, h: 1024 }));
        // already room above: nothing
        assert_eq!(nudge_for_title_bar(Spot { y: 1087, ..login }, work, 1.0), None);
        // a 1920×1056 client wouldn't fit its quarter with our bar: left alone
        assert_eq!(nudge_for_title_bar(Spot { x: 0, y: 0, w: 1920, h: 1056 }, work, 1.0), None);
    }

    #[test]
    fn names_make_safe_labels() {
        assert_eq!(label_for("Darkness Holder"), "clientbar-DarknessHolder");
        assert_eq!(label_for("Tsawind"), "clientbar-Tsawind");
    }
}
