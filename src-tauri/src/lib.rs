use tauri::{
    webview::{Color, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    window::WindowBuilder,
    AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl, Window,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

mod appconfig;
mod audiodetect;
mod bar;
mod chatlog;
mod clipboard;
mod clientaudio;
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

/// Whether `url` belongs in the mapper's own page ("main"): any mapper page except the
/// app's admin page, which opens with the app's other pages.
fn is_mapper_page(url: &Url) -> bool {
    url.host_str() == Some(host_of(MAPPER_URL)) && !url.path().starts_with("/desktop-app/")
}

/// Where a page in the main window may go (`me` = "main" or "pages"). The mapper stays in
/// "main" and everything else of ours in "pages": a link from one to the other goes to the
/// other page (shown), so the mapper is never unloaded by a trip to SeAT or Settings. Logins
/// stay where they started. A plain link to anywhere else opens in the normal browser
/// instead of replacing the page in the app, where there's no Back button.
fn main_view_may_go(app: &AppHandle, me: &str, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        // tauri://, about:blank, data:, blob: ... the app's own and the page's own.
        _ => return true,
    }
    let host = url.host_str().unwrap_or_default();
    let ours = host == "tauri.localhost" || host == "localhost" || in_app_host(host);
    if !ours {
        open_in_browser(app, me, url.as_str());
        return false;
    }
    let belongs = if is_mapper_page(url) {
        MAPPER_VIEW
    } else if host == host_of(SEAT_URL) || host == "tauri.localhost" || host == "localhost" || host == host_of(MAPPER_URL) {
        PAGES_VIEW
    } else {
        // EVE / Discord logins: wherever they were started.
        return true;
    };
    if belongs == me {
        return true;
    }
    let app = app.clone();
    let url = url.clone();
    tauri::async_runtime::spawn(async move { go(&app, url) });
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

/// The main window holds two pages under the bar, one shown at a time like browser tabs:
/// the mapper ("main", only ever a mapper page, so it stays loaded while you're elsewhere and
/// its pop-outs keep working) and the app's other pages ("pages": start screen, settings,
/// SeAT, Your accounts, PI, admin).
pub const MAPPER_VIEW: &str = "main";
pub const PAGES_VIEW: &str = "pages";

/// Which of the two is showing.
static SHOWN: std::sync::Mutex<&'static str> = std::sync::Mutex::new(PAGES_VIEW);

/// The mapper's page in the main window (live location, scans and pop-outs go here).
pub fn main_view(app: &AppHandle) -> Option<Webview> {
    app.get_webview(MAPPER_VIEW)
}

/// The page you're looking at in the main window.
pub fn shown_view(app: &AppHandle) -> Option<Webview> {
    app.get_webview(*SHOWN.lock().unwrap())
}

/// Shows one of the main window's two pages and hides the other.
fn show_view(app: &AppHandle, label: &'static str) {
    *SHOWN.lock().unwrap() = label;
    let other = if label == MAPPER_VIEW { PAGES_VIEW } else { MAPPER_VIEW };
    if let Some(view) = app.get_webview(label) {
        let _ = view.show();
        let _ = view.set_focus();
    }
    if let Some(view) = app.get_webview(other) {
        let _ = view.hide();
    }
}

/// The window a page (webview) is in: the main window for "main" and "bar",
/// its own window for pop-outs.
pub fn window_of(app: &AppHandle, label: &str) -> Option<Window> {
    app.get_webview(label).map(|view| view.window())
}

/// Opens `url` in the main window: mapper pages in the mapper's page, the rest in the other.
pub(crate) fn go(app: &AppHandle, url: Url) {
    let label = if is_mapper_page(&url) { MAPPER_VIEW } else { PAGES_VIEW };
    if let Some(view) = app.get_webview(label) {
        let _ = view.navigate(url);
    }
    show_view(app, label);
}

/// The Mapper tab: the mapper as you left it (no reload); loads it if it isn't there yet.
pub(crate) fn show_mapper(app: &AppHandle) {
    let loaded = main_view(app)
        .and_then(|view| view.url().ok())
        .is_some_and(|url| url.host_str() == Some(host_of(MAPPER_URL)));
    if loaded {
        show_view(app, MAPPER_VIEW);
    } else if let Some(url) = site_url(app, "mapper") {
        go(app, url);
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
    for label in [MAPPER_VIEW, PAGES_VIEW] {
        if let Some(page) = app.get_webview(label) {
            let _ = page.set_position(LogicalPosition::new(0.0, BAR_HEIGHT));
            let _ = page.set_size(LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)));
        }
    }
}

/// What the slim bar's buttons do (bar.html calls the bar_action command).
pub fn bar_action(app: &AppHandle, action: &str) {
    let window = app.get_window(MAIN_WINDOW);
    match action {
        "home" => go(app, home_url()),
        "mapper" => show_mapper(app),
        "seat" | "accounts" | "pi" => {
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
            if let Some(view) = shown_view(app) {
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
                "window.__DEVILS_RAGE_APP__ = Object.freeze({{ version: {version:?}, channel: {:?}, parsec: true, cameras: 3, clientAudio: true }});\n{}\n{}\n{}",
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
            // The mapper's page starts loading straight away (hidden until you open it), so
            // it's ready, and live location and scans have somewhere to go.
            let mapper_start = site_url(&handle, "mapper").map(WebviewUrl::External).unwrap_or(WebviewUrl::App("index.html".into()));
            for (label, start) in [(MAPPER_VIEW, mapper_start), (PAGES_VIEW, WebviewUrl::App("index.html".into()))] {
                let view = window.add_child(
                    WebviewBuilder::new(label, start)
                        .background_color(Color(13, 13, 16, 255))
                        .initialization_script(marker.clone())
                        // Windows' file drag-and-drop handler swallows the page's own drag and drop
                        // (dragging tabs, accounts); we never take dropped files.
                        .disable_drag_drop_handler()
                        .on_navigation({
                            let app = handle.clone();
                            move |url| main_view_may_go(&app, label, url)
                        })
                        .on_page_load({
                            let app = handle.clone();
                            move |_, payload| {
                                if matches!(payload.event(), PageLoadEvent::Started) {
                                    scanner::close_for_parent(&app, label);
                                }
                            }
                        })
                        .on_new_window(new_window_handler(handle.clone(), marker.clone(), label.into())),
                    LogicalPosition::new(0.0, BAR_HEIGHT),
                    LogicalSize::new(inner.width, (inner.height - BAR_HEIGHT).max(0.0)),
                )?;
                if label == MAPPER_VIEW {
                    let _ = view.hide();
                }
            }
            {
                let me = window.clone();
                let app = handle.clone();
                window.on_window_event(move |event| match event {
                    WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => layout(&me),
                    // Closing the main window closes the app: pop-outs and character windows
                    // are drawn by its page, so they'd stay open but dead.
                    WindowEvent::Destroyed => {
                        log::info!("main window closed: closing the app");
                        app.exit(0);
                    }
                    _ => {}
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
            match s.start_page.as_deref() {
                Some("mapper") => show_view(&handle, MAPPER_VIEW),
                Some(site) => {
                    if let Some(url) = site_url(&handle, site) {
                        go(&handle, url);
                    }
                }
                None => {}
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

// Links to other websites (zKillboard, Dotlan, ...) go to the normal browser. Some of them
// sit in menus that close on the press, so the browser's own "new window" never comes;
// the app is asked directly instead (once per press). __DR_LINKS__ lets pip.js do the
// same in the scanner pop-out's document.
(() => {
    const OURS = /(^|\.)(devilsrage\.duckdns\.org|seatdevilsrage\.duckdns\.org|eveonline\.com|discord\.com|discordapp\.com)$/i;
    let last = { href: '', at: 0 };
    const external = (target) => {
        const a = target && typeof target.closest === 'function' ? target.closest('a[href]') : null;
        if (!a) return null;
        let url;
        try { url = new URL(a.getAttribute('href'), a.ownerDocument.baseURI); } catch (e) { return null; }
        if (url.protocol !== 'https:' && url.protocol !== 'http:') return null;
        if (OURS.test(url.hostname)) return null;
        return url.href;
    };
    const open = (href, win) => {
        const now = Date.now();
        if (last.href === href && now - last.at < 1500) return;
        last = { href, at: now };
        try { (win || window).open('https://dr-app.invalid/open?url=' + encodeURIComponent(href), '_blank'); } catch (e) {}
    };
    const watch = (doc, win) => {
        let pressed = null;
        doc.addEventListener('pointerdown', (e) => { pressed = external(e.target); }, true);
        const handle = (e) => {
            if (e.button !== 0 && e.button !== 1) return;
            const href = external(e.target);
            if (!href) return;
            if (e.type === 'pointerup' && pressed !== href) return;
            if (e.type !== 'pointerup') e.preventDefault();
            open(href, win);
        };
        doc.addEventListener('pointerup', handle, true);
        doc.addEventListener('click', handle, true);
        doc.addEventListener('auxclick', handle, true);
    };
    watch(document, window);
    window.__DR_LINKS__ = watch;
})();
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
                // A page's link to another website (see SHORTCUTS).
                "/open" => {
                    if let Some(target) = url.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned()) {
                        open_in_browser(&app, &me, &target);
                    }
                }
                "/clipboard" | "/attention" if from_mapper(&app, &me) => page_signal(&app, &me, &url),
                // Cameras: one EVE client's own sound for the hole watch (clientaudio.rs).
                "/audio-clients" | "/audio-listen" | "/audio-learn" if from_mapper(&app, &me) => clientaudio::signal(&app, &me, &url),
                "/audio-clients" | "/audio-listen" | "/audio-learn" => log::warn!("ignored {} from a non-mapper window ({me})", url.path()),
                // Cameras: the page asks for its window directly (a link's own new window can
                // vanish with the menu it sits in).
                "/cameras" if from_mapper(&app, &me) => {
                    match url.query_pairs().find(|(k, _)| k == "url").and_then(|(_, v)| Url::parse(&v).ok()) {
                        Some(target) if camera_window(&target).is_some() => open_camera_window(&app, &marker, target),
                        _ => log::warn!("ignored /cameras without a camera page ({me})"),
                    }
                }
                // Cameras: the share page / wall's own Close button (its window goes, even if the
                // page is too busy to close itself).
                "/close-window" if me == "cameras" || me == "camwall" => {
                    if let Some(window) = app.get_webview_window(&me) {
                        log::info!("closing the {me} window (its Close button)");
                        let _ = window.destroy();
                    }
                }
                // Patch 51b: the character tabs close / bring up a character's pop-out.
                "/popout-focus" if from_mapper(&app, &me) && url.query_pairs().any(|(k, v)| k == "own" && v == "1") => {
                    scanner::focus_for(&app, &me);
                }
                "/popout-close" | "/popout-focus" if from_mapper(&app, &me) => {
                    let character: String = url
                        .query_pairs()
                        .find(|(k, _)| k == "character")
                        .map(|(_, v)| v.chars().filter(char::is_ascii_digit).collect())
                        .unwrap_or_default();
                    if !character.is_empty() {
                        if url.path() == "/popout-close" {
                            scanner::close_for_character(&app, &character);
                        } else {
                            scanner::focus_for_character(&app, &character);
                        }
                    }
                }
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

        // Cameras: "Share a camera" and the camera wall get a window of their own, so the map
        // stays where it is (the share page keeps streaming while it's open).
        if camera_window(&url).is_some() {
            open_camera_window(&app, &marker, url);
            return NewWindowResponse::Deny;
        }

        let solo = url.query_pairs().any(|(k, v)| k == "solo" && v == "1");
        // A link of ours that asks for a new tab (SeAT's Discord connector "register" button,
        // target="_blank"): the app has no tabs, and an allowed new window never showed, so
        // the button did nothing. It opens in the app's page instead.
        let our_site = url.host_str().is_some_and(|host| host == host_of(SEAT_URL) || host == host_of(MAPPER_URL));
        if !solo && our_site {
            log::info!("new window for {} opened in the app's page instead", url.path());
            let app = app.clone();
            tauri::async_runtime::spawn(async move { go(&app, url) });
            return NewWindowResponse::Deny;
        }
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
            .disable_drag_drop_handler()
            .window_features(features)
            .title("Devil's Rage")
            .always_on_top(!host)
            .visible(!host)
            .skip_taskbar(host)
            .initialization_script(marker.clone())
            .on_page_load({
                let app = app.clone();
                let label = label.clone();
                move |_, payload| {
                    if matches!(payload.event(), PageLoadEvent::Started) {
                        scanner::close_for_parent(&app, &label);
                    }
                }
            })
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

/// Cameras: opens (or brings up) the cameras / camera wall window. Built off the page's own
/// event (like `go`), so a page waiting on the answer never holds it up.
fn open_camera_window(app: &AppHandle, marker: &str, url: Url) {
    let Some(label) = camera_window(&url) else { return };
    if let Some(existing) = app.get_webview_window(label) {
        let _ = existing.navigate(url);
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        return;
    }
    log::info!("opening the {label} window ({})", url.path());
    let app = app.clone();
    let marker = marker.to_string();
    tauri::async_runtime::spawn(async move {
        let built = WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
            .disable_drag_drop_handler()
            .title(if label == "cameras" { "Devil's Rage · share a camera" } else { "Devil's Rage · camera wall" })
            .inner_size(1360.0, 860.0)
            .initialization_script(marker.clone())
            .on_new_window(new_window_handler(app.clone(), marker, label.to_string()))
            .build();
        match built {
            Ok(window) => {
                let _ = window.set_focus();
            }
            Err(err) => log::warn!("couldn't open the {label} window: {err}"),
        }
    });
}

/// Cameras: the window a camera page of the mapper opens in ("cameras" for Share a camera,
/// "camwall" for the wall), or None for any other page.
fn camera_window(url: &Url) -> Option<&'static str> {
    if url.host_str() != Some(host_of(MAPPER_URL)) {
        return None;
    }
    let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["maps", _, "cameras", "share"] => Some("cameras"),
        ["maps", _, "cameras"] => Some("camwall"),
        _ => None,
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


#[cfg(test)]
mod camera_window_tests {
    use super::*;

    #[test]
    fn camera_pages_get_their_own_window() {
        let at = |path: &str| camera_window(&Url::parse(&format!("{MAPPER_URL}{path}")).unwrap());
        assert_eq!(at("/maps/daisy-1/cameras/share"), Some("cameras"));
        assert_eq!(at("/maps/daisy-1/cameras"), Some("camwall"));
        assert_eq!(at("/maps/daisy-1"), None);
        assert_eq!(at("/maps/daisy-1/camera-feed"), None);
        assert_eq!(camera_window(&Url::parse("https://example.com/maps/x/cameras").unwrap()), None);
    }
}
