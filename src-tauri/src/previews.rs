//! Patch 62t / 63c: our own EVE-O Preview. One small borderless, always-on-top window per EVE
//! client, each showing that client's live picture (from capture.rs).
//!
//! - **Edit mode** (one switch for all): drag previews where you want them, resize from the
//!   corner, turn each one on or off, set its click-through and opacity. Positions and sizes
//!   are saved per character.
//! - **Locked** (edit mode off): previews stay put. Clicking one brings that client forward,
//!   unless that client is set to **click-through**: then clicks go straight to whatever is
//!   underneath (the game) and you switch with the pedal / hotkeys only.
//! - **Import** an EVE-O Preview config: thumbnail positions / sizes and its cycle groups
//!   (which become launch groups with their pedal hotkeys).
//!
//! Nothing is sent anywhere and nothing touches the game.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

/// Label every preview window shares a prefix, so we can find and close them together.
const PREFIX: &str = "preview-";
const DEFAULT_SIZE: (f64, f64) = (320.0, 200.0);
const MIN_SIZE: (f64, f64) = (80.0, 45.0);

fn label_for(name: &str) -> String {
    // Keep it to a safe window label (letters and digits); the page gets the real name.
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{PREFIX}{slug}")
}

// ---- Saved layout --------------------------------------------------------------------------

fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientPrefs {
    /// Shown when previews are open (edit mode shows hidden ones too, dimmed).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Clicks pass through to the game (switch with the pedal only).
    #[serde(default)]
    pub click_through: bool,
    /// 0.2 – 1.0.
    #[serde(default = "one")]
    pub opacity: f64,
    /// Screen position and size in physical pixels (None = let Windows place it).
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub w: Option<u32>,
    #[serde(default)]
    pub h: Option<u32>,
}

impl Default for ClientPrefs {
    fn default() -> Self {
        ClientPrefs {
            enabled: true,
            click_through: false,
            opacity: 1.0,
            x: None,
            y: None,
            w: None,
            h: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub edit: bool,
    #[serde(default)]
    pub clients: BTreeMap<String, ClientPrefs>,
    /// Master opacity for every preview (multiplies each client's own), 0.1 – 1.0.
    #[serde(default = "one")]
    pub opacity: f64,
    /// Patch 70: open the previews by themselves when EVE clients start.
    #[serde(default = "yes")]
    pub auto_open: bool,
    /// Hide the preview of the client in front (you're looking at it already).
    #[serde(default)]
    pub hide_active: bool,
    /// Snap grid in pixels when edit mode ends (0 = off): spots and sizes round to it, and
    /// edges within a few pixels of another preview's line up with it.
    #[serde(default = "ten")]
    pub snap: u32,
    /// Previews for clients still at the login / character-select screen (off: a big batch of
    /// alts at character select would otherwise each get a live window).
    #[serde(default)]
    pub login_previews: bool,
}

/// At most this many previews open by themselves (more can still be opened by hand).
const AUTO_MAX: usize = 16;

fn ten() -> u32 {
    10
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            edit: false,
            clients: BTreeMap::new(),
            opacity: 1.0,
            auto_open: true,
            hide_active: false,
            snap: 10,
            login_previews: false,
        }
    }
}

static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);
static SAVE_PENDING: AtomicBool = AtomicBool::new(false);

fn layout_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("previews.json"))
}

fn with_layout<T>(app: &AppHandle, f: impl FnOnce(&mut Layout) -> T) -> T {
    let mut guard = LAYOUT.lock().unwrap();
    if guard.is_none() {
        let loaded = layout_path(app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        *guard = Some(loaded);
    }
    f(guard.as_mut().expect("layout loaded"))
}

/// Save soon (window moves fire many times while dragging; one write after they stop).
fn save_soon(app: &AppHandle) {
    if SAVE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        SAVE_PENDING.store(false, Ordering::SeqCst);
        let text = with_layout(&app, |layout| {
            serde_json::to_string_pretty(layout).unwrap_or_default()
        });
        if let Some(path) = layout_path(&app) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    });
}

fn prefs_for(app: &AppHandle, name: &str) -> ClientPrefs {
    with_layout(app, |layout| {
        layout.clients.get(name).cloned().unwrap_or_default()
    })
}

fn edit_mode(app: &AppHandle) -> bool {
    with_layout(app, |layout| layout.edit)
}

/// What a preview page needs to draw itself (sent on open and after every change).
fn mode_json(app: &AppHandle, name: &str) -> serde_json::Value {
    let prefs = prefs_for(app, name);
    let master = with_layout(app, |layout| layout.opacity);
    serde_json::json!({
        "edit": edit_mode(app),
        "enabled": prefs.enabled,
        "clickThrough": prefs.click_through,
        "opacity": prefs.opacity,
        "master": master,
    })
}

/// Push the mode to one preview window: click-through (locked mode only) and its page.
fn apply_mode(app: &AppHandle, window: &WebviewWindow, name: &str) {
    let prefs = prefs_for(app, name);
    let edit = edit_mode(app);
    let _ = window.set_ignore_cursor_events(!edit && prefs.click_through);
    let _ = window.eval(format!(
        "window.__previewMode && window.__previewMode({});",
        mode_json(app, name)
    ));
}

// ---- Commands -------------------------------------------------------------------------------

/// The EVE clients open right now (character name + whether it's minimized).
#[tauri::command]
pub fn preview_clients() -> Vec<serde_json::Value> {
    crate::clipboard::open_clients()
        .into_iter()
        .map(|(name, handle)| serde_json::json!({ "name": name, "minimized": crate::capture::minimized_handle(handle) }))
        .collect()
}

/// One client's picture as a data URL, for its preview window to draw. The capture and JPEG
/// encode run on a blocking thread so a preview never stalls the UI thread.
#[tauri::command]
pub async fn preview_frame(
    name: String,
    width: Option<u32>,
    quality: Option<u8>,
) -> Result<String, String> {
    let w = width.unwrap_or(480).clamp(120, 1280);
    let q = quality.unwrap_or(70).clamp(30, 90);
    tauri::async_runtime::spawn_blocking(move || {
        let jpeg = crate::capture::client_jpeg(&name, w, q)?;
        Ok(format!("data:image/jpeg;base64,{}", base64_encode(&jpeg)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Bring a client to the front (its preview was clicked).
#[tauri::command]
pub fn preview_focus(name: String) -> bool {
    crate::capture::focus_client(&name)
}

/// The EVE client in front (a character or "Login N"), so its preview can light up.
#[tauri::command]
pub fn active_client() -> Option<String> {
    crate::clipboard::foreground_client()
}

/// A preview page asking how to show itself (edit mode, on/off, click-through, opacity).
#[tauri::command]
pub fn preview_mode(app: AppHandle, name: String) -> serde_json::Value {
    mode_json(&app, &name)
}

/// The saved layout (for Home: edit mode on/off, which clients are hidden / click-through).
#[tauri::command]
pub fn preview_layout(app: AppHandle) -> Layout {
    with_layout(&app, |layout| layout.clone())
}

/// Open (or refocus) a preview for every open EVE client that's turned on (in edit mode:
/// every open client, so hidden ones can be turned back on). Idempotent.
///
/// The windows are built off the main thread (building them inside the command would block the
/// UI thread until each webview is ready, which froze the whole app), so this returns at once.
#[tauri::command]
pub fn open_previews(app: AppHandle) -> Result<usize, String> {
    AUTO_PAUSED.store(false, std::sync::atomic::Ordering::Relaxed);
    let clients = crate::clipboard::open_clients();
    if clients.is_empty() {
        return Err("No EVE clients are open.".to_string());
    }
    let edit = edit_mode(&app);
    let logins = with_layout(&app, |layout| layout.login_previews);
    let wanted: Vec<String> = clients
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| edit || prefs_for(&app, name).enabled)
        .filter(|name| logins || !crate::clipboard::is_login_label(name))
        .collect();
    let count = wanted.len();
    for name in &wanted {
        if let Some(existing) = app.get_webview_window(&label_for(name)) {
            let _ = existing.show();
            apply_mode(&app, &existing, name);
        }
    }
    tauri::async_runtime::spawn(async move {
        for name in wanted {
            if app.get_webview_window(&label_for(&name)).is_none() {
                build_preview(&app, &name);
            }
        }
        log::info!("previews opened");
    });
    Ok(count)
}

fn build_preview(app: &AppHandle, name: &str) {
    let label = label_for(name);
    let prefs = prefs_for(app, name);
    let init = format!(
        "window.__PREVIEW_NAME__ = {};",
        serde_json::Value::String(name.to_string())
    );
    let url = format!("preview.html?name={}", urlencode(name));
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .initialization_script(init)
        .title(format!("{name} · preview"))
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(true)
        .inner_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
        // Hidden until it's in its saved spot (no flash in the default corner).
        .visible(false)
        .build();
    let window = match built {
        Ok(window) => window,
        Err(err) => {
            log::warn!("couldn't open preview for {name}: {err}");
            return;
        }
    };
    if let (Some(w), Some(h)) = (prefs.w, prefs.h) {
        let _ = window.set_size(PhysicalSize::new(
            w.max(MIN_SIZE.0 as u32),
            h.max(MIN_SIZE.1 as u32),
        ));
    }
    if let (Some(x), Some(y)) = (prefs.x, prefs.y) {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    apply_mode(app, &window, name);

    // In edit mode, remember where it's dragged and how big it's made.
    let app_for_events = app.clone();
    let name_for_events = name.to_string();
    window.on_window_event(move |event| {
        if !edit_mode(&app_for_events) {
            return;
        }
        match event {
            WindowEvent::Moved(pos) => {
                with_layout(&app_for_events, |layout| {
                    let prefs = layout.clients.entry(name_for_events.clone()).or_default();
                    prefs.x = Some(pos.x);
                    prefs.y = Some(pos.y);
                });
                save_soon(&app_for_events);
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                with_layout(&app_for_events, |layout| {
                    let prefs = layout.clients.entry(name_for_events.clone()).or_default();
                    prefs.w = Some(size.width);
                    prefs.h = Some(size.height);
                });
                save_soon(&app_for_events);
            }
            _ => {}
        }
    });
}

/// Edit mode on / off for every preview. On: every open client gets a preview (hidden ones
/// dimmed), all can be moved and nothing clicks through. Off: hidden ones close, the rest
/// lock in place and the click-through ones let clicks through.
#[tauri::command]
pub fn set_preview_edit(app: AppHandle, on: bool) -> Result<(), String> {
    with_layout(&app, |layout| layout.edit = on);
    save_soon(&app);
    if on {
        // Make sure every open client has a preview to arrange.
        let _ = open_previews(app.clone());
    }
    if !on {
        snap_all(&app);
    }
    for (label, window) in app.webview_windows() {
        if !label.starts_with(PREFIX) {
            continue;
        }
        let Some(name) = name_of_window(&window) else {
            continue;
        };
        if !on && !prefs_for(&app, &name).enabled {
            destroy_all(&app, vec![window]);
            continue;
        }
        apply_mode(&app, &window, &name);
    }
    Ok(())
}

/// Change one client's preview: shown, click-through, opacity.
#[tauri::command]
pub fn set_preview_client(
    app: AppHandle,
    name: String,
    enabled: Option<bool>,
    click_through: Option<bool>,
    opacity: Option<f64>,
) -> Result<(), String> {
    with_layout(&app, |layout| {
        let prefs = layout.clients.entry(name.clone()).or_default();
        if let Some(enabled) = enabled {
            prefs.enabled = enabled;
        }
        if let Some(click_through) = click_through {
            prefs.click_through = click_through;
        }
        if let Some(opacity) = opacity {
            prefs.opacity = opacity.clamp(0.2, 1.0);
        }
    });
    save_soon(&app);
    if let Some(window) = app.get_webview_window(&label_for(&name)) {
        if !edit_mode(&app) && !prefs_for(&app, &name).enabled {
            destroy_all(&app, vec![window]);
        } else {
            apply_mode(&app, &window, &name);
        }
    }
    Ok(())
}

/// Patch 70: open previews by themselves when EVE clients start (Home's switch).
/// On / off: hide the preview of the client in front.
#[tauri::command]
pub fn set_previews_hide_active(app: AppHandle, on: bool) {
    with_layout(&app, |layout| layout.hide_active = on);
    save_soon(&app);
    if !on {
        let windows: Vec<WebviewWindow> = app.webview_windows().into_iter().filter(|(l, _)| l.starts_with(PREFIX)).map(|(_, w)| w).collect();
        let _ = app.run_on_main_thread(move || {
            for window in windows {
                let _ = window.show();
            }
        });
    } else {
        push_active(&app, crate::clipboard::foreground_client().as_deref());
    }
}

/// On / off: previews for clients at the login / character-select screen.
#[tauri::command]
pub fn set_previews_logins(app: AppHandle, on: bool) {
    with_layout(&app, |layout| layout.login_previews = on);
    save_soon(&app);
    if !on {
        let gone: Vec<WebviewWindow> = app
            .webview_windows()
            .into_iter()
            .filter(|(l, _)| l.starts_with(PREFIX))
            .map(|(_, w)| w)
            .filter(|w| name_of_window(w).is_some_and(|n| crate::clipboard::is_login_label(&n)))
            .collect();
        destroy_all(&app, gone);
    }
}

/// How often each preview should ask for a picture: twice a second with a few previews,
/// slower as there are more (the total stays around 16 pictures a second).
#[tauri::command]
pub fn preview_interval(app: AppHandle) -> u64 {
    let open = app.webview_windows().keys().filter(|l| l.starts_with(PREFIX)).count() as u64;
    (open * 1000 / 16).clamp(500, 3000)
}

/// The snap grid in pixels (0 = off).
#[tauri::command]
pub fn set_previews_snap(app: AppHandle, grid: u32) {
    with_layout(&app, |layout| layout.snap = grid.min(100));
    save_soon(&app);
}

/// One rectangle snapped: round to the grid, then edges within `pull` px of another
/// rectangle's edge line up with it (side by side or flush).
pub fn snap_rect(rect: (i32, i32, i32, i32), others: &[(i32, i32, i32, i32)], grid: i32, pull: i32) -> (i32, i32, i32, i32) {
    let round = |v: i32| if grid > 1 { ((v as f64 / grid as f64).round() as i32) * grid } else { v };
    let (mut x, mut y, mut w, mut h) = (round(rect.0), round(rect.1), round(rect.2).max(MIN_SIZE.0 as i32), round(rect.3).max(MIN_SIZE.1 as i32));
    for &(ox, oy, ow, oh) in others {
        for target in [ox, ox + ow] {
            if (x - target).abs() <= pull {
                x = target;
            } else if (x + w - target).abs() <= pull {
                x = target - w;
            }
        }
        for target in [oy, oy + oh] {
            if (y - target).abs() <= pull {
                y = target;
            } else if (y + h - target).abs() <= pull {
                y = target - h;
            }
        }
    }
    (x, y, w, h)
}

fn snap_all(app: &AppHandle) {
    let grid = with_layout(app, |layout| layout.snap) as i32;
    if grid == 0 {
        return;
    }
    let windows: Vec<(String, WebviewWindow)> = app.webview_windows().into_iter().filter(|(l, _)| l.starts_with(PREFIX)).filter_map(|(_, w)| name_of_window(&w).map(|n| (n, w))).collect();
    let mut placed: Vec<(i32, i32, i32, i32)> = Vec::new();
    for (name, window) in windows {
        let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) else { continue };
        let (x, y, w, h) = snap_rect((pos.x, pos.y, size.width as i32, size.height as i32), &placed, grid, 12);
        placed.push((x, y, w, h));
        let _ = window.set_position(PhysicalPosition::new(x, y));
        let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
        with_layout(app, |layout| {
            let prefs = layout.clients.entry(name).or_default();
            prefs.x = Some(x);
            prefs.y = Some(y);
            prefs.w = Some(w as u32);
            prefs.h = Some(h as u32);
        });
    }
    save_soon(app);
}

#[tauri::command]
pub fn set_previews_auto(app: AppHandle, on: bool) {
    with_layout(&app, |layout| layout.auto_open = on);
    save_soon(&app);
}

/// Patch 70: watch the client in front and the clients open. The client in front is pushed to
/// every preview the moment it changes (the green border no longer waits for a picture), and
/// new EVE clients get their previews without anyone pressing a button.
pub fn start_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut front: Option<String> = None;
        let mut front_handle: isize = 0;
        let mut known: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut last_open: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(80));
            let handle = crate::clipboard::foreground_handle();
            if handle != front_handle {
                front_handle = handle;
                let now = crate::clipboard::foreground_client();
                if now.is_some() && now != front {
                    front = now.clone();
                    push_active(&app, front.as_deref());
                }
            }
            tick += 1;
            if tick % 5 != 0 {
                continue;
            }
            // Every 0.4 s: a client that has closed takes its preview with it (gone two checks in
            // a row, so a title change while logging in doesn't count). Not in edit mode; the
            // preview comes back by itself when that client starts again.
            let open: std::collections::BTreeSet<String> = crate::clipboard::open_clients()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            if !with_layout(&app, |layout| layout.edit) {
                let gone: Vec<WebviewWindow> = app
                    .webview_windows()
                    .into_iter()
                    .filter(|(label, _)| label.starts_with(PREFIX))
                    .map(|(_, window)| window)
                    .filter(|window| {
                        name_of_window(window)
                            .is_some_and(|name| !open.contains(&name) && !last_open.contains(&name))
                    })
                    .collect();
                if !gone.is_empty() {
                    // Windows are closed on the main thread: closing one from this watcher
                    // thread left its frame on screen with a dead page inside.
                    destroy_all(&app, gone);
                }
            }
            last_open = open.clone();
            // Every 2 s: new clients get their previews.
            if tick % 25 == 0 {
                let new_names: Vec<String> = open.iter().filter(|name| !known.contains(*name)).cloned().collect();
                let fresh = new_names.iter().any(|n| !crate::clipboard::is_login_label(n));
                known = open;
                if fresh {
                    let app = app.clone();
                    // A new client (or one that just logged in) goes to its remembered spot.
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        crate::clientwindows::on_new_clients(&app, &new_names);
                    });
                }
                if fresh
                    && !AUTO_PAUSED.load(std::sync::atomic::Ordering::Relaxed)
                    && with_layout(&app, |layout| layout.auto_open)
                {
                    let app = app.clone();
                    // Give a new client a moment to draw something before its preview opens.
                    let open_now = app.webview_windows().keys().filter(|l| l.starts_with(PREFIX)).count();
                    if open_now < AUTO_MAX {
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            let _ = open_previews(app);
                        });
                    }
                }
            }
        }
    });
}

/// Tell every preview which client is in front now (and, with "hide the one in front", hide
/// that client's preview and bring the others back).
pub fn push_active(app: &AppHandle, name: Option<&str>) {
    let arg = serde_json::to_string(&name).unwrap_or_else(|_| "null".into());
    let (hide, edit) = with_layout(app, |layout| (layout.hide_active, layout.edit));
    if hide && !edit {
        let front = name.map(str::to_string);
        let windows: Vec<WebviewWindow> = app.webview_windows().into_iter().filter(|(l, _)| l.starts_with(PREFIX)).map(|(_, w)| w).collect();
        let _ = app.run_on_main_thread(move || {
            for window in windows {
                let is_front = front.is_some() && name_of_window(&window) == front;
                let _ = if is_front { window.hide() } else { window.show() };
            }
        });
    }
    for (label, window) in app.webview_windows() {
        if label.starts_with(PREFIX) {
            let _ = window.eval(format!(
                "window.__previewActive && window.__previewActive({arg});"
            ));
        }
    }
}

/// The master opacity for every preview at once (Home's slider).
#[tauri::command]
pub fn set_previews_opacity(app: AppHandle, opacity: f64) {
    with_layout(&app, |layout| layout.opacity = opacity.clamp(0.1, 1.0));
    save_soon(&app);
    for (label, window) in app.webview_windows() {
        if let (true, Some(name)) = (label.starts_with(PREFIX), name_of_window(&window)) {
            apply_mode(&app, &window, &name);
        }
    }
}

/// Resize the calling preview (its corner handle in edit mode), in physical pixels.
#[tauri::command]
pub fn preview_resize(window: WebviewWindow, width: u32, height: u32) {
    let _ = window.set_size(PhysicalSize::new(
        width.max(MIN_SIZE.0 as u32),
        height.max(MIN_SIZE.1 as u32),
    ));
}

/// Close one preview (its own ✕). In edit mode this also turns that client's preview off.
#[tauri::command]
pub fn close_preview(app: AppHandle, window: WebviewWindow) {
    if edit_mode(&app) {
        if let Some(name) = name_of_window(&window) {
            with_layout(&app, |layout| {
                layout.clients.entry(name).or_default().enabled = false
            });
            save_soon(&app);
        }
    }
    destroy_all(&app, vec![window]);
}

/// Close every preview window (and leave edit mode). New clients don't bring previews back
/// until Open previews is pressed again.
#[tauri::command]
pub fn close_previews(app: AppHandle) {
    AUTO_PAUSED.store(true, std::sync::atomic::Ordering::Relaxed);
    with_layout(&app, |layout| layout.edit = false);
    save_soon(&app);
    let all = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(PREFIX))
        .map(|(_, w)| w)
        .collect();
    destroy_all(&app, all);
}

/// Set by Close previews, cleared by Open previews: stops new clients opening previews meanwhile.
static AUTO_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hide straight away, then destroy on the main thread (a destroy from elsewhere can leave the
/// frame up with "failed to acquire webview reference" inside).
fn destroy_all(app: &AppHandle, windows: Vec<WebviewWindow>) {
    if windows.is_empty() {
        return;
    }
    for window in &windows {
        let _ = window.hide();
    }
    let _ = app.run_on_main_thread(move || {
        for window in windows {
            let _ = window.destroy();
        }
    });
}

/// Which character a preview window shows (from its title "Name · preview").
fn name_of_window(window: &WebviewWindow) -> Option<String> {
    window
        .title()
        .ok()
        .and_then(|title| title.strip_suffix(" · preview").map(str::to_string))
}

/// Pick an EVE-O Preview config file and bring in its thumbnail spots / sizes and its cycle
/// groups (as launch groups with their pedal hotkeys). Returns a short summary.
#[tauri::command]
pub async fn import_eveo(app: AppHandle) -> Result<String, String> {
    let app2 = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app2.dialog()
            .file()
            .add_filter("EVE-O Preview settings", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(file) = picked else {
        return Ok("Nothing picked.".to_string());
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("not an EVE-O Preview file: {e}"))?;
    let imported = parse_eveo(&value);

    let spots = imported.spots.len();
    with_layout(&app, |layout| {
        for spot in &imported.spots {
            let prefs = layout.clients.entry(spot.name.clone()).or_default();
            prefs.x = Some(spot.x);
            prefs.y = Some(spot.y);
            prefs.w = Some(spot.w);
            prefs.h = Some(spot.h);
        }
    });
    save_soon(&app);

    let groups: Vec<crate::launch::Group> = imported
        .groups
        .into_iter()
        .map(|g| crate::launch::Group {
            id: format!("eveo-{}", g.number),
            name: format!("EVE-O cycle group {}", g.number),
            members: g.members,
            skipped: g.skipped,
            forward_hotkey: g.forward,
            backward_hotkey: g.backward,
        })
        .collect();
    let group_count = groups.len();
    crate::launch::merge_imported(&app, groups)?;
    Ok(format!(
        "Imported {spots} preview spots and {group_count} cycle group(s) with their hotkeys."
    ))
}

// ---- EVE-O Preview import (pure, tested) -----------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct Spot {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, PartialEq)]
pub struct CycleGroup {
    pub number: u32,
    pub forward: Option<String>,
    pub backward: Option<String>,
    pub members: Vec<String>,
    /// EVE-O "!Name" entries: in the group, switched off.
    pub skipped: Vec<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct EveoImport {
    pub spots: Vec<Spot>,
    pub groups: Vec<CycleGroup>,
}

/// "EVE - !Tsawind" -> "Tsawind"; plain "EVE" and placeholders -> None.
fn character_of(title: &str) -> Option<String> {
    let name = title
        .strip_prefix("EVE - ")?
        .trim()
        .trim_start_matches('!')
        .trim();
    let junk =
        name.is_empty() || name.starts_with("Example Toon") || name.starts_with("cycle group");
    (!junk).then(|| name.to_string())
}

/// "1311, 1767" -> (1311, 1767).
fn pair(text: &str) -> Option<(i64, i64)> {
    let mut parts = text.split(',').map(|part| part.trim().parse::<i64>());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b)), None) => Some((a, b)),
        _ => None,
    }
}

/// The first usable hotkey of a list ("" and dangling "Control+" are skipped).
fn hotkey(list: Option<&serde_json::Value>) -> Option<String> {
    list?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .find(|key| !key.is_empty() && !key.ends_with('+'))
        .map(str::to_string)
}

pub fn parse_eveo(config: &serde_json::Value) -> EveoImport {
    let mut out = EveoImport::default();

    let (default_w, default_h) = config
        .get("ThumbnailSize")
        .and_then(|v| v.as_str())
        .and_then(pair)
        .unwrap_or((320, 200));
    let sizes: BTreeMap<String, (i64, i64)> = config
        .get("PerClientThumbnailSize")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(title, size)| Some((character_of(title)?, pair(size.as_str()?)?)))
                .collect()
        })
        .unwrap_or_default();
    if let Some(flat) = config.get("FlatLayout").and_then(|v| v.as_object()) {
        for (title, spot) in flat {
            let (Some(name), Some((x, y))) = (character_of(title), spot.as_str().and_then(pair))
            else {
                continue;
            };
            if out.spots.iter().any(|known| known.name == name) {
                continue;
            }
            let (w, h) = sizes.get(&name).copied().unwrap_or((default_w, default_h));
            out.spots.push(Spot {
                name,
                x: x as i32,
                y: y as i32,
                w: w.max(MIN_SIZE.0 as i64) as u32,
                h: h.max(MIN_SIZE.1 as i64) as u32,
            });
        }
    }

    // EVE-O's single spot for a client at the login screen becomes our "Login 1" spot.
    if let Some((x, y)) = config
        .get("LoginThumbnailLocation")
        .and_then(|v| v.as_str())
        .and_then(pair)
    {
        let name = crate::clipboard::login_label(0);
        if !out.spots.iter().any(|known| known.name == name) {
            let (w, h) = (
                default_w.max(MIN_SIZE.0 as i64) as u32,
                default_h.max(MIN_SIZE.1 as i64) as u32,
            );
            out.spots.push(Spot {
                name,
                x: x as i32,
                y: y as i32,
                w,
                h,
            });
        }
    }

    // Same order whatever order the file's keys came in.
    out.spots.sort_by(|a, b| a.name.cmp(&b.name));

    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for number in 1..=5u32 {
        let Some(order) = config
            .get(format!("CycleGroup{number}ClientsOrder"))
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let mut ranked: Vec<(i64, usize, String, bool)> = order
            .iter()
            .enumerate()
            .filter_map(|(i, (title, rank))| {
                // EVE-O's bare "EVE" entry = clients at the login / character-select screen.
                let name = if title.trim() == "EVE" {
                    crate::cycle::LOGIN_SCREENS.to_string()
                } else {
                    character_of(title)?
                };
                let off = title
                    .trim()
                    .strip_prefix("EVE - ")
                    .is_some_and(|rest| rest.trim_start().starts_with('!'));
                Some((rank.as_i64().unwrap_or(i64::MAX), i, name, off))
            })
            .collect();
        ranked.sort();
        let mut members: Vec<String> = Vec::new();
        let mut on: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, _, name, off) in ranked {
            if !off {
                on.insert(name.clone());
            }
            if !members.contains(&name) {
                members.push(name);
            }
        }
        // "!Name" switches a character off unless the group also lists it plainly.
        let skipped: Vec<String> = members
            .iter()
            .filter(|name| !on.contains(*name))
            .cloned()
            .collect();
        if members.is_empty() {
            continue;
        }
        // A key can only drive one group; the lower-numbered group keeps it (EVE-O's order).
        let mut claim =
            |key: Option<String>| key.filter(|k| claimed.insert(k.to_ascii_uppercase()));
        let forward = claim(hotkey(
            config.get(format!("CycleGroup{number}ForwardHotkeys")),
        ));
        let backward = claim(hotkey(
            config.get(format!("CycleGroup{number}BackwardHotkeys")),
        ));
        out.groups.push(CycleGroup {
            number,
            forward,
            backward,
            members,
            skipped,
        });
    }
    out
}

// ---- Small helpers ---------------------------------------------------------------------------

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(CHARS[(n >> 18 & 63) as usize] as char);
        out.push(CHARS[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_safe_and_stable() {
        assert_eq!(label_for("Luigi Squirrel"), "preview-LuigiSquirrel");
        assert_eq!(label_for("Déjà'Vu 2"), "preview-DjVu2");
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"EVE"), "RVZF");
    }

    #[test]
    fn urlencodes_names() {
        assert_eq!(urlencode("Luigi Squirrel"), "Luigi%20Squirrel");
        assert_eq!(urlencode("A-b_c.d~e"), "A-b_c.d~e");
    }

    #[test]
    fn old_layout_files_still_load() {
        let layout: Layout =
            serde_json::from_str(r#"{"clients":{"Tsawind":{"x":10,"y":20}}}"#).unwrap();
        let prefs = &layout.clients["Tsawind"];
        assert!(prefs.enabled);
        assert!(!prefs.click_through);
        assert_eq!(prefs.opacity, 1.0);
        assert_eq!((prefs.x, prefs.y), (Some(10), Some(20)));
        assert_eq!(layout.opacity, 1.0);
    }

    #[test]
    fn reads_character_names_from_eveo_titles() {
        assert_eq!(character_of("EVE - !Tsawind").as_deref(), Some("Tsawind"));
        assert_eq!(
            character_of("EVE - The NocnitsaZ").as_deref(),
            Some("The NocnitsaZ")
        );
        assert_eq!(character_of("EVE"), None);
        assert_eq!(character_of("EVE - Example Toon 1"), None);
        assert_eq!(character_of("EVE - cycle group 4"), None);
    }

    #[test]
    fn imports_spots_and_cycle_groups() {
        let config = serde_json::json!({
            "ThumbnailSize": "100, 50",
            "PerClientThumbnailSize": { "EVE - Example Toon 1": "200, 200", "EVE - Tsawind": "300, 160" },
            "FlatLayout": { "EVE - Tsawind": "1211, 1368", "EVE - LuigiSquirrel": "1311, 1767", "EVE": "1411, 1667" },
            "LoginThumbnailLocation": "900, 1100",
            "CycleGroup1ForwardHotkeys": ["F15", "Control+F15"],
            "CycleGroup1BackwardHotkeys": ["", "Control+"],
            "CycleGroup1ClientsOrder": { "EVE": 1 },
            "CycleGroup3ForwardHotkeys": ["F13"],
            "CycleGroup3BackwardHotkeys": [""],
            "CycleGroup3ClientsOrder": { "EVE - !Tsawind": 1, "EVE - !LuigiSquirrel": 10, "EVE - Darkness Holder": 2, "EVE - Tsawind": 1 }
        });
        let import = parse_eveo(&config);
        assert_eq!(
            import.spots,
            vec![
                Spot {
                    name: "Login 1".into(),
                    x: 900,
                    y: 1100,
                    w: 100,
                    h: 50
                },
                Spot {
                    name: "LuigiSquirrel".into(),
                    x: 1311,
                    y: 1767,
                    w: 100,
                    h: 50
                },
                Spot {
                    name: "Tsawind".into(),
                    x: 1211,
                    y: 1368,
                    w: 300,
                    h: 160
                },
            ]
        );
        // Group 1's bare "EVE" entry is the login screens, cycled with F15.
        assert_eq!(import.groups.len(), 2);
        assert_eq!(import.groups[0].members, vec![crate::cycle::LOGIN_SCREENS]);
        assert_eq!(import.groups[0].forward.as_deref(), Some("F15"));
        let group = &import.groups[1];
        assert_eq!(group.number, 3);
        assert_eq!(group.forward.as_deref(), Some("F13"));
        assert_eq!(group.backward, None);
        assert_eq!(
            group.members,
            vec!["Tsawind", "Darkness Holder", "LuigiSquirrel"]
        );
        // "!LuigiSquirrel" only: off. "!Tsawind" also listed plainly: on.
        assert_eq!(group.skipped, vec!["LuigiSquirrel"]);
    }

    #[test]
    fn reads_size_pairs() {
        assert_eq!(pair("1311, 1767"), Some((1311, 1767)));
        assert_eq!(pair("-7,0"), Some((-7, 0)));
        assert_eq!(pair("nope"), None);
        assert_eq!(pair("1,2,3"), None);
    }
}

#[cfg(test)]
mod snap_tests {
    use super::snap_rect;

    #[test]
    fn rounds_to_the_grid() {
        assert_eq!(snap_rect((13, 27, 318, 203), &[], 10, 12), (10, 30, 320, 200));
    }

    #[test]
    fn lines_up_with_a_neighbour() {
        // Right next to one at x 0..320: its left edge pulls flush to 320.
        assert_eq!(snap_rect((326, 4, 320, 200), &[(0, 0, 320, 200)], 1, 12), (320, 0, 320, 200));
    }

    #[test]
    fn grid_off_leaves_it() {
        assert_eq!(snap_rect((13, 27, 318, 203), &[], 0, 0), (13, 27, 318, 203));
    }
}
