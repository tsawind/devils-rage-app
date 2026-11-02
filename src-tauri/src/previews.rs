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
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

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
        ClientPrefs { enabled: true, click_through: false, opacity: 1.0, x: None, y: None, w: None, h: None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub edit: bool,
    #[serde(default)]
    pub clients: BTreeMap<String, ClientPrefs>,
}

static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);
static SAVE_PENDING: AtomicBool = AtomicBool::new(false);

fn layout_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("previews.json"))
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
        let text = with_layout(&app, |layout| serde_json::to_string_pretty(layout).unwrap_or_default());
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
    with_layout(app, |layout| layout.clients.get(name).cloned().unwrap_or_default())
}

fn edit_mode(app: &AppHandle) -> bool {
    with_layout(app, |layout| layout.edit)
}

/// What a preview page needs to draw itself (sent on open and after every change).
fn mode_json(app: &AppHandle, name: &str) -> serde_json::Value {
    let prefs = prefs_for(app, name);
    serde_json::json!({
        "edit": edit_mode(app),
        "enabled": prefs.enabled,
        "clickThrough": prefs.click_through,
        "opacity": prefs.opacity,
    })
}

/// Push the mode to one preview window: click-through (locked mode only) and its page.
fn apply_mode(app: &AppHandle, window: &WebviewWindow, name: &str) {
    let prefs = prefs_for(app, name);
    let edit = edit_mode(app);
    let _ = window.set_ignore_cursor_events(!edit && prefs.click_through);
    let _ = window.eval(format!("window.__previewMode && window.__previewMode({});", mode_json(app, name)));
}

// ---- Commands -------------------------------------------------------------------------------

/// The EVE clients open right now (character name + whether it's minimized).
#[tauri::command]
pub fn preview_clients() -> Vec<serde_json::Value> {
    crate::clipboard::open_eve_windows()
        .into_iter()
        .map(|(name, handle)| serde_json::json!({ "name": name, "minimized": crate::capture::minimized_handle(handle) }))
        .collect()
}

/// One client's picture as a data URL, for its preview window to draw. The capture and JPEG
/// encode run on a blocking thread so a preview never stalls the UI thread.
#[tauri::command]
pub async fn preview_frame(name: String, width: Option<u32>, quality: Option<u8>) -> Result<String, String> {
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
    let clients = crate::clipboard::open_eve_windows();
    if clients.is_empty() {
        return Err("No EVE clients are open.".to_string());
    }
    let edit = edit_mode(&app);
    let wanted: Vec<String> = clients.into_iter().map(|(name, _)| name).filter(|name| edit || prefs_for(&app, name).enabled).collect();
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
    let init = format!("window.__PREVIEW_NAME__ = {};", serde_json::Value::String(name.to_string()));
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
        let _ = window.set_size(PhysicalSize::new(w.max(MIN_SIZE.0 as u32), h.max(MIN_SIZE.1 as u32)));
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
    for (label, window) in app.webview_windows() {
        if !label.starts_with(PREFIX) {
            continue;
        }
        let Some(name) = name_of_window(&window) else { continue };
        if !on && !prefs_for(&app, &name).enabled {
            let _ = window.destroy();
            continue;
        }
        apply_mode(&app, &window, &name);
    }
    Ok(())
}

/// Change one client's preview: shown, click-through, opacity.
#[tauri::command]
pub fn set_preview_client(app: AppHandle, name: String, enabled: Option<bool>, click_through: Option<bool>, opacity: Option<f64>) -> Result<(), String> {
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
            let _ = window.destroy();
        } else {
            apply_mode(&app, &window, &name);
        }
    }
    Ok(())
}

/// Resize the calling preview (its corner handle in edit mode), in physical pixels.
#[tauri::command]
pub fn preview_resize(window: WebviewWindow, width: u32, height: u32) {
    let _ = window.set_size(PhysicalSize::new(width.max(MIN_SIZE.0 as u32), height.max(MIN_SIZE.1 as u32)));
}

/// Close one preview (its own ✕). In edit mode this also turns that client's preview off.
#[tauri::command]
pub fn close_preview(app: AppHandle, window: WebviewWindow) {
    if edit_mode(&app) {
        if let Some(name) = name_of_window(&window) {
            with_layout(&app, |layout| layout.clients.entry(name).or_default().enabled = false);
            save_soon(&app);
        }
    }
    let _ = window.destroy();
}

/// Close every preview window (and leave edit mode).
#[tauri::command]
pub fn close_previews(app: AppHandle) {
    with_layout(&app, |layout| layout.edit = false);
    save_soon(&app);
    for (label, window) in app.webview_windows() {
        if label.starts_with(PREFIX) {
            let _ = window.destroy();
        }
    }
}

/// Which character a preview window shows (from its title "Name · preview").
fn name_of_window(window: &WebviewWindow) -> Option<String> {
    window.title().ok().and_then(|title| title.strip_suffix(" · preview").map(str::to_string))
}

/// Pick an EVE-O Preview config file and bring in its thumbnail spots / sizes and its cycle
/// groups (as launch groups with their pedal hotkeys). Returns a short summary.
#[tauri::command]
pub async fn import_eveo(app: AppHandle) -> Result<String, String> {
    let app2 = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app2.dialog().file().add_filter("EVE-O Preview settings", &["json"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(file) = picked else {
        return Ok("Nothing picked.".to_string());
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("not an EVE-O Preview file: {e}"))?;
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
            forward_hotkey: g.forward,
            backward_hotkey: g.backward,
        })
        .collect();
    let group_count = groups.len();
    crate::launch::merge_imported(&app, groups)?;
    Ok(format!("Imported {spots} preview spots and {group_count} cycle group(s) with their hotkeys."))
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
}

#[derive(Debug, Default, PartialEq)]
pub struct EveoImport {
    pub spots: Vec<Spot>,
    pub groups: Vec<CycleGroup>,
}

/// "EVE - !Tsawind" -> "Tsawind"; plain "EVE" and placeholders -> None.
fn character_of(title: &str) -> Option<String> {
    let name = title.strip_prefix("EVE - ")?.trim().trim_start_matches('!').trim();
    let junk = name.is_empty() || name.starts_with("Example Toon") || name.starts_with("cycle group");
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

    let (default_w, default_h) = config.get("ThumbnailSize").and_then(|v| v.as_str()).and_then(pair).unwrap_or((320, 200));
    let sizes: BTreeMap<String, (i64, i64)> = config
        .get("PerClientThumbnailSize")
        .and_then(|v| v.as_object())
        .map(|map| map.iter().filter_map(|(title, size)| Some((character_of(title)?, pair(size.as_str()?)?))).collect())
        .unwrap_or_default();
    if let Some(flat) = config.get("FlatLayout").and_then(|v| v.as_object()) {
        for (title, spot) in flat {
            let (Some(name), Some((x, y))) = (character_of(title), spot.as_str().and_then(pair)) else { continue };
            if out.spots.iter().any(|known| known.name == name) {
                continue;
            }
            let (w, h) = sizes.get(&name).copied().unwrap_or((default_w, default_h));
            out.spots.push(Spot { name, x: x as i32, y: y as i32, w: w.max(MIN_SIZE.0 as i64) as u32, h: h.max(MIN_SIZE.1 as i64) as u32 });
        }
    }

    // Same order whatever order the file's keys came in.
    out.spots.sort_by(|a, b| a.name.cmp(&b.name));

    for number in 1..=5u32 {
        let Some(order) = config.get(format!("CycleGroup{number}ClientsOrder")).and_then(|v| v.as_object()) else { continue };
        let mut ranked: Vec<(i64, usize, String)> = order
            .iter()
            .enumerate()
            .filter_map(|(i, (title, rank))| Some((rank.as_i64().unwrap_or(i64::MAX), i, character_of(title)?)))
            .collect();
        ranked.sort();
        let mut members: Vec<String> = Vec::new();
        for (_, _, name) in ranked {
            if !members.contains(&name) {
                members.push(name);
            }
        }
        if members.is_empty() {
            continue;
        }
        out.groups.push(CycleGroup {
            number,
            forward: hotkey(config.get(format!("CycleGroup{number}ForwardHotkeys"))),
            backward: hotkey(config.get(format!("CycleGroup{number}BackwardHotkeys"))),
            members,
        });
    }
    out
}

// ---- Small helpers ---------------------------------------------------------------------------

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(CHARS[(n >> 18 & 63) as usize] as char);
        out.push(CHARS[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' });
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
        let layout: Layout = serde_json::from_str(r#"{"clients":{"Tsawind":{"x":10,"y":20}}}"#).unwrap();
        let prefs = &layout.clients["Tsawind"];
        assert!(prefs.enabled);
        assert!(!prefs.click_through);
        assert_eq!(prefs.opacity, 1.0);
        assert_eq!((prefs.x, prefs.y), (Some(10), Some(20)));
    }

    #[test]
    fn reads_character_names_from_eveo_titles() {
        assert_eq!(character_of("EVE - !Tsawind").as_deref(), Some("Tsawind"));
        assert_eq!(character_of("EVE - The NocnitsaZ").as_deref(), Some("The NocnitsaZ"));
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
                Spot { name: "LuigiSquirrel".into(), x: 1311, y: 1767, w: 100, h: 50 },
                Spot { name: "Tsawind".into(), x: 1211, y: 1368, w: 300, h: 160 },
            ]
        );
        // Group 1 only had the junk "EVE" entry: skipped.
        assert_eq!(import.groups.len(), 1);
        let group = &import.groups[0];
        assert_eq!(group.number, 3);
        assert_eq!(group.forward.as_deref(), Some("F13"));
        assert_eq!(group.backward, None);
        assert_eq!(group.members, vec!["Tsawind", "Darkness Holder", "LuigiSquirrel"]);
    }

    #[test]
    fn reads_size_pairs() {
        assert_eq!(pair("1311, 1767"), Some((1311, 1767)));
        assert_eq!(pair("-7,0"), Some((-7, 0)));
        assert_eq!(pair("nope"), None);
        assert_eq!(pair("1,2,3"), None);
    }
}
