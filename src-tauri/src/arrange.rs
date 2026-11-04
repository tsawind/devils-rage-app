//! Patch 117: arrange the EVE client windows themselves, for clients in **Fixed Window** mode
//! (no title bar, so EVE can't be dragged by hand). Arrange puts a see-through handle of ours
//! over every logged-in client: drag a handle and its client's window follows, pull its corner
//! and the window resizes. Done snaps them to the preview snap grid, lines up edges, remembers
//! every spot (in a stack's spot when the client is in one) and takes the handles away.
//!
//! This moves the game's window the way dragging its title bar would (SetWindowPos); nothing
//! inside the game is clicked or typed.

use crate::clientwindows::{self, Spot};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const PREFIX: &str = "arrange-";
static ARRANGING: AtomicBool = AtomicBool::new(false);

pub fn arranging() -> bool {
    ARRANGING.load(Ordering::Relaxed)
}

fn label_for(name: &str) -> String {
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{PREFIX}{slug}")
}

/// The handle's rectangle (it has no frame, so outer = the whole thing).
fn rect_of(window: &tauri::WebviewWindow) -> Option<Spot> {
    let (pos, size) = (window.outer_position().ok()?, window.outer_size().ok()?);
    Some(Spot { x: pos.x, y: pos.y, w: size.width as i32, h: size.height as i32 })
}

/// Arrange on / off (Home's button and each handle's Done).
#[tauri::command]
pub fn arrange_windows(app: AppHandle, on: bool) -> Result<String, String> {
    if on {
        let clients: Vec<(String, isize, Spot)> = crate::clipboard::open_clients()
            .into_iter()
            .filter(|(name, _)| clientwindows::keeps_a_spot(name))
            .filter_map(|(name, handle)| clientwindows::window_spot(handle).map(|spot| (name, handle, spot)))
            .collect();
        if clients.is_empty() {
            return Err("No logged-in EVE clients to arrange (minimised ones are skipped).".into());
        }
        ARRANGING.store(true, Ordering::Relaxed);
        let count = clients.len();
        // Built off the command thread (building webviews inside a command froze the app before).
        tauri::async_runtime::spawn(async move {
            for (name, handle, spot) in clients {
                if app.get_webview_window(&label_for(&name)).is_none() {
                    build_handle(&app, &name, handle, spot);
                }
            }
        });
        Ok(format!("Arranging {count} client{}: drag a handle to move its window, pull its corner to resize. Done when finished.", if count == 1 { "" } else { "s" }))
    } else {
        ARRANGING.store(false, Ordering::Relaxed);
        let handles: Vec<tauri::WebviewWindow> = app.webview_windows().into_iter().filter(|(l, _)| l.starts_with(PREFIX)).map(|(_, w)| w).collect();
        // Snap: grid from the previews' setting, edges within 12 px of a neighbour line up.
        let grid = crate::previews::snap_grid(&app) as i32;
        let mut placed: Vec<(i32, i32, i32, i32)> = Vec::new();
        for window in &handles {
            let Some(name) = name_of(window) else { continue };
            let Some(r) = rect_of(window) else { continue };
            let (x, y, w, h) = if grid > 0 { crate::previews::snap_rect((r.x, r.y, r.w, r.h), &placed, grid, 12) } else { (r.x, r.y, r.w, r.h) };
            placed.push((x, y, w, h));
            if let Some(handle) = handle_of(&name) {
                clientwindows::place_window(handle, Spot { x, y, w, h });
            }
        }
        let said = clientwindows::client_windows_remember(app.clone()).unwrap_or_else(|e| e);
        let _ = app.run_on_main_thread(move || {
            for window in handles {
                let _ = window.destroy();
            }
        });
        Ok(format!("Windows locked in. {said}"))
    }
}

fn name_of(window: &tauri::WebviewWindow) -> Option<String> {
    window.title().ok().and_then(|t| t.strip_suffix(" · arrange").map(str::to_string))
}

fn handle_of(name: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h)
}

fn build_handle(app: &AppHandle, name: &str, handle: isize, spot: Spot) {
    let init = format!("window.__ARRANGE_NAME__ = {};", serde_json::Value::String(name.to_string()));
    let built = WebviewWindowBuilder::new(app, label_for(name), WebviewUrl::App("arrange.html".into()))
        .initialization_script(init)
        .title(format!("{name} · arrange"))
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(true)
        .min_inner_size(200.0, 120.0)
        .visible(false)
        .build();
    let window = match built {
        Ok(window) => window,
        Err(err) => {
            log::warn!("couldn't open the arrange handle for {name}: {err}");
            return;
        }
    };
    let _ = window.set_position(PhysicalPosition::new(spot.x, spot.y));
    let _ = window.set_size(PhysicalSize::new(spot.w.max(200) as u32, spot.h.max(120) as u32));
    let _ = window.show();
    // The client's window follows its handle.
    let follow = window.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) && arranging() {
            if let Some(rect) = rect_of(&follow).filter(|r| r.w > 100 && r.h > 100) {
                clientwindows::place_window(handle, rect);
            }
        }
    });
}

/// Resize the calling handle to an exact size (its W × H boxes), physical pixels.
#[tauri::command]
pub fn arrange_size(window: tauri::WebviewWindow, width: u32, height: u32) {
    let _ = window.set_size(PhysicalSize::new(width.clamp(200, 8000), height.clamp(120, 8000)));
}
