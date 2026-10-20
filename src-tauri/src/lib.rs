use tauri::{
    webview::{Color, NewWindowResponse, WebviewBuilder},
    window::WindowBuilder,
    AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl, Window,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

mod appconfig;
mod bar;
mod chatlog;
mod clipboard;
mod commands;
mod location;
mod mainwindow;
mod parsec;
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

/// Our sites and the logins they use stay in the app; EVE's login can hop between
/// eveonline.com addresses, and Discord's between discord.com / discordapp.com.
fn in_app_host(host: &str) -> bool {
    IN_APP_HOSTS.contains(&host)
        || host.ends_with(".eveonline.com")
        || host == "discordapp.com"
        || host.ends_with(".discord.com")
}

/// Where the main page may go. The app's own pages, our sites and logins stay in the
/// app; a plain link to anywhere else (no "new tab") opens in the normal browser
/// instead of replacing the mapper in the app, where there's no Back button.
fn main_view_may_go(app: &AppHandle, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        // tauri://, about:blank, data:, blob: ... the app's own and the page's own.
        _ => return true,
    }
    let host = url.host_str().unwrap_or_default();
    if host == "tauri.localhost" || host == "localhost" || in_app_host(host) {
        return true;
    }
    open_in_browser(app, "main", url.as_str());
    false
}

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
        "accounts" => return format!("{SEAT_URL}/devilsrage/your-accounts").parse().ok(),
        "pi" => return format!("{SEAT_URL}/devilsrage/pi").parse().ok(),
        _ => return None,
    };
    last.filter(|page| mainwindow::reopenable(page))
        .and_then(|page| page.parse::<Url>().ok())
        .filter(|url| url.host_str() == Some(host_of(root)))
        .or_else(|| root.parse().ok())
}

/// The main window (label "app"): the slim bar on top, the page below.
pub const MAIN_WINDOW: &str = "app";
/// Height of the slim bar, in logical pixels.
const BAR_HEIGHT: f64 = 30.0;

/// The page part of the main window (start screen, mapper, SeAT, settings).
pub fn main_view(app: &AppHandle) -> Option<Webview> {
    app.get_webview("main")
}

/// The window a page (webview) is in: the main window for "main" and "bar",
/// its own window for pop-outs.
pub fn window_of(app: &AppHandle, label: &str) -> Option<Window> {
    app.get_webview(label).map(|view| view.window())
}

fn go(app: &AppHandle, url: Url) {
    if let Some(view) = main_view(app) {
        let _ = view.navigate(url);
        let _ = view.set_focus();
    }
}

/// Lays the bar and the page out in the main window (after every resize).
fn layout(window: &Window) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let Ok(size) = window.inner_size() else { return };
    let size = size.to_logical::<f64>(scale);
    if size.width <= 0.0 || size.height <= 0.0 {
        return;
    }
    let app = window.app_handle();
    if let Some(bar) = app.get_webview("bar") {
        let _ = bar.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = bar.set_size(LogicalSize::new(size.width, BAR_HEIGHT));
    }
    if let Some(page) = app.get_webview("main") {
        let _ = page.set_position(LogicalPosition::new(0.0, BAR_HEIGHT));
        let _ = page.set_size(LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)));
    }
}

/// What the slim bar's buttons do (bar.html calls the bar_action command).
pub fn bar_action(app: &AppHandle, action: &str) {
    let window = app.get_window(MAIN_WINDOW);
    match action {
        "home" => go(app, home_url()),
        "mapper" | "seat" | "accounts" | "pi" => {
            if let Some(url) = site_url(app, action) {
                go(app, url);
            }
        }
        "settings" => go(app, app_url("settings.html")),
        "admin" => {
            if let Ok(url) = format!("{MAPPER_URL}/desktop-app/admin").parse() {
                go(app, url);
            }
        }
        "reload" => {
            if let Some(view) = main_view(app) {
                let _ = view.eval("location.reload()");
            }
        }
        #[cfg(desktop)]
        "updates" => updates::check_now(app.clone()),
        "minimize" => {
            if let Some(w) = window {
                let _ = w.minimize();
            }
        }
        "maximize" => {
            if let Some(w) = window {
                if w.is_maximized().unwrap_or(false) {
                    let _ = w.unmaximize();
                } else {
                    let _ = w.maximize();
                }
            }
        }
        "close" => {
            if let Some(w) = window {
                let _ = w.close();
            }
        }
        "drag" => {
            if let Some(w) = window {
                let _ = w.start_dragging();
            }
        }
        _ => {}
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
            commands::open_parsec,
            commands::read_log,
            commands::open_log,
            commands::open_devtools,
            commands::open_admin,
            commands::reset_app,
            bar::bar_action,
            bar::bar_state,
        ])
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
                "window.__DEVILS_RAGE_APP__ = Object.freeze({{ version: {version:?}, channel: {:?}, parsec: true }});\n{}\n{}\n{}",
                channel.as_str(),
                SHORTCUTS,
                RECONNECT,
                include_str!("pip.js")
            );

            // The main window: no Windows title bar; our slim dark bar on top (bar.html,
            // the app's own page) and the page below it (start screen, mapper, SeAT).
            let window = WindowBuilder::new(app, MAIN_WINDOW)
                .title(title)
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .center()
                .decorations(false)
                .shadow(true)
                // Hidden until it's back in its saved place (no jump on screen).
                .visible(false)
                .build()?;
            let scale = window.scale_factor().unwrap_or(1.0);
            let inner = window.inner_size()?.to_logical::<f64>(scale);
            window.add_child(
                WebviewBuilder::new("bar", WebviewUrl::App("bar.html".into()))
                    .background_color(Color(10, 10, 12, 255)),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(inner.width, BAR_HEIGHT),
            )?;
            window.add_child(
                WebviewBuilder::new("main", WebviewUrl::App("index.html".into()))
                    .background_color(Color(13, 13, 16, 255))
                    .initialization_script(marker.clone())
                    .on_navigation({
                        let app = handle.clone();
                        move |url| main_view_may_go(&app, url)
                    })
                    .on_new_window(new_window_handler(handle.clone(), marker, "main".into())),
                LogicalPosition::new(0.0, BAR_HEIGHT),
                LogicalSize::new(inner.width, (inner.height - BAR_HEIGHT).max(0.0)),
            )?;
            {
                let me = window.clone();
                window.on_window_event(move |event| {
                    if matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                        layout(&me);
                    }
                });
            }

            mainwindow::restore(&handle, &window);
            mainwindow::remember(&handle, &window);
            layout(&window);
            let _ = window.show();
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

/// Patch 48: a mapper or SeAT page that loaded as the proxy's error ("404 page not found",
/// 502/503/504) while the server restarts (deploys, EVE downtime, reboots) says it's reconnecting
/// and reloads by itself once the page answers again. Pages that loaded fine run nothing.
const RECONNECT: &str = r#"
(() => {
    if (!['devilsrage.duckdns.org', 'seatdevilsrage.duckdns.org'].includes(location.host)) return;
    const check = () => {
        const body = document.body;
        if (!body || document.querySelector('#app, script[data-page]')) return;
        const text = (body.innerText || '').trim().toLowerCase();
        const down = text === '404 page not found' || /^(50[234] )?(bad gateway|service unavailable|gateway time-?out)/.test(text);
        if (!down) return;
        const started = Date.now();
        document.title = 'Reconnecting…';
        body.innerHTML = '<div style="position:fixed;inset:0;display:flex;align-items:center;justify-content:center;background:#0a0a0a;color:#bbb;font:15px system-ui,sans-serif">The server is restarting. This page reloads by itself when it is back…</div>';
        const again = () => setTimeout(retry, Date.now() - started < 120000 ? 5000 : 15000);
        const retry = () => fetch(location.href, { cache: 'no-store', credentials: 'same-origin' })
            .then((r) => (r.ok ? location.reload() : again()), again);
        again();
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', check);
    else check();
})();
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
                // Patch 50: "Connect to my EVE PC" on the PI screen (a SeAT page).
                "/parsec" if from_seat(&app, &me) => parsec::connect(&app),
                "/parsec" => log::warn!("ignored /parsec from a non-SeAT window ({me})"),
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

        let ours = url.host_str().is_some_and(in_app_host);
        if !ours {
            open_in_browser(&app, &me, url.as_str());
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

/// A link to another site (zKillboard, Dotlan, ...): the normal browser opens it. Off the
/// window's own thread, and logged, so a link that doesn't open shows up in the app log.
fn open_in_browser(app: &AppHandle, from: &str, url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log::warn!("not opening {url} from {from}: not a web address");
        return;
    }
    log::info!("opening in the browser (from {from}): {url}");
    let app = app.clone();
    let url = url.to_string();
    std::thread::spawn(move || {
        if let Err(err) = app.opener().open_url(&url, None::<&str>) {
            log::warn!("couldn't open {url} in the browser: {err}");
        }
    });
}

/// Whether window `me` shows a mapper page (only those may write the clipboard or ask for attention).
fn from_mapper(app: &AppHandle, me: &str) -> bool {
    app.get_webview(me)
        .and_then(|w| w.url().ok())
        .is_some_and(|u| u.host_str() == Some(host_of(MAPPER_URL)))
}

/// Whether window `me` shows a SeAT page (the PI screen's Parsec button).
fn from_seat(app: &AppHandle, me: &str) -> bool {
    app.get_webview(me)
        .and_then(|w| w.url().ok())
        .is_some_and(|u| u.host_str() == Some(host_of(SEAT_URL)))
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
            let Some(window) = window_of(app, me) else { return };
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

