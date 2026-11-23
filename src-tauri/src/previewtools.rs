//! 23 Nov: arranging previews made easier.
//!
//! - **Right-click menu** on a preview (its own small window, so a 150 x 100 preview isn't
//!   covered by its settings): shown, click-through, opacity, size, snap, line up, move to a
//!   monitor, undo.
//! - **Snap mode** (while arranging): every monitor's work area becomes a grid of cells, shown
//!   on a see-through layer. A preview dragged or resized fills the cells it's over when let go.
//! - The same see-through layer draws other clients' outlines for Place clients
//!   (placeclients.rs).
//!
//! Only our own windows are touched here.

use crate::clientwindows::Spot;
use crate::previews::{edit_mode, label_for, name_of_window, prefs_for, save_soon, with_layout, PREFIX};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const GRID: &str = "snapgrid-";
const MENU: &str = "pmenu";
/// Gap left round a snapped preview inside its cells (physical px).
const GAP: i32 = 2;

// ---- Monitors and cells ---------------------------------------------------------------------

/// Every monitor's work area (screen minus taskbar), physical pixels, in Windows' order.
pub fn monitors(app: &AppHandle) -> Vec<Spot> {
    let mut out = Vec::new();
    for m in app.available_monitors().unwrap_or_default() {
        let full = Spot { x: m.position().x, y: m.position().y, w: m.size().width as i32, h: m.size().height as i32 };
        out.push(work_area(full).unwrap_or(full));
    }
    out
}

#[cfg(windows)]
fn work_area(of: Spot) -> Option<Spot> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    let r = RECT { left: of.x + of.w / 2, top: of.y + of.h / 2, right: of.x + of.w / 2 + 1, bottom: of.y + of.h / 2 + 1 };
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

#[cfg(not(windows))]
fn work_area(of: Spot) -> Option<Spot> {
    Some(of)
}

/// The monitor a rectangle's middle is on (else the nearest one).
pub fn monitor_of(mons: &[Spot], r: Spot) -> Option<usize> {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    mons.iter().position(|m| cx >= m.x && cx < m.x + m.w && cy >= m.y && cy < m.y + m.h).or_else(|| {
        mons.iter().enumerate().min_by_key(|(_, m)| {
            let dx = (cx - (m.x + m.w / 2)) as i64;
            let dy = (cy - (m.y + m.h / 2)) as i64;
            dx * dx + dy * dy
        }).map(|(i, _)| i)
    })
}

/// The cells a rectangle snaps to on a monitor of `cols` x `rows` cells: as many cells as it
/// covers (at least one), starting at the nearest grid line, kept on the monitor.
pub fn cell_target(mon: Spot, cols: u32, rows: u32, r: Spot) -> Spot {
    let cols = cols.clamp(1, 48) as i32;
    let rows = rows.clamp(1, 48) as i32;
    let cw = mon.w as f64 / cols as f64;
    let ch = mon.h as f64 / rows as f64;
    let span_c = ((r.w as f64 / cw).round() as i32).clamp(1, cols);
    let span_r = ((r.h as f64 / ch).round() as i32).clamp(1, rows);
    let col = (((r.x - mon.x) as f64 / cw).round() as i32).clamp(0, cols - span_c);
    let row = (((r.y - mon.y) as f64 / ch).round() as i32).clamp(0, rows - span_r);
    let x0 = mon.x + (col as f64 * cw).round() as i32;
    let y0 = mon.y + (row as f64 * ch).round() as i32;
    let x1 = mon.x + ((col + span_c) as f64 * cw).round() as i32;
    let y1 = mon.y + ((row + span_r) as f64 * ch).round() as i32;
    Spot { x: x0 + GAP, y: y0 + GAP, w: x1 - x0 - 2 * GAP, h: y1 - y0 - 2 * GAP }
}

fn grid_size(app: &AppHandle) -> (bool, u32, u32) {
    with_layout(app, |l| (l.snap_on, l.snap_cols, l.snap_rows))
}

// ---- The see-through layer (one per monitor) --------------------------------------------------

/// What the layer shows besides the grid: other clients' outlines while placing clients.
static OUTLINES: Mutex<Vec<(Spot, String, bool)>> = Mutex::new(Vec::new());
/// Whether the grid lines are drawn (snap mode) and whether the layer is wanted at all.
static LAYER: Mutex<(bool, bool)> = Mutex::new((false, false));

/// Show or hide the grid (snap mode while arranging).
pub fn show_grid(app: &AppHandle, on: bool) {
    let placing = !OUTLINES.lock().unwrap().is_empty();
    *LAYER.lock().unwrap() = (on, on || placing);
    sync_layer(app);
}

/// Place clients: the other clients' outlines (empty clears them).
pub fn set_outlines(app: &AppHandle, outlines: Vec<(Spot, String, bool)>) {
    let any = !outlines.is_empty();
    *OUTLINES.lock().unwrap() = outlines;
    let grid = LAYER.lock().unwrap().0;
    *LAYER.lock().unwrap() = (grid, grid || any);
    sync_layer(app);
}

fn layer_json(app: &AppHandle, mon: Spot) -> serde_json::Value {
    let (grid, _) = *LAYER.lock().unwrap();
    let (_, cols, rows) = grid_size(app);
    let rel = |s: Spot| serde_json::json!({ "x": (s.x - mon.x) as f64 / mon.w as f64, "y": (s.y - mon.y) as f64 / mon.h as f64, "w": s.w as f64 / mon.w as f64, "h": s.h as f64 / mon.h as f64 });
    let outlines: Vec<serde_json::Value> = OUTLINES
        .lock()
        .unwrap()
        .iter()
        .filter(|(s, _, _)| s.x < mon.x + mon.w && s.x + s.w > mon.x && s.y < mon.y + mon.h && s.y + s.h > mon.y)
        .map(|(s, name, current)| {
            let mut v = rel(*s);
            v["name"] = serde_json::json!(name);
            v["current"] = serde_json::json!(current);
            v
        })
        .collect();
    serde_json::json!({ "grid": grid, "cols": cols, "rows": rows, "outlines": outlines })
}

fn sync_layer(app: &AppHandle) {
    let (_, wanted) = *LAYER.lock().unwrap();
    let mons = if wanted { monitors(app) } else { Vec::new() };
    // Close layers for monitors that are gone (or all of them).
    let extra: Vec<WebviewWindow> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.strip_prefix(GRID).and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| n >= mons.len()))
        .map(|(_, w)| w)
        .collect();
    if !extra.is_empty() {
        let _ = app.run_on_main_thread(move || {
            for w in extra {
                let _ = w.destroy();
            }
        });
    }
    for (i, mon) in mons.into_iter().enumerate() {
        let label = format!("{GRID}{i}");
        let state = layer_json(app, mon);
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.eval(format!("window.__layer && window.__layer({state});"));
            continue;
        }
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let built = WebviewWindowBuilder::new(&app2, &label, WebviewUrl::App("snapgrid.html".into()))
                .initialization_script(format!("window.__LAYER__ = {state};"))
                .title("Snap grid")
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(false)
                .resizable(false)
                .focused(false)
                .visible(false)
                .build();
            match built {
                Ok(w) => {
                    crate::clientbar::tool_window(&w);
                    let _ = w.set_position(PhysicalPosition::new(mon.x, mon.y));
                    let _ = w.set_size(PhysicalSize::new(mon.w as u32, mon.h as u32));
                    let _ = w.set_ignore_cursor_events(true);
                    let _ = w.show();
                    // Previews stay above the layer.
                    for (l, p) in app2.webview_windows() {
                        if l.starts_with(PREFIX) {
                            let _ = p.set_always_on_top(true);
                        }
                    }
                }
                Err(e) => log::warn!("couldn't open the snap grid: {e}"),
            }
        });
    }
}

/// Light up the cells a preview will land in (None clears it).
fn highlight(app: &AppHandle, target: Option<Spot>) {
    for (label, w) in app.webview_windows() {
        let Some(i) = label.strip_prefix(GRID).and_then(|n| n.parse::<usize>().ok()) else { continue };
        let mons = monitors(app);
        let Some(mon) = mons.get(i).copied() else { continue };
        let arg = match target {
            Some(t) if t.x < mon.x + mon.w && t.x + t.w > mon.x && t.y < mon.y + mon.h && t.y + t.h > mon.y => serde_json::json!({
                "x": (t.x - mon.x) as f64 / mon.w as f64, "y": (t.y - mon.y) as f64 / mon.h as f64,
                "w": t.w as f64 / mon.w as f64, "h": t.h as f64 / mon.h as f64,
            }),
            _ => serde_json::Value::Null,
        };
        let _ = w.eval(format!("window.__hl && window.__hl({arg});"));
    }
}

// ---- Moving our previews -----------------------------------------------------------------------

/// Previews we're moving ourselves: their own move events are ignored until then.
static IGNORE: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// A preview being dragged in snap mode: when it last moved, and where it started.
static DRAG: Mutex<Option<HashMap<String, (Instant, Spot)>>> = Mutex::new(None);
/// The last move we made (or a drag that snapped), to put back with Undo.
static UNDO: Mutex<Vec<(String, Spot)>> = Mutex::new(Vec::new());

pub fn preview_rect(window: &WebviewWindow) -> Option<Spot> {
    let p = window.outer_position().ok()?;
    let s = window.outer_size().ok()?;
    Some(Spot { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 })
}

fn preview_window(app: &AppHandle, name: &str) -> Option<WebviewWindow> {
    app.get_webview_window(&label_for(name))
}

/// Every open preview with its name and spot.
fn all_previews(app: &AppHandle) -> Vec<(String, WebviewWindow, Spot)> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(PREFIX))
        .filter_map(|(_, w)| {
            let name = name_of_window(&w)?;
            let r = preview_rect(&w)?;
            Some((name, w, r))
        })
        .collect()
}

/// Move and size one preview, and save it there.
pub fn set_preview_rect(app: &AppHandle, name: &str, r: Spot) {
    let Some(window) = preview_window(app, name) else { return };
    IGNORE.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.to_string(), Instant::now() + Duration::from_millis(500));
    let w = r.w.max(80) as u32;
    let h = r.h.max(45) as u32;
    let _ = window.set_position(PhysicalPosition::new(r.x, r.y));
    let _ = window.set_size(PhysicalSize::new(w, h));
    with_layout(app, |layout| {
        let prefs = layout.clients.entry(name.to_string()).or_default();
        prefs.x = Some(r.x);
        prefs.y = Some(r.y);
        prefs.w = Some(w);
        prefs.h = Some(h);
    });
    save_soon(app);
}

#[cfg(windows)]
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    // SAFETY: a plain key-state read.
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
}

#[cfg(not(windows))]
fn left_button_down() -> bool {
    false
}

/// A preview moved or changed size (edit mode): in snap mode, light up where it'll land and snap
/// it there once the mouse button is let go.
pub fn on_preview_changed(app: &AppHandle, name: &str) {
    if !edit_mode(app) {
        return;
    }
    if IGNORE.lock().unwrap().as_ref().and_then(|m| m.get(name)).is_some_and(|until| Instant::now() < *until) {
        return;
    }
    let (on, cols, rows) = grid_size(app);
    if !on {
        return;
    }
    let Some(window) = preview_window(app, name) else { return };
    let Some(r) = preview_rect(&window) else { return };
    let mons = monitors(app);
    let Some(i) = monitor_of(&mons, r) else { return };
    highlight(app, Some(cell_target(mons[i], cols, rows, r)));
    let fresh = {
        let mut drag = DRAG.lock().unwrap();
        let map = drag.get_or_insert_with(HashMap::new);
        let fresh = !map.contains_key(name);
        let start = map.get(name).map(|(_, s)| *s).unwrap_or(r);
        map.insert(name.to_string(), (Instant::now(), start));
        fresh
    };
    if fresh {
        let name = name.to_string();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(120));
            let last = DRAG.lock().unwrap().as_ref().and_then(|m| m.get(&name).map(|(t, _)| *t));
            let Some(last) = last else { return };
            if last.elapsed() >= Duration::from_millis(150) && !left_button_down() {
                let name = name.clone();
                crate::mainthread::run(move |app| finish_drag(app, &name));
                return;
            }
        });
    }
}

fn finish_drag(app: &AppHandle, name: &str) {
    let start = DRAG.lock().unwrap().as_mut().and_then(|m| m.remove(name)).map(|(_, s)| s);
    highlight(app, None);
    let (on, cols, rows) = grid_size(app);
    let Some(window) = preview_window(app, name) else { return };
    let Some(r) = preview_rect(&window) else { return };
    if !on {
        return;
    }
    let mons = monitors(app);
    let Some(i) = monitor_of(&mons, r) else { return };
    let to = cell_target(mons[i], cols, rows, r);
    if let Some(start) = start {
        *UNDO.lock().unwrap() = vec![(name.to_string(), start)];
    }
    set_preview_rect(app, name, to);
}

// ---- The right-click menu --------------------------------------------------------------------

static MENU_FOR: Mutex<String> = Mutex::new(String::new());

/// Open the menu for one preview at the mouse.
#[tauri::command]
pub fn preview_menu_open(app: AppHandle, name: String) {
    *MENU_FOR.lock().unwrap() = name.clone();
    let at = app.cursor_position().map(|p| (p.x as i32, p.y as i32)).unwrap_or((100, 100));
    if let Some(menu) = app.get_webview_window(MENU) {
        let _ = menu.eval(format!("window.__menuFor && window.__menuFor({});", serde_json::json!(name)));
        place_menu(&app, &menu, at, None);
        let _ = menu.show();
        let _ = menu.set_focus();
        return;
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let built = WebviewWindowBuilder::new(&app2, MENU, WebviewUrl::App("pmenu.html".into()))
            .title("Preview menu")
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(true)
            .inner_size(280.0, 420.0)
            .visible(false)
            .build();
        let menu = match built {
            Ok(m) => m,
            Err(e) => {
                log::warn!("couldn't open the preview menu: {e}");
                return;
            }
        };
        crate::clientbar::tool_window(&menu);
        let m2 = menu.clone();
        menu.on_window_event(move |event| {
            if let WindowEvent::Focused(false) = event {
                let _ = m2.hide();
            }
        });
        place_menu(&app2, &menu, at, None);
        let _ = menu.show();
        let _ = menu.set_focus();
    });
}

/// Put the menu at the mouse, kept on the monitor (flipped left / up near an edge).
fn place_menu(app: &AppHandle, menu: &WebviewWindow, at: (i32, i32), height: Option<f64>) {
    let scale = menu.scale_factor().unwrap_or(1.0);
    if let Some(h) = height {
        let _ = menu.set_size(LogicalSize::new(280.0, h.clamp(120.0, 900.0)));
    }
    let size = menu.outer_size().map(|s| (s.width as i32, s.height as i32)).unwrap_or(((280.0 * scale) as i32, (420.0 * scale) as i32));
    let mons = monitors(app);
    let mon = monitor_of(&mons, Spot { x: at.0, y: at.1, w: 1, h: 1 }).and_then(|i| mons.get(i).copied());
    let (mut x, mut y) = (at.0 + 2, at.1 + 2);
    if let Some(m) = mon {
        if x + size.0 > m.x + m.w {
            x = (at.0 - size.0 - 2).max(m.x);
        }
        if y + size.1 > m.y + m.h {
            y = (m.y + m.h - size.1).max(m.y);
        }
    }
    let _ = menu.set_position(PhysicalPosition::new(x, y));
}

/// The menu page fits its window to what it shows (CSS pixels).
#[tauri::command]
pub fn preview_menu_fit(app: AppHandle, height: f64) {
    let Some(menu) = app.get_webview_window(MENU) else { return };
    let Some(pos) = menu.outer_position().ok() else { return };
    place_menu(&app, &menu, (pos.x - 2, pos.y - 2), Some(height));
}

#[tauri::command]
pub fn preview_menu_close(app: AppHandle) {
    if let Some(menu) = app.get_webview_window(MENU) {
        let _ = menu.hide();
    }
}

/// What the menu shows for the preview it's open for.
#[tauri::command]
pub fn preview_menu_state(app: AppHandle) -> serde_json::Value {
    let name = MENU_FOR.lock().unwrap().clone();
    let prefs = prefs_for(&app, &name);
    let (snap_on, cols, rows) = grid_size(&app);
    let mons = monitors(&app);
    let here = preview_window(&app, &name).and_then(|w| preview_rect(&w)).and_then(|r| monitor_of(&mons, r));
    let in_stack = !crate::clipboard::is_login_label(&name) && crate::launch::stacks_for(&app, &name).iter().any(|s| s["state"] != "out");
    serde_json::json!({
        "name": name,
        "enabled": prefs.enabled,
        "clickThrough": prefs.click_through,
        "opacity": prefs.opacity,
        "edit": edit_mode(&app),
        "snapOn": snap_on,
        "cols": cols,
        "rows": rows,
        "monitors": mons.len(),
        "monitor": here,
        "canUndo": !UNDO.lock().unwrap().is_empty(),
        "size": preview_window(&app, &name).and_then(|w| preview_rect(&w)).map(|r| [r.w, r.h]),
        "inStack": in_stack,
    })
}

/// Snap mode on / off and its grid (cells across x down each monitor).
#[tauri::command]
pub fn set_previews_snap_mode(app: AppHandle, on: bool, cols: Option<u32>, rows: Option<u32>) {
    with_layout(&app, |l| {
        l.snap_on = on;
        if let Some(c) = cols {
            l.snap_cols = c.clamp(2, 32);
        }
        if let Some(r) = rows {
            l.snap_rows = r.clamp(2, 32);
        }
    });
    save_soon(&app);
    show_grid(&app, on && edit_mode(&app));
}

/// The stacks (groups with a pedal key) a client is in.
fn stack_ids(app: &AppHandle, name: &str) -> Vec<String> {
    crate::launch::stacks_for(app, name).iter().filter(|s| s["state"] != "out").filter_map(|s| s["id"].as_str().map(str::to_string).or_else(|| s["id"].as_u64().map(|n| n.to_string()))).collect()
}

/// One of the menu's arranging actions on preview `name`.
#[tauri::command]
pub fn preview_action(app: AppHandle, name: String, action: String, value: Option<i64>) -> Result<String, String> {
    let all = all_previews(&app);
    let me = all.iter().find(|(n, _, _)| *n == name).map(|(_, _, r)| *r).ok_or("That preview isn't open.")?;
    let mons = monitors(&app);
    let mon = monitor_of(&mons, me).and_then(|i| mons.get(i).copied()).ok_or("No monitor found.")?;
    let (_, cols, rows) = grid_size(&app);
    let mut moves: Vec<(String, Spot)> = Vec::new();
    let said = match action.as_str() {
        "size-s" | "size-m" | "size-l" => {
            let part = match action.as_str() {
                "size-s" => 8.0,
                "size-m" => 6.0,
                _ => 4.0,
            };
            let w = (mon.w as f64 / part) as i32;
            let h = (w as f64 * me.h as f64 / me.w.max(1) as f64) as i32;
            moves.push((name.clone(), Spot { w, h, ..me }));
            "Resized.".to_string()
        }
        "size-set" => {
            let v = value.unwrap_or(0);
            let (w, h) = ((v / 100_000) as i32, (v % 100_000) as i32);
            if w < 80 || h < 45 {
                return Err("At least 80 x 45.".into());
            }
            moves.push((name.clone(), Spot { w, h, ..me }));
            format!("{w} x {h}.")
        }
        "size-all" => {
            for (n, _, r) in &all {
                moves.push((n.clone(), Spot { w: me.w, h: me.h, ..*r }));
            }
            format!("Every preview is {} x {} now.", me.w, me.h)
        }
        "snap-now" => {
            moves.push((name.clone(), cell_target(mon, cols, rows, me)));
            format!("Snapped to the {cols} x {rows} grid.")
        }
        "row-monitor" | "col-monitor" | "row-stack" | "col-stack" => {
            let row = action.starts_with("row");
            let others: Vec<(String, Spot)> = if action.ends_with("stack") {
                let mine = stack_ids(&app, &name);
                if mine.is_empty() {
                    return Err("This client isn't in a stack.".into());
                }
                all.iter().filter(|(n, _, _)| *n != name && stack_ids(&app, n).iter().any(|s| mine.contains(s))).map(|(n, _, r)| (n.clone(), *r)).collect()
            } else {
                all.iter().filter(|(n, _, r)| *n != name && monitor_of(&mons, *r).and_then(|i| mons.get(i).copied()) == Some(mon)).map(|(n, _, r)| (n.clone(), *r)).collect()
            };
            let mut others = others;
            if row {
                others.sort_by_key(|(_, r)| (r.y, r.x));
            } else {
                others.sort_by_key(|(_, r)| (r.x, r.y));
            }
            let (sw, sh) = (me.w + 4, me.h + 4);
            let per_row = ((mon.x + mon.w - me.x) / sw).max(1);
            let per_col = ((mon.y + mon.h - me.y) / sh).max(1);
            for (k, (n, _)) in others.iter().enumerate() {
                let i = k as i32 + 1;
                let (c, r) = if row { (i % per_row, i / per_row) } else { (i / per_col, i % per_col) };
                moves.push((n.clone(), Spot { x: me.x + c * sw, y: me.y + r * sh, w: me.w, h: me.h }));
            }
            format!("Lined up {} preview{} {} from here.", others.len(), if others.len() == 1 { "" } else { "s" }, if row { "in rows" } else { "in columns" })
        }
        "monitor" => {
            let i = value.unwrap_or(0).max(0) as usize;
            let to = *mons.get(i).ok_or("That monitor isn't there.")?;
            let fx = (me.x - mon.x) as f64 / mon.w as f64;
            let fy = (me.y - mon.y) as f64 / mon.h as f64;
            let x = (to.x + (fx * to.w as f64) as i32).min(to.x + to.w - me.w).max(to.x);
            let y = (to.y + (fy * to.h as f64) as i32).min(to.y + to.h - me.h).max(to.y);
            moves.push((name.clone(), Spot { x, y, ..me }));
            format!("Moved to monitor {}.", i + 1)
        }
        "undo" => {
            let back = std::mem::take(&mut *UNDO.lock().unwrap());
            if back.is_empty() {
                return Err("Nothing to undo.".into());
            }
            for (n, r) in &back {
                set_preview_rect(&app, n, *r);
            }
            return Ok("Put back.".into());
        }
        _ => return Err("Unknown action.".into()),
    };
    let before: Vec<(String, Spot)> = moves.iter().filter_map(|(n, _)| all.iter().find(|(m, _, _)| m == n).map(|(m, _, r)| (m.clone(), *r))).collect();
    *UNDO.lock().unwrap() = before;
    for (n, r) in moves {
        set_preview_rect(&app, &n, r);
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_cells_it_covers() {
        let mon = Spot { x: 0, y: 0, w: 800, h: 800 };
        // 8 x 8 cells of 100: a 190 x 110 preview at 95,205 covers 2 x 1 cells from column 1, row 2.
        let t = cell_target(mon, 8, 8, Spot { x: 95, y: 205, w: 190, h: 110 });
        assert_eq!(t, Spot { x: 102, y: 202, w: 196, h: 96 });
        // Never off the monitor; at least one cell.
        let t = cell_target(mon, 8, 8, Spot { x: 790, y: 790, w: 10, h: 10 });
        assert_eq!(t, Spot { x: 702, y: 702, w: 96, h: 96 });
    }

    #[test]
    fn finds_the_monitor() {
        let mons = [Spot { x: 0, y: 0, w: 100, h: 100 }, Spot { x: 100, y: 0, w: 100, h: 100 }];
        assert_eq!(monitor_of(&mons, Spot { x: 120, y: 10, w: 10, h: 10 }), Some(1));
        assert_eq!(monitor_of(&mons, Spot { x: 500, y: 10, w: 10, h: 10 }), Some(1));
    }
}
