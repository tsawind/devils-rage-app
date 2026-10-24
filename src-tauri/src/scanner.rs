//! The scanner pop-out window: the mapper's Patch 39 pop-out, drawn by the mapper
//! into an app window that sits on top of EVE, without a Windows title bar.
//! It reopens where (and as big as) it was last time, per pop-out: the main
//! window's pop-out and each extra character's pop-out keep their own spot.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tauri::{
    webview::NewWindowFeatures, AppHandle, Manager, PhysicalPosition, PhysicalSize, Url,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::settings;

static NEXT: AtomicUsize = AtomicUsize::new(1);

/// (scanner window label, the window it was popped out from).
static PARENTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Brings up the pop-out that `parent` popped out (asking for a character's
/// pop-out again shows the one already open).
pub fn focus_for(app: &AppHandle, parent: &str) {
    let scanners: Vec<String> = PARENTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, p)| p == parent)
        .map(|(s, _)| s.clone())
        .collect();
    for label in scanners {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

/// Patch 51b: the characters whose pop-outs live in windows of their own (a character's hidden
/// host or own window), for the main page's character tabs. The main page's own pop-out it
/// knows about itself.
pub fn hosted_characters() -> Vec<i64> {
    let mut ids: Vec<i64> = PARENTS
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, parent)| {
            parent
                .strip_prefix("pophost-")
                .or_else(|| parent.strip_prefix("solo-"))
                .and_then(|id| id.parse().ok())
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Tells the main page which characters have a pop-out in a window of their own.
pub fn announce(app: &AppHandle) {
    let ids = serde_json::to_string(&hosted_characters()).unwrap_or_else(|_| "[]".into());
    if let Some(view) = crate::main_view(app) {
        let _ = view.eval(format!(
            "window.dispatchEvent(new CustomEvent('devils-rage-app:popouts', {{ detail: {{ characters: {ids} }} }}));"
        ));
    }
}

fn scanners_of_character(character: &str) -> Vec<String> {
    let host = format!("pophost-{character}");
    let solo = format!("solo-{character}");
    PARENTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, parent)| *parent == host || *parent == solo)
        .map(|(scanner, _)| scanner.clone())
        .collect()
}

/// Closes a character's pop-out (its hidden host window goes with it).
pub fn close_for_character(app: &AppHandle, character: &str) {
    for label in scanners_of_character(character) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.close();
        }
    }
}

/// The page in window `parent` is loading another page (Settings, SeAT, Home, Reload...): the
/// pop-outs it drew would stay on screen as a frozen picture that can't be moved or closed,
/// since the page that drew them is gone. They close instead (their spot is remembered).
///
/// Patch 54: a character's hidden pop-out window that reloads (after a deploy) pops its character
/// out again by itself, so it must stay open: the pop-outs are taken off the list first, which
/// tells their Destroyed handler not to close the window that hosted them.
pub fn close_for_parent(app: &AppHandle, parent: &str) {
    let scanners: Vec<String> = {
        let mut parents = PARENTS.lock().unwrap();
        let mine: Vec<String> = parents.iter().filter(|(_, p)| p == parent).map(|(s, _)| s.clone()).collect();
        parents.retain(|(_, p)| p != parent);
        mine
    };
    for label in scanners {
        if let Some(window) = app.get_webview_window(&label) {
            log::info!("{parent} is loading another page: closing its pop-out {label}");
            let _ = window.close();
        }
    }
}

/// Brings a character's pop-out to the front.
pub fn focus_for_character(app: &AppHandle, character: &str) {
    for label in scanners_of_character(character) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

/// Opens the window the mapper's pop-out draws into (pip.js asked for about:blank).
pub fn open(
    app: &AppHandle,
    marker: &str,
    url: Url,
    features: NewWindowFeatures,
    parent: &str,
) -> Option<WebviewWindow> {
    let label = format!("scanner-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .window_features(features)
        .disable_drag_drop_handler()
        .title("Devil's Rage · scanner")
        .decorations(false)
        .always_on_top(settings::load(app).pop_out_on_top())
        .min_inner_size(260.0, 160.0)
        // Hidden until it's in its saved spot (no flash in the default corner).
        .visible(false)
        .on_new_window(crate::new_window_handler(
            app.clone(),
            marker.to_string(),
            label.clone(),
        ))
        .build()
        .map_err(|e| log::warn!("couldn't open the scanner window: {e}"))
        .ok()?;
    log::info!("scanner pop-out opened ({label}, from {parent})");
    PARENTS
        .lock()
        .unwrap()
        .push((label.clone(), parent.to_string()));

    restore_bounds(app, &window, parent);
    remember_bounds(app, &window, parent);
    let _ = window.show();
    let _ = window.set_focus();
    announce(app);
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
        // The map is on the left, so the window grows and shrinks on its LEFT
        // edge: the signature panel stays where it is on screen and the map
        // folds into it.
        "/resize-by" => {
            let dx = query_number(url, "dx");
            let dy = query_number(url, "dy");
            let scale = window.scale_factor().unwrap_or(1.0);
            if let (Ok(size), Ok(pos)) = (window.inner_size(), window.outer_position()) {
                let width = (size.width as f64 + dx * scale).max(260.0 * scale);
                let height = (size.height as f64 + dy * scale).max(160.0 * scale);
                let moved_by = width - size.width as f64;
                let _ = window.set_size(PhysicalSize::new(width as u32, height as u32));
                let _ = window.set_position(PhysicalPosition::new(
                    pos.x - moved_by.round() as i32,
                    pos.y,
                ));
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
fn restore_bounds(app: &AppHandle, window: &WebviewWindow, parent: &str) {
    let s = settings::load(app);
    let saved = s
        .scanner_bounds_by
        .get(parent)
        .copied()
        .or(if parent == "main" {
            s.scanner_bounds
        } else {
            None
        });
    let Some([x, y, w, h]) = saved else {
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
fn remember_bounds(app: &AppHandle, window: &WebviewWindow, parent: &str) {
    let app = app.clone();
    let me = window.clone();
    let parent = parent.to_string();
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
                s.scanner_bounds_by.insert(parent.clone(), bounds);
                let _ = settings::save(&app, &s);
            }
            let label = me.label().to_string();
            // Still listed = you closed it (not its page reloading, see close_for_parent).
            let closed_by_you = {
                let mut parents = PARENTS.lock().unwrap();
                let listed = parents.iter().any(|(s, _)| *s == label);
                parents.retain(|(s, _)| *s != label);
                listed
            };
            announce(&app);
            // A pop-out's hidden window has no other job: close it too.
            if closed_by_you && parent.starts_with("pophost-") {
                if let Some(host) = app.get_webview_window(&parent) {
                    let _ = host.close();
                }
            }
        }
        _ => {}
    });
}

/// Settings → "Keep on top of EVE": applies to scanner windows already open too.
pub fn set_on_top(app: &AppHandle, on_top: bool) {
    for (label, window) in app.webview_windows() {
        if label.starts_with("scanner-") || label.starts_with("solo-") {
            let _ = window.set_always_on_top(on_top);
        }
    }
}
