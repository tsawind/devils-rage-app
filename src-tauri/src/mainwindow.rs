//! The main window remembers where it was (size, position, maximised) and which
//! mapper / SeAT page you were on, so the next start goes straight back there.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Url, Window, WindowEvent};

use crate::settings;

/// Puts the window back where it was, if that spot is still on a screen.
pub fn restore(app: &AppHandle, window: &Window) {
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

fn on_a_screen(window: &Window, x: i32, y: i32) -> bool {
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
pub fn remember(app: &AppHandle, window: &Window) {
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

/// A remembered page that's still fine to reopen (older builds could remember
/// a bad one).
pub fn reopenable(page: &str) -> bool {
    page.parse::<Url>().is_ok_and(|url| worth_remembering(&url))
}

/// Pages that shouldn't be reopened next time: logins, logouts, callbacks, the
/// desktop app's own pages (admin), and /app/ (the server's websocket path).
fn worth_remembering(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    if path.starts_with("/app/") || path.starts_with("/desktop-app/") {
        return false;
    }
    // The mapper: only real pages are worth reopening (your map, the map list, settings…),
    // never an address the mapper only answers with data, like /map-solarsystems: a page
    // left there by a failed request showed "405 Method Not Allowed" on every start.
    if url.host_str() == Some(crate::host_of(crate::MAPPER_URL)) {
        let segments: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        let page = match segments.as_slice() {
            [] => true,
            ["maps"] | ["maps", _] => true,
            [one] => ["settings", "documentation"].contains(one),
            _ => false,
        };
        if !page {
            return false;
        }
    }
    // Your accounts and PI have their own tabs: SeAT reopens your last real SeAT page, not these
    // (nor the login steps it runs).
    if path.starts_with("/devilsrage/your-accounts")
        || path.starts_with("/devilsrage/accounts/login")
        || path.starts_with("/devilsrage/pi")
    {
        return false;
    }
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
        // What each of the main window's two pages (mapper, other pages) showed last time.
        let mut seen = [String::new(), String::new()];
        loop {
            tokio::time::sleep(Duration::from_secs(4)).await;
            for (i, label) in [crate::MAPPER_VIEW, crate::PAGES_VIEW].into_iter().enumerate() {
                let Some(win) = app.get_webview(label) else {
                    continue;
                };
                let Ok(url) = win.url() else { continue };
                if url.as_str() == seen[i] {
                    continue;
                }
                seen[i] = url.to_string();
                note_page(&app, &url);
            }
        }
    });
}

/// A page in the main window changed: a mapper page gets everyone's location; real
/// mapper / SeAT pages are remembered for next time.
fn note_page(app: &AppHandle, url: &Url) {
    let host = url.host_str().unwrap_or_default();
    if host == crate::host_of(crate::MAPPER_URL) {
        // A mapper page (re)opened: once it's listening, tell it where everyone is.
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            crate::location::resend_all(&app);
            crate::scanner::announce(&app);
        });
    }
    if !worth_remembering(url) {
        return;
    }
    let mut s = settings::load(app);
    let page = Some(clean(url));
    if host == crate::host_of(crate::MAPPER_URL) && s.last_mapper_page != page {
        s.last_mapper_page = page;
    } else if host == crate::host_of(crate::SEAT_URL) && s.last_seat_page != page {
        s.last_seat_page = page;
    } else {
        return;
    }
    let _ = settings::save(app, &s);
}
