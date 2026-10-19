use serde::Serialize;
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

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

            let opener = handle.clone();
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(title)
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .center()
                // Lets the mapper know it runs inside the app (for the native
                // pop-out windows coming next). Read-only marker, no powers.
                .initialization_script(format!(
                    "window.__DEVILS_RAGE_APP__ = Object.freeze({{ version: {version:?}, channel: {:?} }});",
                    channel.as_str()
                ))
                // window.open / target=_blank: our sites and logins stay in the
                // app, everything else (zKillboard, Dotlan, ...) opens in the
                // normal browser.
                .on_new_window(move |url, _features| {
                    let ours = url
                        .host_str()
                        .is_some_and(|h| IN_APP_HOSTS.contains(&h));
                    if ours {
                        NewWindowResponse::Allow
                    } else {
                        let _ = opener.opener().open_url(url.as_str(), None::<&str>);
                        NewWindowResponse::Deny
                    }
                });

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

/// The slim menu bar above the page: always there, never covers the mapper.
#[cfg(desktop)]
fn build_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem};
    let home = MenuItem::with_id(app, "home", "Home", true, Some("CmdOrCtrl+Shift+H"))?;
    let mapper = MenuItem::with_id(app, "mapper", "Mapper", true, None::<&str>)?;
    let seat = MenuItem::with_id(app, "seat", "SeAT", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", true, None::<&str>)?;
    Menu::with_items(app, &[&home, &mapper, &seat, &reload, &updates])
}
