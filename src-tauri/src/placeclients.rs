//! 23 Nov: Place clients. One small window that steps through every client (or every preview)
//! one at a time, so a stack can be set up exactly on top of each other:
//! - the one being placed is brought forward; the others' frames and names are drawn on a
//!   see-through layer (previewtools.rs), and the others can also be faded to 10 %, minimised,
//!   or left as they are;
//! - Same spot as previous, Snap to the grid, Set the whole stack (everyone in its stacks gets
//!   this spot), nudge by 1 / 10 px, Save spots (each client's spot is remembered).
//!
//! Only window positions, sizes and see-through styles change; nothing is clicked or typed in
//! EVE. Everything faded or minimised is put back when you move on and when the window closes.

use crate::clientwindows::Spot;
use crate::previewtools::{cell_target, monitor_of, monitors, preview_rect, set_outlines, set_preview_rect};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// EVE's own window sizes (its Display list), and its recommended smallest.
pub const EVE_SIZES: &[(i32, i32)] = &[
    (1024, 768), (1152, 864), (1280, 720), (1280, 768), (1280, 800), (1280, 960), (1280, 1024), (1360, 768), (1366, 768), (1440, 900), (1440, 1080),
    (1600, 900), (1600, 1024), (1600, 1200), (1680, 1050), (1920, 1080), (1920, 1200), (1920, 1440), (2048, 1536), (2560, 1440), (2560, 1600), (3840, 2160),
];
pub const EVE_MIN: (i32, i32) = (1024, 768);

const WINDOW: &str = "place";

#[derive(Default)]
struct Placing {
    previews: bool,
    index: usize,
    /// "outline" (only the frames on the layer), "fade" (10 %), "hide" (minimise), "normal".
    others: String,
    /// Windows we faded, with their style before.
    faded: Vec<(isize, isize)>,
    /// Windows we minimised.
    minimised: Vec<isize>,
    /// Previews we hid or faded.
    ghosted: Vec<String>,
    /// We turned arranging on for Place previews (turned off again when done).
    edit_on: bool,
    /// Window zones across x down each monitor (client windows).
    zones: (u32, u32),
    /// Window mode picked here per client (true = Fixed Window), saved with its spot.
    modes: HashMap<String, bool>,
}

static PLACING: Mutex<Option<Placing>> = Mutex::new(None);

// ---- What's being placed ------------------------------------------------------------------------

/// 23 Nov: everyone, open or not: each account's login screen ("Login 3") then its characters,
/// in Your accounts order, then any other client open now. A client that isn't open is placed
/// with a stand-in frame (client windows) or its preview opened empty (previews).
fn names(app: &AppHandle, _previews: bool) -> Vec<String> {
    let seat = crate::launch::load_seat(app);
    let mut accounts: Vec<&crate::launch::SeatAccount> = seat.accounts.iter().collect();
    accounts.sort_by_key(|a| crate::launch::nickname_key(&a.nickname));
    let mut list: Vec<String> = Vec::new();
    for (i, a) in accounts.iter().enumerate() {
        list.push(crate::clipboard::login_label(i));
        for c in &a.characters {
            if !c.name.is_empty() {
                list.push(c.name.clone());
            }
        }
    }
    let mut extra: Vec<String> = crate::clipboard::open_clients().into_iter().map(|(n, _)| n).filter(|n| !list.contains(n)).collect();
    extra.sort_by_key(|n| n.to_lowercase());
    list.extend(extra);
    let mut seen = std::collections::HashSet::new();
    list.retain(|n| seen.insert(n.clone()));
    list
}

fn is_open(name: &str) -> bool {
    handle_of(name).is_some()
}

fn handle_of(name: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h)
}

fn rect_of(app: &AppHandle, previews: bool, name: &str) -> Option<Spot> {
    if previews {
        return app.get_webview_window(&crate::previews::label_for(name)).and_then(|w| preview_rect(&w));
    }
    match handle_of(name) {
        Some(h) => crate::clientwindows::window_spot(h),
        None if standin_for().as_deref() == Some(name) => standin_rect(app),
        None => None,
    }
}

fn move_to(app: &AppHandle, previews: bool, name: &str, to: Spot) -> bool {
    if previews {
        set_preview_rect(app, name, to);
        return true;
    }
    match handle_of(name) {
        Some(h) => crate::clientwindows::place_window(h, to),
        None if standin_for().as_deref() == Some(name) => {
            let fixed = STANDIN_FIXED.lock().unwrap().unwrap_or(true);
            set_standin(app, name, to, fixed);
            true
        }
        None => false,
    }
}

// ---- The stand-in: a frame for a client that isn't open -----------------------------------------

const STANDIN: &str = "standin";
static STANDIN_FOR: Mutex<Option<String>> = Mutex::new(None);
/// The window mode the stand-in shows now.
static STANDIN_FIXED: Mutex<Option<bool>> = Mutex::new(None);

fn standin_for() -> Option<String> {
    STANDIN_FOR.lock().unwrap().clone()
}

fn standin_rect(app: &AppHandle) -> Option<Spot> {
    app.get_webview_window(STANDIN).and_then(|w| preview_rect(&w))
}

/// Show the stand-in frame for `name` at `at` (built the first time).
/// (`fixed`: its window mode, shown on the frame.) Never called with PLACING locked by the caller
/// unless `fixed` comes from that lock.
fn set_standin(app: &AppHandle, name: &str, at: Spot, fixed: bool) {
    *STANDIN_FOR.lock().unwrap() = Some(name.to_string());
    *SI_IGNORE.lock().unwrap() = Some(Instant::now() + Duration::from_millis(400));
    *STANDIN_FIXED.lock().unwrap() = Some(fixed);
    let arg = standin_info(app, name);
    if let Some(w) = app.get_webview_window(STANDIN) {
        let _ = w.set_position(tauri::PhysicalPosition::new(at.x, at.y));
        let _ = w.set_size(tauri::PhysicalSize::new(at.w.max(200) as u32, at.h.max(120) as u32));
        let frame = serde_json::json!(eve_frame());
        let _ = w.eval(format!("window.__standin && window.__standin({arg}); window.__standinMode && window.__standinMode({fixed}, {frame});"));
        let _ = w.show();
        raise_place(app);
        return;
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let built = WebviewWindowBuilder::new(&app2, STANDIN, WebviewUrl::App("standin.html".into()))
            .initialization_script(format!("window.__STANDIN__ = {arg};"))
            .title("Stand-in")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(true)
            .focused(false)
            .visible(false)
            .build();
        match built {
            Ok(w) => {
                crate::clientbar::tool_window(&w);
                let _ = w.set_position(tauri::PhysicalPosition::new(at.x, at.y));
                let _ = w.set_size(tauri::PhysicalSize::new(at.w.max(200) as u32, at.h.max(120) as u32));
                let _ = w.eval(format!("window.__standinMode && window.__standinMode({fixed}, {});", serde_json::json!(eve_frame())));
                w.on_window_event(|event| {
                    if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                        standin_changed();
                    }
                });
                let _ = w.show();
                crate::mainthread::run(raise_place);
            }
            Err(e) => log::warn!("couldn't open the stand-in frame: {e}"),
        }
    });
}

/// 23 Nov: what the stand-in draws, so it looks like that EVE client: a login screen names its
/// account and shows its characters; a character shows its name and portrait.
fn standin_info(app: &AppHandle, name: &str) -> serde_json::Value {
    let seat = crate::launch::load_seat(app);
    let mut accounts: Vec<&crate::launch::SeatAccount> = seat.accounts.iter().collect();
    accounts.sort_by_key(|a| crate::launch::nickname_key(&a.nickname));
    let login_no = name.strip_prefix("Login ").and_then(|n| n.parse::<usize>().ok());
    let acct = match login_no {
        Some(n) => n.checked_sub(1).and_then(|i| accounts.get(i).copied()),
        None => accounts.iter().find(|a| a.characters.iter().any(|c| c.name == name)).copied(),
    };
    let infos = crate::pcaccounts::infos(app);
    let login = acct.and_then(|a| a.launcher_user_id).and_then(|u| infos.get(&u)).and_then(|i| i.login.clone());
    let chars: Vec<serde_json::Value> = acct.map(|a| a.characters.iter().map(|c| serde_json::json!({ "id": c.id, "name": c.name })).collect()).unwrap_or_default();
    let id = acct.and_then(|a| a.characters.iter().find(|c| c.name == name)).map(|c| c.id);
    serde_json::json!({
        "name": name,
        "login": login_no.is_some(),
        "account": acct.map(|a| a.nickname.clone()),
        "eveLogin": login,
        "chars": chars,
        "id": id,
    })
}

/// Keep the Place clients window above the stand-in and the layer.
fn raise_place(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.set_always_on_top(false);
        let _ = w.set_always_on_top(true);
    }
}

fn hide_standin(app: &AppHandle) {
    *STANDIN_FOR.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window(STANDIN) {
        let _ = w.hide();
    }
}

/// Where a client that isn't open would go: its saved spot, else the previous one's, else a
/// 1280 x 720 frame on the first monitor.
fn spot_for_closed(app: &AppHandle, list: &[String], index: usize, name: &str) -> Spot {
    if let Some(s) = crate::clientwindows::remembered(app, name) {
        return s;
    }
    if index > 0 {
        if let Some(s) = rect_of(app, false, &list[index - 1]).or_else(|| crate::clientwindows::remembered(app, &list[index - 1])) {
            return s;
        }
    }
    let m = monitors(app).first().copied().unwrap_or(Spot { x: 0, y: 0, w: 1920, h: 1080 });
    Spot { x: m.x + 40, y: m.y + 40, w: 1280.min(m.w), h: 720.min(m.h) }
}

// ---- Looks: fade, minimise, put back ---------------------------------------------------------------

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, IsIconic, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, LWA_ALPHA, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    /// Fade a window to `alpha` (0–255) and let clicks through it; returns its style before.
    pub fn fade(handle: isize, alpha: u8) -> isize {
        // SAFETY: style read / write and layered attributes on a top-level window handle.
        unsafe {
            let hwnd = handle as HWND;
            let before = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, before | WS_EX_LAYERED as isize | WS_EX_TRANSPARENT as isize);
            SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
            before
        }
    }

    pub fn unfade(handle: isize, before: isize) {
        // SAFETY: as above.
        unsafe { SetWindowLongPtrW(handle as HWND, GWL_EXSTYLE, before) };
    }

    pub fn minimise(handle: isize) -> bool {
        // SAFETY: plain window calls on a top-level window handle.
        unsafe {
            if IsIconic(handle as HWND) != 0 {
                return false;
            }
            ShowWindow(handle as HWND, SW_SHOWMINNOACTIVE);
        }
        true
    }

    pub fn unminimise(handle: isize) {
        // SAFETY: as above.
        unsafe { ShowWindow(handle as HWND, SW_SHOWNOACTIVATE) };
    }

    /// A window's frame (left, top, right, bottom): outer rectangle minus its inside.
    pub fn frame_of(handle: isize) -> Option<(i32, i32, i32, i32)> {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};
        // SAFETY: rectangle reads on a top-level window handle.
        unsafe {
            let hwnd = handle as HWND;
            let (mut outer, mut inner): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
            if GetWindowRect(hwnd, &mut outer) == 0 || GetClientRect(hwnd, &mut inner) == 0 {
                return None;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut origin);
            let f = (origin.x - outer.left, origin.y - outer.top, outer.right - (origin.x + inner.right), outer.bottom - (origin.y + inner.bottom));
            (f.1 > 0).then_some(f)
        }
    }

    /// A normal window's frame for this PC's scale (title bar and borders).
    pub fn frame_default() -> (i32, i32, i32, i32) {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{AdjustWindowRectEx, WS_OVERLAPPEDWINDOW};
        let mut r = RECT { left: 0, top: 0, right: 1000, bottom: 1000 };
        // SAFETY: a plain calculation.
        unsafe { AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, 0, 0) };
        (-r.left, -r.top, r.right - 1000, r.bottom - 1000)
    }
}

#[cfg(not(windows))]
mod win {
    pub fn fade(_h: isize, _a: u8) -> isize {
        0
    }
    pub fn unfade(_h: isize, _b: isize) {}
    pub fn minimise(_h: isize) -> bool {
        false
    }
    pub fn unminimise(_h: isize) {}
    pub fn frame_of(_h: isize) -> Option<(i32, i32, i32, i32)> {
        None
    }
    pub fn frame_default() -> (i32, i32, i32, i32) {
        (8, 31, 8, 8)
    }
}

/// 23 Nov: the title bar and borders of an EVE window in Window mode (left, top, right, bottom):
/// measured on an open one, else what Windows gives a normal window here. EVE's "Window Size"
/// is the inside; the stand-in and the app's spots are the outside.
pub fn eve_frame() -> (i32, i32, i32, i32) {
    crate::clipboard::open_clients()
        .into_iter()
        .filter(|(_, h)| crate::clientwindows::captioned_window(*h))
        .find_map(|(_, h)| win::frame_of(h))
        .unwrap_or_else(win::frame_default)
}

/// The size EVE shows for a window spot: the inside in Window mode, all of it in Fixed.
fn eve_size(spot: Spot, fixed: bool) -> (i32, i32) {
    if fixed {
        return (spot.w, spot.h);
    }
    let (l, t, r, b) = eve_frame();
    (spot.w - l - r, spot.h - t - b)
}

/// Put back everything we faded, minimised or ghosted.
fn put_back(app: &AppHandle, st: &mut Placing, keep_minimised: Option<isize>) {
    for (h, before) in st.faded.drain(..) {
        win::unfade(h, before);
    }
    st.minimised.retain(|h| {
        if Some(*h) == keep_minimised {
            return true;
        }
        win::unminimise(*h);
        false
    });
    for name in st.ghosted.drain(..) {
        if let Some(w) = app.get_webview_window(&crate::previews::label_for(&name)) {
            let _ = w.show();
            let _ = w.eval("window.__placeGhost && window.__placeGhost(null);");
        }
    }
}

/// Show the one being placed and treat the others as chosen; draw everyone's frame.
fn apply(app: &AppHandle, st: &mut Placing) {
    let list = names(app, st.previews);
    if list.is_empty() {
        put_back(app, st, None);
        set_outlines(app, Vec::new());
        return;
    }
    st.index = st.index.min(list.len() - 1);
    let current = list[st.index].clone();
    // Restore last step's looks first (but leave others minimised when we stay in "hide").
    put_back(app, st, None);
    if st.previews || is_open(&current) {
        hide_standin(app);
    } else {
        let at = spot_for_closed(app, &list, st.index, &current);
        let fixed = mode_of(app, st, &current);
        set_standin(app, &current, at, fixed);
    }
    if st.previews && app.get_webview_window(&crate::previews::label_for(&current)).is_none() {
        // Its client isn't open: open its preview (empty) so it can be put where it goes; one
        // never placed goes just right of the previous one (not Windows' default corner).
        let next_to = if crate::previews::prefs_for(app, &current).x.is_none() && st.index > 0 {
            rect_of(app, true, &list[st.index - 1]).map(|r| Spot { x: r.x + r.w + 6, ..r })
        } else {
            None
        };
        let (app2, name) = (app.clone(), current.clone());
        tauri::async_runtime::spawn(async move {
            crate::previews::build_preview(&app2, &name);
            if let Some(to) = next_to {
                crate::mainthread::run(move |app| set_preview_rect(app, &name, to));
            }
        });
    }
    if st.previews {
        for name in list.iter().filter(|n| **n != current) {
            let Some(w) = app.get_webview_window(&crate::previews::label_for(name)) else { continue };
            match st.others.as_str() {
                "hide" => {
                    let _ = w.hide();
                }
                "fade" | "outline" => {
                    let _ = w.eval("window.__placeGhost && window.__placeGhost(0.12);");
                }
                _ => continue,
            }
            st.ghosted.push(name.clone());
        }
    } else {
        if let Some(h) = handle_of(&current) {
            win::unminimise(h);
        }
        for name in list.iter().filter(|n| **n != current) {
            let Some(h) = handle_of(name) else { continue };
            match st.others.as_str() {
                "fade" => st.faded.push((h, win::fade(h, 26))),
                "hide" => {
                    if win::minimise(h) {
                        st.minimised.push(h);
                    }
                }
                _ => {}
            }
        }
        // Bring the one being placed forward, as clicking its preview does.
        let _ = crate::previews::preview_focus(current.clone());
        if let Some(w) = app.get_webview_window(WINDOW) {
            let _ = w.set_always_on_top(true);
            // Keys (arrows, Page Up / Down) keep going to this window.
            let _ = w.set_focus();
        }
    }
    draw_outlines(app, st.previews, st.zones, &list, &current);
}

fn draw_outlines(app: &AppHandle, previews: bool, zones: (u32, u32), list: &[String], current: &str) {
    crate::previewtools::set_zones(app, if previews { None } else { Some(zones) });
    let open = open_map();
    let outlines: Vec<(Spot, String, bool)> = list
        .iter()
        .filter(|n| *n == current || open.contains_key(*n))
        .filter_map(|n| {
            let r = match (previews, open.get(n)) {
                (false, Some(h)) => crate::clientwindows::window_spot(*h),
                _ => rect_of(app, previews, n),
            };
            r.map(|r| (r, n.clone(), n == current))
        })
        .collect();
    set_outlines(app, outlines);
}

/// The clients open now, by name (one look at the windows).
fn open_map() -> std::collections::HashMap<String, isize> {
    crate::clipboard::open_clients().into_iter().collect()
}

// ---- Commands -----------------------------------------------------------------------------------

/// Open Place clients (`previews`: step through the previews instead of the client windows).
#[tauri::command]
pub fn place_clients_open(app: AppHandle, previews: Option<bool>) {
    let previews = previews.unwrap_or(false);
    {
        let mut guard = PLACING.lock().unwrap();
        if let Some(st) = guard.as_mut() {
            put_back(&app, st, None);
        }
        *guard = Some(Placing { previews, others: "outline".into(), zones: (2, 2), ..Placing::default() });
    }
    if previews && !crate::previews::edit_mode(&app) {
        let _ = crate::previews::set_preview_edit(app.clone(), true);
        if let Some(st) = PLACING.lock().unwrap().as_mut() {
            st.edit_on = true;
        }
    }
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.set_title(if previews { "Preview setup" } else { "Client setup" });
        let _ = w.eval("window.__reload && window.__reload();");
        let _ = w.show();
        let _ = w.set_focus();
        step(&app);
        return;
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let built = WebviewWindowBuilder::new(&app2, WINDOW, WebviewUrl::App("place.html".into()))
            .title(if previews { "Preview setup" } else { "Client setup" })
            .inner_size(560.0, 760.0)
            .min_inner_size(420.0, 420.0)
            .always_on_top(true)
            .visible(true)
            .build();
        match built {
            Ok(w) => {
                w.on_window_event(|event| {
                    if let WindowEvent::Destroyed = event {
                        crate::mainthread::run(done);
                    }
                });
                crate::mainthread::run(step);
            }
            Err(e) => log::warn!("couldn't open Place clients: {e}"),
        }
    });
}

fn step(app: &AppHandle) {
    let mut guard = PLACING.lock().unwrap();
    if let Some(st) = guard.as_mut() {
        apply(app, st);
    }
}

fn done(app: &AppHandle) {
    let taken = PLACING.lock().unwrap().take();
    if let Some(mut st) = taken {
        put_back(app, &mut st, None);
        if st.edit_on {
            let _ = crate::previews::set_preview_edit(app.clone(), false);
        }
    }
    hide_standin(app);
    crate::previewtools::set_zones(app, None);
    set_outlines(app, Vec::new());
    log::info!("Place clients closed: everything put back");
}

/// What the window shows (it asks every second, so moved windows' frames follow).
#[tauri::command]
pub fn place_state(app: AppHandle) -> serde_json::Value {
    let guard = PLACING.lock().unwrap();
    let Some(st) = guard.as_ref() else { return serde_json::json!({ "open": false }) };
    let list = names(&app, st.previews);
    let index = st.index.min(list.len().saturating_sub(1));
    let current = list.get(index).cloned();
    if let Some(c) = &current {
        draw_outlines(&app, st.previews, st.zones, &list, c);
    }
    let remembered: Vec<bool> = list
        .iter()
        .map(|n| if st.previews { crate::previews::prefs_for(&app, n).x.is_some() } else { crate::clientwindows::remembered(&app, n).is_some() })
        .collect();
    let open_now = open_map();
    let open: Vec<bool> = list.iter().map(|n| open_now.contains_key(n)).collect();
    let (snap_on, cols, rows) = crate::previews::with_layout(&app, |l| (l.snap_on, l.snap_cols, l.snap_rows));
    serde_json::json!({
        "open": true,
        "previews": st.previews,
        "names": list,
        "remembered": remembered,
        "isOpen": open,
        "standin": current.as_deref().is_some_and(|c| standin_for().as_deref() == Some(c)),
        "index": index,
        "current": current,
        "rect": current.as_deref().and_then(|c| rect_of(&app, st.previews, c)),
        "others": st.others,
        "grid": [cols, rows],
        "snapOn": snap_on,
        "zones": [st.zones.0, st.zones.1],
        "fixed": current.as_deref().map(|c| mode_of(&app, st, c)),
        "eveSize": current.as_deref().and_then(|c| rect_of(&app, st.previews, c).map(|r| eve_size(r, mode_of(&app, st, c)))),
        "frame": eve_frame(),
        "sizes": EVE_SIZES,
        "min": [EVE_MIN.0, EVE_MIN.1],
        "monitors": monitors(&app),
        "profile": current.as_deref().filter(|_| !st.previews).and_then(|c| profile_of_client(&app, c)).map(|(p, shared)| serde_json::json!({ "name": p, "shared": shared })),
    })
}

/// The window mode picked for a client (true = Fixed): picked here, else remembered, else
/// what it is now, else Fixed.
fn mode_of(app: &AppHandle, st: &Placing, name: &str) -> bool {
    st.modes
        .get(name)
        .copied()
        .or_else(|| crate::clientwindows::fixed_of(app, name))
        .or_else(|| handle_of(name).map(|h| !crate::clientwindows::captioned_window(h)))
        .unwrap_or(true)
}

/// The settings profile a client starts with (its account's own, else the one it last started
/// with, else its number from Your accounts) and how many other EVE accounts use that folder.
fn profile_of_client(app: &AppHandle, name: &str) -> Option<(String, usize)> {
    let seat = crate::launch::load_seat(app);
    let mut accounts: Vec<&crate::launch::SeatAccount> = seat.accounts.iter().collect();
    accounts.sort_by_key(|a| crate::launch::nickname_key(&a.nickname));
    let acct = match name.strip_prefix("Login ").and_then(|n| n.parse::<usize>().ok()) {
        Some(n) => accounts.get(n.checked_sub(1)?).copied(),
        None => accounts.iter().find(|a| a.characters.iter().any(|c| c.name == name)).copied(),
    }?;
    let infos = crate::pcaccounts::infos(app);
    let info = acct.launcher_user_id.and_then(|u| infos.get(&u));
    let profile = info
        .and_then(crate::pcaccounts::own_profile)
        .or_else(|| info.and_then(|i| i.profile.clone()))
        .or_else(|| acct.profile.clone().filter(|p| !p.trim().is_empty()))?;
    let root = crate::profiles::root().ok()?;
    if !root.join(format!("settings_{profile}")).is_dir() {
        return None;
    }
    let shared = crate::pcaccounts::folders().into_iter().find(|f| f.profile == profile).map(|f| f.users.len().saturating_sub(1)).unwrap_or(0);
    Some((profile, shared))
}

/// Write a client's window into its profile's core_public__.yaml: Fixed Window and its size,
/// or Window mode with its spot. A snapshot of the profile is taken first.
fn write_window(profile: &str, fixed: bool, spot: Spot) -> Result<String, String> {
    let root = crate::profiles::root()?;
    let path = root.join(format!("settings_{profile}")).join("core_public__.yaml");
    let text = std::fs::read_to_string(&path).map_err(|_| format!("settings_{profile} has no core_public__.yaml yet: start a client with it once."))?;
    let snap = crate::snapshots::auto_snapshot(profile, "before Place clients set its window")?;
    let stamp = crate::tuning::file_time_now();
    let text = crate::windowfix::set_window_mode(&text, if fixed { 2 } else { 1 }, stamp);
    let text = if fixed {
        crate::windowfix::set_block_values(&text, "FixedWindowSettings", &[("width", spot.w as i64), ("height", spot.h as i64)], stamp)
    } else {
        // EVE's WindowedSettings: width / height are the inside, left / top the outside's corner.
        let (w, h) = eve_size(spot, false);
        crate::windowfix::set_block_values(&text, "WindowedSettings", &[("width", w as i64), ("height", h as i64), ("left", spot.x as i64), ("top", spot.y as i64), ("showState", 0)], stamp)
    };
    std::fs::write(&path, text).map_err(|e| format!("couldn't write settings_{profile}: {e}"))?;
    let (w, h) = eve_size(spot, fixed);
    log::info!("Place clients: settings_{profile} set to {} {w}x{h} (snapshot {snap})", if fixed { "Fixed Window" } else { "Window mode" });
    Ok(snap)
}

/// The one being placed: its name, whether it's a preview, and its spot.
fn current(app: &AppHandle) -> Result<(String, bool, Spot), String> {
    let (previews, index) = {
        let guard = PLACING.lock().unwrap();
        let st = guard.as_ref().ok_or("Place clients isn't open.")?;
        (st.previews, st.index)
    };
    let list = names(app, previews);
    let name = list.get(index).cloned().ok_or("Nothing to place.")?;
    let here = rect_of(app, previews, &name).ok_or("Can't find that window yet (a moment after opening, try again).")?;
    Ok((name, previews, here))
}

fn below_min(s: Spot, fixed: bool) -> String {
    let (w, h) = eve_size(s, fixed);
    if w < EVE_MIN.0 || h < EVE_MIN.1 {
        format!(" Smaller than EVE's recommended {} x {}.", EVE_MIN.0, EVE_MIN.1)
    } else {
        String::new()
    }
}

/// Resize the one being placed: to `w` x `h`, or by `dw` / `dh`, keeping its top-left.
#[tauri::command]
pub fn place_size(app: AppHandle, w: Option<i32>, h: Option<i32>, dw: Option<i32>, dh: Option<i32>) -> Result<String, String> {
    let (name, previews, here) = current(&app)?;
    let fixed = PLACING.lock().unwrap().as_ref().map(|st| mode_of(&app, st, &name)).unwrap_or(true);
    // A size picked (EVE's list) is EVE's Window Size: in Window mode its title bar and borders
    // go round it.
    let (fl, ft, fr, fb) = if previews || fixed { (0, 0, 0, 0) } else { eve_frame() };
    let to = Spot {
        w: w.map(|w| w + fl + fr).unwrap_or(here.w + dw.unwrap_or(0)).max(80),
        h: h.map(|h| h + ft + fb).unwrap_or(here.h + dh.unwrap_or(0)).max(45),
        ..here
    };
    if !move_to(&app, previews, &name, to) {
        return Err(format!("{name} is open in Fixed Window mode: its size comes from its profile. Close it and size its stand-in instead."));
    }
    if previews {
        return Ok(format!("{} x {}.", to.w, to.h));
    }
    let (ew, eh) = eve_size(to, fixed);
    Ok(format!("EVE window size {ew} x {eh}{}.{}", if fixed { String::new() } else { format!(" ({} x {} with its title bar and borders)", to.w, to.h) }, below_min(to, fixed)))
}

/// Window mode (false) or Fixed Window (true) for the one being placed (saved with its spot).
#[tauri::command]
pub fn place_mode(app: AppHandle, fixed: bool) -> Result<String, String> {
    let (name, _, _) = current(&app)?;
    if let Some(st) = PLACING.lock().unwrap().as_mut() {
        st.modes.insert(name.clone(), fixed);
    }
    if standin_for().as_deref() == Some(name.as_str()) {
        *STANDIN_FIXED.lock().unwrap() = Some(fixed);
    }
    if let Some(w) = app.get_webview_window(STANDIN) {
        let _ = w.eval(format!("window.__standinMode && window.__standinMode({fixed}, {});", serde_json::json!(eve_frame())));
    }
    Ok(format!("{name}: {} (saved with its spot).", if fixed { "Fixed Window, no title bar" } else { "Window mode, with a title bar" }))
}

/// The window zones (columns x rows of each monitor).
#[tauri::command]
pub fn place_zones(app: AppHandle, cols: u32, rows: u32) {
    if let Some(st) = PLACING.lock().unwrap().as_mut() {
        st.zones = (cols.clamp(1, 6), rows.clamp(1, 6));
    }
    let _ = place_state(app);
}

/// Fill one zone of a monitor with the one being placed (a Fixed Window client keeps its size
/// and goes to the zone's top-left).
#[tauri::command]
pub fn place_zone(app: AppHandle, monitor: usize, col: u32, row: u32) -> Result<String, String> {
    let (name, previews, here) = current(&app)?;
    let zones = PLACING.lock().unwrap().as_ref().map(|st| st.zones).unwrap_or((2, 2));
    let mon = *monitors(&app).get(monitor).ok_or("That monitor isn't there.")?;
    let (cw, ch) = (mon.w as f64 / zones.0 as f64, mon.h as f64 / zones.1 as f64);
    let x0 = mon.x + (col as f64 * cw).round() as i32;
    let y0 = mon.y + (row as f64 * ch).round() as i32;
    let x1 = mon.x + ((col + 1) as f64 * cw).round() as i32;
    let y1 = mon.y + ((row + 1) as f64 * ch).round() as i32;
    let mut to = Spot { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    if !previews && handle_of(&name).is_some_and(|h| !crate::clientwindows::captioned_window(h)) {
        to.w = here.w;
        to.h = here.h;
    }
    move_to(&app, previews, &name, to);
    Ok(format!("{name}: {} x {} at {}, {}.{}", to.w, to.h, to.x, to.y, if previews { String::new() } else { below_min(to, PLACING.lock().unwrap().as_ref().map(|st| mode_of(&app, st, &name)).unwrap_or(true)) }))
}

/// Put the one being placed flush in a corner, against a side, or in the middle of its monitor
/// ("nw", "n", "ne", "w", "c", "e", "sw", "s", "se"), keeping its size.
#[tauri::command]
pub fn place_align(app: AppHandle, to: String) -> Result<String, String> {
    let (name, previews, here) = current(&app)?;
    let mons = monitors(&app);
    let mon = monitor_of(&mons, here).and_then(|i| mons.get(i).copied()).ok_or("No monitor found.")?;
    let x = match to.as_str() {
        "nw" | "w" | "sw" => mon.x,
        "ne" | "e" | "se" => mon.x + mon.w - here.w,
        _ => mon.x + (mon.w - here.w) / 2,
    };
    let y = match to.as_str() {
        "nw" | "n" | "ne" => mon.y,
        "sw" | "s" | "se" => mon.y + mon.h - here.h,
        _ => mon.y + (mon.h - here.h) / 2,
    };
    move_to(&app, previews, &name, Spot { x, y, ..here });
    Ok(String::new())
}

/// Edges within this many pixels of a monitor edge, its middle or a zone line stick to it.
const MAGNET: i32 = 40;

/// Pull a frame's edges onto the nearest monitor edge, middle line or zone line close by.
fn magnet(app: &AppHandle, r: Spot, zones: (u32, u32)) -> Spot {
    let mons = monitors(app);
    let Some(mon) = monitor_of(&mons, r).and_then(|i| mons.get(i).copied()) else { return r };
    let mut xs = vec![mon.x, mon.x + mon.w, mon.x + mon.w / 2];
    let mut ys = vec![mon.y, mon.y + mon.h, mon.y + mon.h / 2];
    for c in 1..zones.0 {
        xs.push(mon.x + (mon.w as f64 * c as f64 / zones.0 as f64).round() as i32);
    }
    for c in 1..zones.1 {
        ys.push(mon.y + (mon.h as f64 * c as f64 / zones.1 as f64).round() as i32);
    }
    let best = |pos: i32, size: i32, lines: &[i32]| -> i32 {
        let mut pick = (MAGNET + 1, pos);
        for &l in lines {
            for cand in [l, l - size] {
                let d = (cand - pos).abs();
                if d < pick.0 {
                    pick = (d, cand);
                }
            }
        }
        pick.1
    };
    Spot { x: best(r.x, r.w, &xs), y: best(r.y, r.h, &ys), ..r }
}

static SI_LAST: Mutex<Option<Instant>> = Mutex::new(None);
static SI_IGNORE: Mutex<Option<Instant>> = Mutex::new(None);

/// The stand-in was moved or resized by hand: once let go, its edges stick to close lines.
fn standin_changed() {
    if SI_IGNORE.lock().unwrap().is_some_and(|until| Instant::now() < until) {
        return;
    }
    let fresh = SI_LAST.lock().unwrap().replace(Instant::now()).is_none();
    if fresh {
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(120));
            let last = *SI_LAST.lock().unwrap();
            let Some(last) = last else { return };
            if last.elapsed() >= Duration::from_millis(150) && !crate::previewtools::left_button_down() {
                *SI_LAST.lock().unwrap() = None;
                crate::mainthread::run(|app| {
                    let zones = PLACING.lock().unwrap().as_ref().map(|st| st.zones).unwrap_or((2, 2));
                    let (Some(name), Some(r)) = (standin_for(), standin_rect(app)) else { return };
                    let to = magnet(app, r, zones);
                    if to != r {
                        let fixed = STANDIN_FIXED.lock().unwrap().unwrap_or(true);
                        set_standin(app, &name, to, fixed);
                    }
                });
                return;
            }
        });
    }
}

/// Go to client number `index` (wraps round).
#[tauri::command]
pub fn place_go(app: AppHandle, index: i64) {
    let mut guard = PLACING.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    let n = names(&app, st.previews).len().max(1) as i64;
    st.index = index.rem_euclid(n) as usize;
    apply(&app, st);
}

/// How the others look: "outline", "fade", "hide" or "normal".
#[tauri::command]
pub fn place_others(app: AppHandle, mode: String) {
    let mut guard = PLACING.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    st.others = match mode.as_str() {
        "fade" | "hide" | "normal" => mode,
        _ => "outline".into(),
    };
    apply(&app, st);
}

/// Step through client windows or previews.
#[tauri::command]
pub fn place_kind(app: AppHandle, previews: bool) {
    let mut guard = PLACING.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    put_back(&app, st, None);
    st.previews = previews;
    if previews && !crate::previews::edit_mode(&app) {
        let _ = crate::previews::set_preview_edit(app.clone(), true);
        st.edit_on = true;
    }
    apply(&app, st);
}

/// One action on the one being placed: "same-prev", "snap", "stack", "save", or a nudge
/// ("left" / "right" / "up" / "down" by `step` px).
#[tauri::command]
pub fn place_action(app: AppHandle, action: String, step: Option<i32>) -> Result<String, String> {
    let (previews, index) = {
        let guard = PLACING.lock().unwrap();
        let st = guard.as_ref().ok_or("Place clients isn't open.")?;
        (st.previews, st.index)
    };
    let list = names(&app, previews);
    let current = list.get(index).cloned().ok_or("Nothing to place.")?;
    let here = rect_of(&app, previews, &current).ok_or("Can't find that window.")?;
    let said = match action.as_str() {
        "same-prev" => {
            let prev = list.get((index + list.len() - 1) % list.len()).cloned().ok_or("No previous one.")?;
            let to = rect_of(&app, previews, &prev).ok_or("Can't find the previous one.")?;
            if !move_to(&app, previews, &current, to) {
                return Err(format!("Couldn't move {current} (a Fixed Window client keeps its size: it needs the same size as {prev})."));
            }
            format!("{current} is on {prev}'s spot.")
        }
        "snap" => {
            let (_, cols, rows) = crate::previews::with_layout(&app, |l| (l.snap_on, l.snap_cols, l.snap_rows));
            let mons = monitors(&app);
            let mon = monitor_of(&mons, here).and_then(|i| mons.get(i).copied()).ok_or("No monitor found.")?;
            let mut to = cell_target(mon, cols, rows, here);
            if !previews {
                // Clients fill their cells (no gap), and a Fixed Window client keeps its size.
                to = Spot { x: to.x - 2, y: to.y - 2, w: to.w + 4, h: to.h + 4 };
                if !crate::clientwindows::captioned_window(handle_of(&current).unwrap_or(0)) {
                    to.w = here.w;
                    to.h = here.h;
                }
            }
            move_to(&app, previews, &current, to);
            format!("Snapped to the {cols} x {rows} grid.")
        }
        "stack" => {
            let mine: Vec<String> = stack_ids(&app, &current);
            if mine.is_empty() {
                return Err(format!("{current} isn't in a stack (a launch group with a pedal key)."));
            }
            let mut moved = 0;
            for n in list.iter().filter(|n| **n != current && stack_ids(&app, n).iter().any(|s| mine.contains(s))) {
                if move_to(&app, previews, n, here) {
                    moved += 1;
                }
            }
            format!("{moved} other{} in its stack moved onto this spot.", if moved == 1 { "" } else { "s" })
        }
        "save-one" => {
            if previews {
                return Ok(format!("{current}'s preview spot is saved ({} x {}).", here.w, here.h));
            }
            // A client writes its settings when it closes, and the launcher reads profiles when
            // it starts: saving a client's window needs every EVE client and the launcher closed.
            crate::pcaccounts::require_eve_closed()?;
            let fixed = PLACING.lock().unwrap().as_ref().map(|st| mode_of(&app, st, &current)).unwrap_or(true);
            crate::clientwindows::remember_one(&app, &current, here);
            crate::clientwindows::note_mode(&app, &current, fixed);
            let mode = if fixed { "Fixed Window" } else { "Window mode" };
            let yaml = match profile_of_client(&app, &current) {
                Some((profile, shared)) => match write_window(&profile, fixed, here) {
                    Ok(_) => format!(
                        " settings_{profile} now opens in {mode} at {} x {}{}.",
                        eve_size(here, fixed).0,
                        eve_size(here, fixed).1,
                        if shared > 0 { format!(" (shared with {shared} other EVE account{}: they get it too)", if shared == 1 { "" } else { "s" }) } else { String::new() }
                    ),
                    Err(e) => format!(" Its profile wasn't changed: {e}"),
                },
                None => " Its settings profile isn't known on this PC yet, so only the app's spot is saved.".into(),
            };
            let (ew, eh) = eve_size(here, fixed);
            format!("Saved {current}: {}, {}, EVE window size {ew} x {eh}.{yaml}{}", here.x, here.y, below_min(here, fixed))
        }
        "reset" => {
            if previews {
                crate::previews::with_layout(&app, |l| {
                    if let Some(p) = l.clients.get_mut(&current) {
                        p.x = None;
                        p.y = None;
                        p.w = None;
                        p.h = None;
                    }
                });
                let m = monitors(&app).first().copied().unwrap_or(Spot { x: 0, y: 0, w: 1920, h: 1080 });
                set_preview_rect(&app, &current, Spot { x: m.x + m.w / 2 - 160, y: m.y + m.h / 2 - 100, w: 320, h: 200 });
                format!("{current}'s preview is back to the middle at 320 x 200.")
            } else {
                crate::clientwindows::forget_one(&app, &current);
                if standin_for().as_deref() == Some(current.as_str()) {
                    let m = monitors(&app).first().copied().unwrap_or(Spot { x: 0, y: 0, w: 1920, h: 1080 });
                    let fixed = STANDIN_FIXED.lock().unwrap().unwrap_or(true);
                    set_standin(&app, &current, Spot { x: m.x + (m.w - 1280) / 2, y: m.y + (m.h - 720) / 2, w: 1280, h: 720 }, fixed);
                }
                format!("{current}'s saved spot is forgotten (its profile's window settings stay as they are).")
            }
        }
        "save" => {
            if previews {
                return Ok("Preview spots save by themselves.".into());
            }
            crate::pcaccounts::require_eve_closed()?;
            let mut saved = 0;
            for n in &list {
                if let Some(r) = rect_of(&app, false, n) {
                    crate::clientwindows::remember_one(&app, n, r);
                    saved += 1;
                }
            }
            format!("Saved {saved} client spot{}: each goes back there when it starts.", if saved == 1 { "" } else { "s" })
        }
        "left" | "right" | "up" | "down" => {
            let d = step.unwrap_or(1).clamp(1, 200);
            let (dx, dy) = match action.as_str() {
                "left" => (-d, 0),
                "right" => (d, 0),
                "up" => (0, -d),
                _ => (0, d),
            };
            move_to(&app, previews, &current, Spot { x: here.x + dx, y: here.y + dy, ..here });
            String::new()
        }
        _ => return Err("Unknown action.".into()),
    };
    if !said.is_empty() {
        log::info!("Place clients: {said}");
    }
    Ok(said)
}

fn stack_ids(app: &AppHandle, name: &str) -> Vec<String> {
    if crate::clipboard::is_login_label(name) {
        return Vec::new();
    }
    crate::launch::stacks_for(app, name).iter().filter(|s| s["state"] != "out").filter_map(|s| s["id"].as_str().map(str::to_string).or_else(|| s["id"].as_u64().map(|n| n.to_string()))).collect()
}

/// Close it (everything is put back).
#[tauri::command]
pub fn place_done(app: AppHandle) {
    done(&app);
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.close();
    }
}
