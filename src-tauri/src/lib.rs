use serde::Serialize;
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

mod scanner;
mod settings;
#[cfg(desktop)]
mod updates;

/// Our sites. Phase 0 step 3 makes these settings (live / test servers).
pub const MAPPER_URL: &str = "https://devilsrage.duckdns.org";
pub const SEAT_URL: &str = "https://seatdevilsrage.duckdns.org";

/// Sites that may open new windows inside the app (logins included).
/// Anything else a page opens in a new window goes to the normal browser.
const IN_APP_HOSTS: &[&str] = &[
    "devilsrage.duckdns.org",
    "seatdevilsrage.duckdns.org",
    "login.eveonline.com",
    "discord.com",
];

/// The start screen bundled with the app (ui/index.html).
fn home_url() -> Url {
    #[cfg(windows)]
    let url = "http://tauri.localhost/index.html";
    #[cfg(not(windows))]
    let url = "tauri://localhost/index.html";
    url.parse().expect("valid home url")
}

fn site_url(site: &str) -> Option<Url> {
    match site {
        "mapper" => MAPPER_URL.parse().ok(),
        "seat" => SEAT_URL.parse().ok(),
        _ => None,
    }
}

fn go(app: &AppHandle, url: Url) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.navigate(url);
        let _ = win.set_focus();
    }
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    channel: &'static str,
    start_page: Option<String>,
}

// ---- Commands for the start screen. Only the app's own pages can call these:
// ---- remote pages (mapper, SeAT) get no IPC access unless a capability says so.

#[tauri::command]
fn app_info(app: AppHandle) -> AppInfo {
    let s = settings::load(&app);
    AppInfo {
        version: app.package_info().version.to_string(),
        channel: s.channel(&app).as_str(),
        start_page: s.start_page,
    }
}

#[tauri::command]
fn open_site(app: AppHandle, site: String, remember: bool) -> Result<(), String> {
    let url = site_url(&site).ok_or("unknown site")?;
    let mut s = settings::load(&app);
    let wanted = remember.then(|| site.clone());
    if s.start_page != wanted {
        s.start_page = wanted;
        settings::save(&app, &s)?;
    }
    go(&app, url);
    Ok(())
}

#[tauri::command]
fn check_updates(app: AppHandle) {
    #[cfg(desktop)]
    updates::check_now(app);
    #[cfg(not(desktop))]
    let _ = app;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .invoke_handler(tauri::generate_handler![app_info, open_site, check_updates])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "home" => go(app, home_url()),
            "mapper" => go(app, MAPPER_URL.parse().unwrap()),
            "seat" => go(app, SEAT_URL.parse().unwrap()),
            "reload" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.eval("location.reload()");
                }
            }
            #[cfg(desktop)]
            "updates" => updates::check_now(app.clone()),
            _ => {}
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let s = settings::load(&handle);
            let version = app.package_info().version.to_string();
            let channel = s.channel(&handle);

            // Stable builds show just the name; test builds show which build it
            // is, so it's obvious when an update has landed.
            let title = match channel {
                settings::Channel::Stable => "Devil's Rage".to_string(),
                _ => format!("Devil's Rage — {version} ({})", channel.as_str()),
            };

            // Lets the mapper know it runs inside the app (read-only marker, no
            // powers), and gives it a stand-in for Chrome's picture-in-picture so
            // its scanner pop-out opens as a pinned app window (see pip.js).
            let marker = format!(
                "window.__DEVILS_RAGE_APP__ = Object.freeze({{ version: {version:?}, channel: {:?} }});\n{}\n{}",
                channel.as_str(),
                SHORTCUTS,
                include_str!("pip.js")
            );

            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(title)
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .center()
                .initialization_script(marker.clone())
                .on_new_window(new_window_handler(handle.clone(), marker, "main".into()));

            #[cfg(desktop)]
            let builder = builder.menu(build_menu(&handle)?);

            builder.build()?;

            // "Skip this screen" ticked: go straight to the chosen site.
            if let Some(url) = s.start_page.as_deref().and_then(site_url) {
                go(&handle, url);
            }

            #[cfg(desktop)]
            updates::start(handle);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Devil's Rage app");
}

/// Fake address pages use to ask the app for things (see pip.js).
const SIGNAL_HOST: &str = "dr-app.invalid";

/// Keyboard shortcuts in every app page: Ctrl+Shift+H = back to the start screen.
const SHORTCUTS: &str = r#"
window.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && !e.altKey && (e.key === 'H' || e.key === 'h')) {
        e.preventDefault();
        try { window.open('https://dr-app.invalid/home', '_blank'); } catch (err) {}
    }
}, true);
"#;

/// window.open / target=_blank from a page (`me` = the window asking):
/// - `about:blank` from the mapper's pop-out (pip.js) becomes the scanner
///   window: pinned on top, no title bar, where you left it last time;
/// - https://dr-app.invalid/... = the scanner asking to drag, close or resize;
/// - the mapper's own fallback popup (?solo=1) becomes a pinned window too;
/// - other pages on our sites and logins open as normal app windows;
/// - everything else (zKillboard, Dotlan, ...) opens in the normal browser.
fn new_window_handler(
    app: AppHandle,
    marker: String,
    me: String,
) -> impl Fn(Url, tauri::webview::NewWindowFeatures) -> NewWindowResponse<tauri::Wry> + Send + 'static
{
    move |url, features| {
        if url.host_str() == Some(SIGNAL_HOST) {
            if url.path() == "/home" {
                go(&app, home_url());
            } else {
                scanner::signal(&app, &me, &url);
            }
            return NewWindowResponse::Deny;
        }

        if url.as_str() == "about:blank" {
            return match scanner::open(&app, &marker, url, features) {
                Some(window) => NewWindowResponse::Create { window },
                None => NewWindowResponse::Deny,
            };
        }

        let ours = url.host_str().is_some_and(|h| IN_APP_HOSTS.contains(&h));
        if !ours {
            let _ = app.opener().open_url(url.as_str(), None::<&str>);
            return NewWindowResponse::Deny;
        }

        let solo = url.query_pairs().any(|(k, v)| k == "solo" && v == "1");
        if !solo {
            return NewWindowResponse::Allow;
        }

        let character: String = url
            .query_pairs()
            .find(|(k, _)| k == "as")
            .map(|(_, v)| v.chars().filter(char::is_ascii_digit).collect())
            .unwrap_or_default();
        let label = format!("solo-{character}");

        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.navigate(url);
            let _ = existing.set_focus();
            return NewWindowResponse::Deny;
        }

        match WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
            .window_features(features)
            .title("Devil's Rage")
            .always_on_top(true)
            .initialization_script(marker.clone())
            .on_new_window(new_window_handler(
                app.clone(),
                marker.clone(),
                label.clone(),
            ))
            .build()
        {
            Ok(window) => NewWindowResponse::Create { window },
            Err(_) => NewWindowResponse::Allow,
        }
    }
}

/// The slim menu bar above the page: always there, never covers the mapper.
#[cfg(desktop)]
fn build_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem};
    // No shortcuts on the items themselves: Windows prints them next to the
    // name in a top-level menu bar ("HomeCtrl+Shift+H"). Ctrl+Shift+H for Home
    // comes from the page instead (SHORTCUTS); Ctrl+R / F5 reload on their own.
    let home = MenuItem::with_id(app, "home", "Home", true, None::<&str>)?;
    let mapper = MenuItem::with_id(app, "mapper", "Mapper", true, None::<&str>)?;
    let seat = MenuItem::with_id(app, "seat", "SeAT", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", true, None::<&str>)?;
    Menu::with_items(app, &[&home, &mapper, &seat, &reload, &updates])
}
