use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

mod appconfig;
mod chatlog;
mod clipboard;
mod commands;
mod location;
mod mainwindow;
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

/// Name of the log file in the app's log folder
/// (Windows: %LOCALAPPDATA%\com.tsawind.devilsrage\logs\devils-rage.log).
pub const LOG_NAME: &str = "devils-rage";

/// A page bundled with the app (ui/...).
fn app_url(page: &str) -> Url {
    #[cfg(windows)]
    let base = "http://tauri.localhost/";
    #[cfg(not(windows))]
    let base = "tauri://localhost/";
    format!("{base}{page}").parse().expect("valid app url")
}

/// The start screen (ui/index.html).
fn home_url() -> Url {
    app_url("index.html")
}

fn host_of(url: &str) -> &str {
    url.trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(url)
}

/// Where "Mapper" / "SeAT" go: the page you were last on there (your map),
/// or the site's front page the first time.
fn site_url(app: &AppHandle, site: &str) -> Option<Url> {
    let s = settings::load(app);
    let (root, last) = match site {
        "mapper" => (MAPPER_URL, s.last_mapper_page),
        "seat" => (SEAT_URL, s.last_seat_page),
        _ => return None,
    };
    last.filter(|page| mainwindow::reopenable(page))
        .and_then(|page| page.parse::<Url>().ok())
        .filter(|url| url.host_str() == Some(host_of(root)))
        .or_else(|| root.parse().ok())
}

fn go(app: &AppHandle, url: Url) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.navigate(url);
        let _ = win.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .max_file_size(2_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some(LOG_NAME.into()),
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ));
    }

    builder
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::open_site,
            commands::check_updates,
            commands::get_settings,
            commands::set_setting,
            commands::reset_pop_out_position,
            commands::read_log,
            commands::open_log,
            commands::open_devtools,
            commands::open_admin,
            commands::reset_app,
        ])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "home" => go(app, home_url()),
            "mapper" | "seat" => {
                if let Some(url) = site_url(app, event.id().as_ref()) {
                    go(app, url);
                }
            }
            "settings" => go(app, app_url("settings.html")),
            "admin" => go(app, format!("{MAPPER_URL}/desktop-app/admin").parse().unwrap()),
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
            log::info!("Devil's Rage {version} starting");
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
                // Hidden until it's back in its saved place (no jump on screen).
                .visible(false)
                .initialization_script(marker.clone())
                .on_new_window(new_window_handler(handle.clone(), marker, "main".into()));

            #[cfg(desktop)]
            let builder = builder.menu(build_menu(&handle)?);

            let main = builder.build()?;
            mainwindow::restore(&handle, &main);
            mainwindow::remember(&handle, &main);
            let _ = main.show();
            mainwindow::follow_pages(handle.clone());
            location::start(handle.clone());
            clipboard::start(handle.clone());
            appconfig::start(handle.clone());

            // "Skip this screen" ticked: go straight to the chosen site (your last map).
            if let Some(url) = s.start_page.as_deref().and_then(|site| site_url(&handle, site)) {
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
            match url.path() {
                "/home" => go(&app, home_url()),
                "/clipboard" | "/attention" if from_mapper(&app, &me) => page_signal(&app, &me, &url),
                "/clipboard" | "/attention" => log::warn!("ignored {} from a non-mapper window ({me})", url.path()),
                _ => scanner::signal(&app, &me, &url),
            }
            return NewWindowResponse::Deny;
        }

        // Directors switched the app-window pop-out off: a plain window instead.
        if url.as_str() == "about:blank" && !appconfig::feature_on("scanner_pop_out") {
            return NewWindowResponse::Allow;
        }

        if url.as_str() == "about:blank" {
            return match scanner::open(&app, &marker, url, features, &me) {
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
        // ?popout=1: the mapper opened this character's window only to host its own
        // pop-out (a second, third... pop-out). The window stays hidden; the pop-out
        // is what you see, and closing the pop-out closes it.
        let host = url.query_pairs().any(|(k, v)| k == "popout" && v == "1");
        let label = if host {
            format!("pophost-{character}")
        } else {
            format!("solo-{character}")
        };

        if let Some(existing) = app.get_webview_window(&label) {
            if host {
                scanner::focus_for(&app, &label);
            } else {
                let _ = existing.navigate(url);
                let _ = existing.set_focus();
            }
            return NewWindowResponse::Deny;
        }

        match WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
            .window_features(features)
            .title("Devil's Rage")
            .always_on_top(!host)
            .visible(!host)
            .skip_taskbar(host)
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

/// Whether window `me` shows a mapper page (only those may write the clipboard or ask for attention).
fn from_mapper(app: &AppHandle, me: &str) -> bool {
    app.get_webview_window(me)
        .and_then(|w| w.url().ok())
        .is_some_and(|u| u.host_str() == Some(host_of(MAPPER_URL)))
}

/// Windows the mapper put on top of EVE while a jump prompt waits (Patch 47), to put back after.
static RAISED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// A mapper page asking for something only the app can do (Patch 47):
/// - /clipboard?text=... : put the text on the clipboard (works while EVE has focus);
/// - /attention?on=1|0   : keep this window on top of EVE, without taking focus, while the
///   jump prompt waits for you (no scanner pop-out open to answer it in), then put it back.
fn page_signal(app: &AppHandle, me: &str, url: &Url) {
    let param = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    match url.path() {
        "/clipboard" => {
            let text = param("text").unwrap_or_default();
            if text.is_empty() || text.len() > 4000 {
                return;
            }
            match clipboard::write(&text) {
                Ok(()) => log::info!("copied for the mapper: {}", text.trim()),
                Err(err) => log::warn!("couldn't write the clipboard: {err}"),
            }
        }
        "/attention" => {
            let Some(window) = app.get_webview_window(me) else { return };
            let on = param("on").as_deref() == Some("1") && appconfig::feature_on("jump_attention");
            let mut raised = RAISED.lock().unwrap();
            if on {
                if raised.iter().any(|l| l == me) {
                    return;
                }
                // Already pinned (a character's own window): nothing to do or undo.
                if window.is_always_on_top().unwrap_or(false) {
                    return;
                }
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_always_on_top(true);
                raised.push(me.to_string());
                log::info!("{me} kept on top of EVE while the jump prompt waits");
            } else if let Some(i) = raised.iter().position(|l| l == me) {
                raised.remove(i);
                let _ = window.set_always_on_top(false);
            }
        }
        _ => {}
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
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", true, None::<&str>)?;
    Menu::with_items(app, &[&home, &mapper, &seat, &settings, &reload, &updates])
}
