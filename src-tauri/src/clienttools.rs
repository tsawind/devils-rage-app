//! Devil's Rage Client Tools (Tsa, 20 Nov): a second, small program next to the app with only
//! what you need to fly: previews, client bars, client window spots and the cycle hotkeys. No
//! mapper, no SeAT, no main window, nothing that needs the internet, so it keeps working when
//! the big app, SeAT, the mapper or an EVE patch breaks something.
//!
//! It's the same code as the app's own previews and bars (same settings, layouts and spots,
//! from the same settings folder), started without the rest. Only one of the two runs at a
//! time: Client Tools won't start while the app is open, and the app closes Client Tools when
//! it starts (it does all of this itself).

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

/// The program's file name next to the app (bundled by the Windows installer).
pub const EXE: &str = "Devils Rage Client Tools.exe";
/// The app's own program name (installed as devils-rage-app.exe).
pub const APP_EXE: &str = "devils-rage-app.exe";

static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This process is Client Tools (set before the app starts).
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
pub const TOOLS_LOCK: &str = "DevilsRageClientTools";

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
        .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {exe}")])
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

/// The app is starting: close Client Tools if it's open (the app does its job now).
pub fn close_if_running() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        use std::os::windows::process::CommandExt;
        if running(EXE) {
            let _ = std::process::Command::new("taskkill").args(["/IM", EXE, "/F"]).creation_flags(0x0800_0000).status();
            log::info!("closed Devil's Rage Client Tools: the app does previews and bars now");
        }
    });
}

/// Start Client Tools: refuse while the app is open, else previews, bars, spots and hotkeys.
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    log::info!("Devil's Rage Client Tools {} starting", app.package_info().version);
    if !claim(TOOLS_LOCK) {
        log::info!("Client Tools is already running: this copy closes");
        app.exit(0);
        return Ok(());
    }
    if held(APP_LOCK) || running(APP_EXE) {
        log::info!("the app is open: Client Tools leaves it the previews and bars");
        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
        let app = app.clone();
        app.dialog()
            .message("Devil's Rage is open, and it already shows the previews and client bars.\n\nClient Tools is for when the app won't start: close the app first, then open Client Tools.")
            .title("Devil's Rage Client Tools")
            .kind(MessageDialogKind::Info)
            .show(move |_| app.exit(0));
        return Ok(());
    }
    crate::incognito::load(app);
    crate::launch::load_account_logins(app);
    #[cfg(desktop)]
    crate::launch::register_cycle_hotkeys(app);
    crate::previews::start_watch(app.clone());
    crate::clientbar::start(app.clone());
    tray(app)?;
    let _ = crate::previews::open_previews(app.clone());
    Ok(())
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open-previews", "Open previews", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close-previews", "Close previews", true, None::<&str>)?;
    let edit = MenuItem::with_id(app, "edit-previews", "Move / resize previews", true, None::<&str>)?;
    let done = MenuItem::with_id(app, "done-previews", "Lock previews in place", true, None::<&str>)?;
    let place = MenuItem::with_id(app, "place-clients", "Put EVE windows back", true, None::<&str>)?;
    let full = MenuItem::with_id(app, "full-app", "Open the full Devil's Rage app", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Client Tools", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let line2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &close, &edit, &done, &place, &line, &full, &line2, &quit])?;
    let mut builder = TrayIconBuilder::with_id("devils-rage-client-tools")
        .tooltip("Devil's Rage Client Tools (previews, bars, hotkeys)")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
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

/// Start the full app (it closes Client Tools when it starts).
fn open_full_app(app: &AppHandle) {
    let Some(exe) = std::env::current_exe().ok().and_then(|me| me.parent().map(|dir| dir.join(APP_EXE))) else { return };
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => log::info!("started the full app"),
        Err(err) => {
            log::warn!("couldn't start {}: {err}", exe.display());
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            app.dialog().message(format!("Couldn't start the app: {err}")).kind(MessageDialogKind::Error).show(|_| {});
        }
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
        assert!(others_in_tasklist("\"Devils Rage Client Tools.exe\",\"77\",\"Console\"", EXE, "1"));
    }
}
