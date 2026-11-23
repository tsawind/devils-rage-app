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
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

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
            set_standin(app, name, to);
            true
        }
        None => false,
    }
}

// ---- The stand-in: a frame for a client that isn't open -----------------------------------------

const STANDIN: &str = "standin";
static STANDIN_FOR: Mutex<Option<String>> = Mutex::new(None);

fn standin_for() -> Option<String> {
    STANDIN_FOR.lock().unwrap().clone()
}

fn standin_rect(app: &AppHandle) -> Option<Spot> {
    app.get_webview_window(STANDIN).and_then(|w| preview_rect(&w))
}

/// Show the stand-in frame for `name` at `at` (built the first time).
fn set_standin(app: &AppHandle, name: &str, at: Spot) {
    *STANDIN_FOR.lock().unwrap() = Some(name.to_string());
    let arg = serde_json::json!(name);
    if let Some(w) = app.get_webview_window(STANDIN) {
        let _ = w.set_position(tauri::PhysicalPosition::new(at.x, at.y));
        let _ = w.set_size(tauri::PhysicalSize::new(at.w.max(200) as u32, at.h.max(120) as u32));
        let _ = w.eval(format!("window.__standin && window.__standin({arg});"));
        let _ = w.show();
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
                let _ = w.show();
            }
            Err(e) => log::warn!("couldn't open the stand-in frame: {e}"),
        }
    });
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
        set_standin(app, &current, at);
    }
    if st.previews && app.get_webview_window(&crate::previews::label_for(&current)).is_none() {
        // Its client isn't open: open its preview (empty) so it can be put where it goes.
        let (app2, name) = (app.clone(), current.clone());
        tauri::async_runtime::spawn(async move { crate::previews::build_preview(&app2, &name) });
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
    draw_outlines(app, st.previews, &list, &current);
}

fn draw_outlines(app: &AppHandle, previews: bool, list: &[String], current: &str) {
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
        *guard = Some(Placing { previews, others: "outline".into(), ..Placing::default() });
    }
    if previews && !crate::previews::edit_mode(&app) {
        let _ = crate::previews::set_preview_edit(app.clone(), true);
        if let Some(st) = PLACING.lock().unwrap().as_mut() {
            st.edit_on = true;
        }
    }
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.eval("window.__reload && window.__reload();");
        let _ = w.show();
        let _ = w.set_focus();
        step(&app);
        return;
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let built = WebviewWindowBuilder::new(&app2, WINDOW, WebviewUrl::App("place.html".into()))
            .title("Place clients")
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
        draw_outlines(&app, st.previews, &list, c);
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
        "open": open,
        "standin": current.as_deref().is_some_and(|c| standin_for().as_deref() == Some(c)),
        "index": index,
        "current": current,
        "rect": current.as_deref().and_then(|c| rect_of(&app, st.previews, c)),
        "others": st.others,
        "grid": [cols, rows],
        "snapOn": snap_on,
    })
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
            crate::clientwindows::remember_one(&app, &current, here);
            format!("Saved: {current} goes to {}, {} ({} x {}) when it starts.", here.x, here.y, here.w, here.h)
        }
        "save" => {
            if previews {
                return Ok("Preview spots save by themselves.".into());
            }
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
