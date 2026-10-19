//! The main window remembers where it was (size, position, maximised) and which
//! mapper / SeAT page you were on, so the next start goes straight back there.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Url, WebviewWindow, WindowEvent};

use crate::settings;

/// Puts the window back where it was, if that spot is still on a screen.
pub fn restore(app: &AppHandle, window: &WebviewWindow) {
    let s = settings::load(app);
    if let Some([x, y, w, h]) = s.main_bounds {
        if on_a_screen(window, x, y) {
            let _ = window.set_size(PhysicalSize::new(w.max(800) as u32, h.max(500) as u32));
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
    }
    if s.main_maximized {
        let _ = window.maximize();
    }
}

fn on_a_screen(window: &WebviewWindow, x: i32, y: i32) -> bool {
    window
        .available_monitors()
        .map(|monitors| {
            monitors.iter().any(|m| {
                let p = m.position();
                let s = m.size();
                let (cx, cy) = (x + 60, y + 10);
                cx >= p.x && cy >= p.y && cx < p.x + s.width as i32 && cy < p.y + s.height as i32
            })
        })
        .unwrap_or(false)
}

/// Follows moves and resizes; saves when the window closes. While maximised it
/// keeps the size from before, so un-maximising next time goes back to it.
pub fn remember(app: &AppHandle, window: &WebviewWindow) {
    let app = app.clone();
    let me = window.clone();
    let last = Mutex::new((None::<[i32; 4]>, false));
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            let maximized = me.is_maximized().unwrap_or(false);
            let minimized = me.is_minimized().unwrap_or(false);
            if minimized {
                return;
            }
            let mut state = last.lock().unwrap();
            state.1 = maximized;
            if !maximized {
                if let (Ok(p), Ok(s)) = (me.outer_position(), me.inner_size()) {
                    if s.width > 0 && s.height > 0 {
                        state.0 = Some([p.x, p.y, s.width as i32, s.height as i32]);
                    }
                }
            }
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            let state = last.lock().unwrap();
            let mut s = settings::load(&app);
            if state.0.is_some() {
                s.main_bounds = state.0;
            }
            s.main_maximized = state.1;
            let _ = settings::save(&app, &s);
        }
        _ => {}
    });
}

/// Pages that shouldn't be reopened next time (logins, logouts, callbacks).
fn worth_remembering(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    ![
        "login", "logout", "callback", "auth", "sso", "register", "password",
    ]
    .iter()
    .any(|word| path.contains(word))
}

/// Strips this-window-only bits the mapper adds (?as=, ?solo=).
fn clean(url: &Url) -> String {
    let mut url = url.clone();
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "as" && k != "solo")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    url.set_fragment(None);
    url.to_string()
}

/// The mapper and SeAT change pages without reloading (Inertia), so the app
/// looks at the main window's address every few seconds and keeps the latest
/// mapper / SeAT page in settings.
pub fn follow_pages(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut seen = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(4)).await;
            let Some(win) = app.get_webview_window("main") else {
                continue;
            };
            let Ok(url) = win.url() else { continue };
            if url.as_str() == seen {
                continue;
            }
            seen = url.to_string();
            let host = url.host_str().unwrap_or_default();
            if !worth_remembering(&url) {
                continue;
            }
            let mut s = settings::load(&app);
            let page = Some(clean(&url));
            if host == crate::host_of(crate::MAPPER_URL) && s.last_mapper_page != page {
                s.last_mapper_page = page;
            } else if host == crate::host_of(crate::SEAT_URL) && s.last_seat_page != page {
                s.last_seat_page = page;
            } else {
                continue;
            }
            let _ = settings::save(&app, &s);
        }
    });
}
