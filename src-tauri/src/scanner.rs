//! The scanner pop-out window: the mapper's Patch 39 pop-out, drawn by the mapper
//! into an app window that sits on top of EVE, without a Windows title bar.
//! It reopens where (and as big as) it was last time.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tauri::{
    webview::NewWindowFeatures, AppHandle, Manager, PhysicalPosition, PhysicalSize, Url,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::settings;

static NEXT: AtomicUsize = AtomicUsize::new(1);

/// Opens the window the mapper's pop-out draws into (pip.js asked for about:blank).
pub fn open(
    app: &AppHandle,
    marker: &str,
    url: Url,
    features: NewWindowFeatures,
) -> Option<WebviewWindow> {
    let label = format!("scanner-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .window_features(features)
        .title("Devil's Rage · scanner")
        .decorations(false)
        .always_on_top(true)
        .min_inner_size(260.0, 160.0)
        .on_new_window(crate::new_window_handler(
            app.clone(),
            marker.to_string(),
            label.clone(),
        ))
        .build()
        .ok()?;

    restore_bounds(app, &window);
    remember_bounds(app, &window);
    Some(window)
}

/// The scanner asking for something (pip.js opened https://dr-app.invalid/<what>).
pub fn signal(app: &AppHandle, me: &str, url: &Url) {
    if !me.starts_with("scanner-") {
        return;
    }
    let Some(window) = app.get_webview_window(me) else {
        return;
    };
    match url.path() {
        // Mouse is down on the pop-out's top bar: let Windows move the window.
        "/drag" => {
            let _ = window.start_dragging();
        }
        "/close" => {
            tauri::async_runtime::spawn(async move {
                let _ = window.close();
            });
        }
        // The mapper folding the map away / bringing it back (CSS pixels).
        "/resize-by" => {
            let dx = query_number(url, "dx");
            let dy = query_number(url, "dy");
            let scale = window.scale_factor().unwrap_or(1.0);
            if let Ok(size) = window.inner_size() {
                let width = (size.width as f64 + dx * scale).max(260.0 * scale);
                let height = (size.height as f64 + dy * scale).max(160.0 * scale);
                let _ = window.set_size(PhysicalSize::new(width as u32, height as u32));
            }
        }
        _ => {}
    }
}

fn query_number(url: &Url, key: &str) -> f64 {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Puts the window back where it was, if that spot is still on a screen.
fn restore_bounds(app: &AppHandle, window: &WebviewWindow) {
    let Some([x, y, w, h]) = settings::load(app).scanner_bounds else {
        return;
    };
    let on_screen = window
        .available_monitors()
        .map(|monitors| {
            monitors.iter().any(|m| {
                let p = m.position();
                let s = m.size();
                // The top-left corner (a little inside it) must be on this screen.
                let (cx, cy) = (x + 40, y + 10);
                cx >= p.x && cy >= p.y && cx < p.x + s.width as i32 && cy < p.y + s.height as i32
            })
        })
        .unwrap_or(false);
    if !on_screen {
        return;
    }
    let _ = window.set_size(PhysicalSize::new(w.max(200) as u32, h.max(120) as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// Keeps track of the window's place while it's open; saves it when it closes.
fn remember_bounds(app: &AppHandle, window: &WebviewWindow) {
    let app = app.clone();
    let me = window.clone();
    let last = Mutex::new(None::<[i32; 4]>);
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if let (Ok(p), Ok(s)) = (me.outer_position(), me.inner_size()) {
                if s.width > 0 && s.height > 0 {
                    *last.lock().unwrap() = Some([p.x, p.y, s.width as i32, s.height as i32]);
                }
            }
        }
        WindowEvent::Destroyed => {
            if let Some(bounds) = *last.lock().unwrap() {
                let mut s = settings::load(&app);
                s.scanner_bounds = Some(bounds);
                let _ = settings::save(&app, &s);
            }
        }
        _ => {}
    });
}
