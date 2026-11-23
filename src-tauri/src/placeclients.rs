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
}

static PLACING: Mutex<Option<Placing>> = Mutex::new(None);

// ---- What's being placed ------------------------------------------------------------------------

/// The clients (or previews) to step through, in a steady order: characters by name, then
/// login screens by number.
fn names(app: &AppHandle, previews: bool) -> Vec<String> {
    let mut list: Vec<String> = if previews {
        app.webview_windows().into_iter().filter(|(l, _)| l.starts_with(crate::previews::PREFIX)).filter_map(|(_, w)| crate::previews::name_of_window(&w)).collect()
    } else {
        crate::clipboard::open_clients().into_iter().map(|(n, _)| n).collect()
    };
    list.sort_by_key(|n| {
        let login = crate::clipboard::is_login_label(n);
        let num = n.strip_prefix("Login ").and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
        (login, num, n.to_lowercase())
    });
    list.dedup();
    list
}

fn handle_of(name: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h)
}

fn rect_of(app: &AppHandle, previews: bool, name: &str) -> Option<Spot> {
    if previews {
        app.get_webview_window(&crate::previews::label_for(name)).and_then(|w| preview_rect(&w))
    } else {
        handle_of(name).and_then(crate::clientwindows::window_spot)
    }
}

fn move_to(app: &AppHandle, previews: bool, name: &str, to: Spot) -> bool {
    if previews {
        set_preview_rect(app, name, to);
        true
    } else {
        handle_of(name).is_some_and(|h| crate::clientwindows::place_window(h, to))
    }
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
    let outlines: Vec<(Spot, String, bool)> = list.iter().filter_map(|n| rect_of(app, previews, n).map(|r| (r, n.clone(), n == current))).collect();
    set_outlines(app, outlines);
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
            .inner_size(460.0, 380.0)
            .min_inner_size(380.0, 300.0)
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
    if let Some(mut st) = PLACING.lock().unwrap().take() {
        put_back(app, &mut st, None);
    }
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
    let remembered: Vec<bool> = list.iter().map(|n| crate::clientwindows::remembered(&app, n).is_some()).collect();
    let (snap_on, cols, rows) = crate::previews::with_layout(&app, |l| (l.snap_on, l.snap_cols, l.snap_rows));
    serde_json::json!({
        "open": true,
        "previews": st.previews,
        "names": list,
        "remembered": remembered,
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
    st.index = 0;
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
