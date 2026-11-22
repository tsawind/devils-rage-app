//! Devil's Peepshow (22 Nov; was "Client Tools", 20 Nov): its own program with what you need to
//! fly: previews, client bars and window modes, client window spots, the cycle hotkeys and
//! Setups. No mapper, no SeAT, nothing that leaves this PC, so it keeps working when the Devils
//! app, SeAT, the mapper or an EVE patch breaks something, and anyone can use it.
//!
//! It's the same code as the app (this library), started with Peepshow's own identifier, so it
//! has its own settings folders. It reads what the app knows (SeAT groups and accounts, this
//! PC's group keys, incognito characters) from the app's folder (shared.rs) and asks the app
//! for cameras, pop-outs and the app's pages through the mailbox (mailbox.rs). The Devils app
//! starts Peepshow when it opens and leaves the previews, bars and hotkeys to it.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Peepshow's program name (its own installer puts it in its own folder).
pub const EXE: &str = "devils-peepshow.exe";
/// The second program as dev.227–249 installed it next to the app (closed when found).
pub const OLD_EXES: [&str; 2] = ["devils-rage-client-tools.exe", "Devils Rage Client Tools.exe"];
/// The app's own program name (installed as devils-rage-app.exe).
pub const APP_EXE: &str = "devils-rage-app.exe";
/// Peepshow's main window.
pub const WINDOW: &str = "peepshow";

/// The Devils app started Peepshow (or found it running) and left it the previews, bars,
/// spots and hotkeys.
static OWNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn peepshow_owns() -> bool {
    OWNED.load(std::sync::atomic::Ordering::SeqCst)
}

static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This process is Devil's Peepshow (set before it starts).
pub fn set_on() {
    ON.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn on() -> bool {
    ON.load(std::sync::atomic::Ordering::SeqCst)
}

/// 21 Nov: one copy of each program at a time. Two copies of the app (or the app and Client
/// Tools) each draw their own client bars and keep stacking them just above the client, so the
/// bars flip back and forth. Each program holds a named lock while it runs.
pub const APP_LOCK: &str = "DevilsRageApp";
pub const PEEPSHOW_LOCK: &str = "DevilsPeepshow";
/// The old Client Tools' lock (dev.227–249).
pub const OLD_TOOLS_LOCK: &str = "DevilsRageClientTools";

/// Take this program's lock for as long as it runs. False: another copy already holds it.
#[cfg(windows)]
pub fn claim(lock: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = format!("Local\\{lock}").encode_utf16().chain([0]).collect();
    // SAFETY: a named mutex; the handle is kept (never closed) while this program runs.
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return false;
        }
    }
    true
}

#[cfg(not(windows))]
pub fn claim(_lock: &str) -> bool {
    true
}

/// Whether another program holds this lock right now.
#[cfg(windows)]
pub fn held(lock: &str) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name: Vec<u16> = format!("Local\\{lock}").encode_utf16().chain([0]).collect();
    // SAFETY: opens (and closes again) a named mutex, only to see that it exists.
    unsafe {
        let handle = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        CloseHandle(handle);
    }
    true
}

#[cfg(not(windows))]
pub fn held(_lock: &str) -> bool {
    false
}

/// A second copy of the app was started: bring the running one's window forward instead.
#[cfg(windows)]
pub fn show_running_app() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE};
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        let mut buf = [0u16; 128];
        // SAFETY: reads a window's title into our buffer.
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        let mut pid = 0u32;
        // SAFETY: the window's process id.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if title.starts_with("Devil's Rage — ") && pid != std::process::id() {
            // SAFETY: restore and focus the running app's main window.
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            return 0;
        }
        1
    }
    // SAFETY: walks the top-level windows with the callback above.
    unsafe { EnumWindows(Some(each), 0) };
}

#[cfg(not(windows))]
pub fn show_running_app() {}

/// Whether a program with this file name is running (other than this process).
#[cfg(windows)]
fn running(exe: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let me = std::process::id().to_string();
    std::process::Command::new("tasklist")
        .raw_arg(format!("/FO CSV /NH /FI \"IMAGENAME eq {exe}\""))
        .creation_flags(NO_WINDOW)
        .output()
        .map(|out| others_in_tasklist(&String::from_utf8_lossy(&out.stdout), exe, &me))
        .unwrap_or(false)
}

/// Whether `tasklist /FO CSV /NH` output lists `exe` with a process id other than `me`.
#[cfg_attr(not(windows), allow(dead_code))]
fn others_in_tasklist(output: &str, exe: &str, me: &str) -> bool {
    let start = format!("\"{}\"", exe.to_lowercase());
    output
        .lines()
        .filter(|line| line.to_lowercase().starts_with(&start))
        .any(|line| line.split(',').nth(1).map(|pid| pid.trim_matches('"')) != Some(me))
}

#[cfg(not(windows))]
fn running(_exe: &str) -> bool {
    false
}

/// Close the old Client Tools (dev.227–249) if it's still open: Peepshow replaces it.
pub fn close_old_client_tools() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        use std::os::windows::process::CommandExt;
        if held(OLD_TOOLS_LOCK) || OLD_EXES.iter().any(|exe| running(exe)) {
            for exe in OLD_EXES {
                let _ = std::process::Command::new("taskkill").raw_arg(format!("/IM \"{exe}\" /F")).creation_flags(0x0800_0000).status();
            }
            log::info!("closed the old Client Tools: Devil's Peepshow replaces it");
        }
    });
}

/// The Devils app is starting: start Peepshow (unless it's running already) and leave it the
/// previews, bars, spots and hotkeys. False: Peepshow isn't installed, the app does them itself.
pub fn start_peepshow() -> bool {
    if held(PEEPSHOW_LOCK) {
        OWNED.store(true, std::sync::atomic::Ordering::SeqCst);
        log::info!("Devil's Peepshow is running: it shows the previews, bars and hotkeys");
        return true;
    }
    let Some(exe) = crate::shared::find_exe(true) else {
        log::info!("Devil's Peepshow isn't installed: the app shows the previews, bars and hotkeys itself");
        return false;
    };
    match std::process::Command::new(&exe).arg("--hidden").spawn() {
        Ok(_) => {
            OWNED.store(true, std::sync::atomic::Ordering::SeqCst);
            log::info!("started Devil's Peepshow ({}): it shows the previews, bars and hotkeys", exe.display());
            true
        }
        Err(err) => {
            log::warn!("couldn't start Devil's Peepshow ({}): {err}; the app does previews and bars itself", exe.display());
            false
        }
    }
}

/// Home's Peepshow card: is it installed and running?
#[tauri::command]
pub fn peepshow_status() -> serde_json::Value {
    let installed = crate::shared::find_exe(true);
    serde_json::json!({
        "installed": installed.is_some(),
        "running": held(PEEPSHOW_LOCK),
        "owns": peepshow_owns(),
        "path": installed.map(|p| p.display().to_string()),
    })
}

/// Peepshow's window: is the Devils app on this PC (its button shows only then)?
#[tauri::command]
pub fn peepshow_app_installed() -> bool {
    crate::shared::find_exe(false).is_some()
}

/// The app's "Open Devil's Peepshow": bring its window up (start it first if needed).
#[tauri::command]
pub fn open_peepshow(page: Option<String>) -> Result<String, String> {
    let page = page.filter(|p| ["peepshow.html", "previews.html", "setups.html"].contains(&p.as_str()));
    if held(PEEPSHOW_LOCK) {
        crate::mailbox::send(crate::mailbox::To::Peepshow, serde_json::json!({ "do": "show", "page": page }));
        return Ok("Opened Devil's Peepshow.".into());
    }
    let exe = crate::shared::find_exe(true).ok_or("Devil's Peepshow isn't installed. Reinstall the Devils Rage app (it brings Peepshow along).")?;
    std::process::Command::new(&exe).spawn().map_err(|e| format!("Couldn't start Devil's Peepshow: {e}"))?;
    OWNED.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(page) = page {
        // Its window opens by itself; the page follows once it's listening.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            crate::mailbox::send(crate::mailbox::To::Peepshow, serde_json::json!({ "do": "show", "page": page }));
        });
    }
    Ok("Started Devil's Peepshow.".into())
}

/// The Devils app's side of the mailbox: Peepshow asks for cameras, pop-outs and its pages.
pub fn app_message(_app: &AppHandle, message: serde_json::Value) {
    let what = message.get("do").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    let text = |key: &str| message.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    match what.as_str() {
        "camera" => {
            let (name, on) = (text("name"), message.get("on").and_then(serde_json::Value::as_bool).unwrap_or(true));
            crate::mainthread::run(move |app| {
                if let Err(err) = crate::clientbar::clientbar_camera(app.clone(), name, on) {
                    log::warn!("camera from Peepshow: {err}");
                }
            });
        }
        "popout" => {
            let (name, id) = (text("name"), message.get("id").and_then(serde_json::Value::as_i64).unwrap_or(0));
            crate::mainthread::run(move |app| {
                if let Err(err) = crate::clientbar::clientbar_popout(app.clone(), id, name) {
                    log::warn!("pop-out from Peepshow: {err}");
                }
            });
        }
        "page" => {
            let page = text("page");
            crate::mainthread::run(move |app| {
                let _ = crate::clientbar::clientbar_open_page(app.clone(), page);
            });
        }
        "show" => {
            crate::mainthread::run(|app| {
                if let Some(window) = app.get_window(crate::MAIN_WINDOW) {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            });
        }
        other => log::info!("unknown message for the app: {other}"),
    }
}

/// Peepshow's side of the mailbox.
fn peepshow_message(_app: &AppHandle, message: serde_json::Value) {
    let what = message.get("do").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    match what.as_str() {
        "show" => open_window(message.get("page").and_then(serde_json::Value::as_str).map(str::to_string)),
        "pause_hotkeys" => {
            crate::mainthread::run(|app| crate::launch::pause_cycle_hotkeys(app.clone()));
        }
        "resume_hotkeys" => {
            crate::mainthread::run(|app| crate::launch::resume_cycle_hotkeys(app.clone()));
        }
        "open_previews" => {
            crate::mainthread::run(|app| {
                let _ = crate::previews::open_previews(app.clone());
            });
        }
        other => log::info!("unknown message for Peepshow: {other}"),
    }
}

/// Peepshow's own window (peepshow.html, or `page`): shown, or built (call from a thread of its
/// own: building a window inside main-thread work can lock up on Windows).
pub fn open_window(page: Option<String>) {
    let Some(app) = crate::mainthread::app() else { return };
    let page = page.unwrap_or_else(|| "peepshow.html".into());
    let shown = crate::mainthread::run({
        let page = page.clone();
        move |app| {
            let Some(window) = app.get_webview_window(WINDOW) else { return false };
            let _ = window.eval(format!("location.href = {};", serde_json::Value::String(page)));
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            true
        }
    });
    if shown == Some(true) {
        return;
    }
    let built = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App(page.into()))
        .title("Devil's Peepshow")
        .inner_size(860.0, 720.0)
        .min_inner_size(620.0, 480.0)
        .initialization_script("window.__PEEPSHOW__ = true;")
        .disable_drag_drop_handler()
        .build();
    if let Err(err) = built {
        log::warn!("couldn't open Peepshow's window: {err}");
    }
}

/// Start Devil's Peepshow: previews, bars, spots, hotkeys, its tray icon and window.
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    log::info!("Devil's Peepshow {} starting", app.package_info().version);
    if !claim(PEEPSHOW_LOCK) {
        // Already running: bring its window up instead.
        log::info!("Devil's Peepshow is already running: this copy shows its window and closes");
        crate::mailbox::send(crate::mailbox::To::Peepshow, serde_json::json!({ "do": "show" }));
        app.exit(0);
        return Ok(());
    }
    close_old_client_tools();
    crate::shared::note_own_exe(true);
    crate::shared::copy_app_settings_once(app);
    crate::incognito::load(app);
    crate::launch::load_account_logins(app);
    #[cfg(desktop)]
    crate::launch::register_cycle_hotkeys(app);
    crate::previews::start_watch(app.clone());
    crate::clientbar::start(app.clone());
    tray(app)?;
    crate::mailbox::start(crate::mailbox::To::Peepshow, peepshow_message);
    follow_app_files();
    let _ = crate::previews::open_previews(app.clone());
    if !std::env::args().any(|a| a == "--hidden") {
        std::thread::spawn(|| open_window(None));
    }
    #[cfg(desktop)]
    crate::updates::start(app.clone());
    Ok(())
}

/// The app's pages change its group keys, launch groups, accounts and incognito characters:
/// reload them here when their files change (every 2 s).
fn follow_app_files() {
    std::thread::spawn(|| {
        let mut last: Option<Vec<Option<std::time::SystemTime>>> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let Some(app) = crate::mainthread::app() else { continue };
            let now = crate::shared::followed_stamps(app);
            if last.as_ref().is_some_and(|l| *l != now) {
                log::info!("the Devils app changed its groups or accounts: reloading them");
                crate::mainthread::run(|app| {
                    crate::incognito::load(app);
                    crate::launch::load_account_logins(app);
                    #[cfg(desktop)]
                    crate::launch::register_cycle_hotkeys(app);
                });
            }
            last = Some(now);
        }
    });
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let window = MenuItem::with_id(app, "window", "Open Devil's Peepshow", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open-previews", "Open previews", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close-previews", "Close previews", true, None::<&str>)?;
    let edit = MenuItem::with_id(app, "edit-previews", "Move / resize previews", true, None::<&str>)?;
    let done = MenuItem::with_id(app, "done-previews", "Lock previews in place", true, None::<&str>)?;
    let place = MenuItem::with_id(app, "place-clients", "Put EVE windows back", true, None::<&str>)?;
    let full = MenuItem::with_id(app, "full-app", "Open the Devils Rage app", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Devil's Peepshow", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let line2 = PredefinedMenuItem::separator(app)?;
    let line3 = PredefinedMenuItem::separator(app)?;
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![&window, &line, &open, &close, &edit, &done, &place, &line2];
    // The Devils app is the corp's: only offered where it's installed.
    let has_app = crate::shared::find_exe(false).is_some();
    if has_app {
        items.push(&full);
    }
    items.push(&line3);
    items.push(&quit);
    let menu = Menu::with_items(app, &items)?;
    let mut builder = TrayIconBuilder::with_id("devils-peepshow")
        .tooltip("Devil's Peepshow (previews, client bars, hotkeys)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|_tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                std::thread::spawn(|| open_window(None));
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "window" => {
                std::thread::spawn(|| open_window(None));
            }
            "open-previews" => {
                let _ = crate::previews::open_previews(app.clone());
            }
            "close-previews" => crate::previews::close_previews(app.clone()),
            "edit-previews" => {
                let _ = crate::previews::set_preview_edit(app.clone(), true);
            }
            "done-previews" => {
                let _ = crate::previews::set_preview_edit(app.clone(), false);
            }
            "place-clients" => {
                let _ = crate::clientwindows::client_windows_place(app.clone());
            }
            "full-app" => open_full_app(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Start the Devils app (or bring it forward if it's open).
#[tauri::command]
pub fn open_devils_app(app: AppHandle) -> Result<String, String> {
    if held(APP_LOCK) {
        crate::mailbox::send(crate::mailbox::To::App, serde_json::json!({ "do": "show" }));
        return Ok("Opened the Devils Rage app.".into());
    }
    let exe = crate::shared::find_exe(false).ok_or("The Devils Rage app isn't installed on this PC.")?;
    let _ = app;
    std::process::Command::new(&exe).spawn().map_err(|e| format!("Couldn't start the app: {e}"))?;
    Ok("Starting the Devils Rage app…".into())
}

fn open_full_app(app: &AppHandle) {
    if let Err(err) = open_devils_app(app.clone()) {
        log::warn!("{err}");
        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
        app.dialog().message(err).kind(MessageDialogKind::Error).show(|_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasklist_shows_another_copy_but_not_this_one() {
        let out = "\"devils-rage-app.exe\",\"4412\",\"Console\",\"1\",\"210,112 K\"\r\n";
        assert!(others_in_tasklist(out, "devils-rage-app.exe", "999"));
        assert!(!others_in_tasklist(out, "devils-rage-app.exe", "4412"));
        assert!(!others_in_tasklist("INFO: No tasks are running which match the specified criteria.", "devils-rage-app.exe", "1"));
        assert!(others_in_tasklist("\"Devils Rage Client Tools.exe\",\"77\",\"Console\"", OLD_EXES[1], "1"));
        assert!(others_in_tasklist("\"devils-rage-client-tools.exe\",\"78\",\"Console\"", OLD_EXES[0], "1"));
        assert!(others_in_tasklist("\"devils-peepshow.exe\",\"79\",\"Console\"", EXE, "1"));
    }
}
