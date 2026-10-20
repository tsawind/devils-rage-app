//! Parsec (Patch 50, PI): "Connect to my EVE PC" on the PI screen opens the Parsec app on
//! this computer, straight into the pilot's EVE computer when they saved its ID in Settings.
//! The app never touches EVE: Parsec shows the other screen and the pilot does everything.

use std::path::PathBuf;
use std::process::Command;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

const DOWNLOAD: &str = "https://parsec.app/downloads";

/// Where Parsec installs itself (all users, then just this user).
fn candidates() -> Vec<PathBuf> {
    let mut list = Vec::new();
    #[cfg(windows)]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            list.push(PathBuf::from(pf).join("Parsec").join("parsecd.exe"));
        }
        if let Some(roaming) = dirs::config_dir() {
            list.push(roaming.join("Parsec").join("parsecd.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    list.push(PathBuf::from(
        "/Applications/Parsec.app/Contents/MacOS/parsecd",
    ));
    #[cfg(all(unix, not(target_os = "macos")))]
    list.push(PathBuf::from("/usr/bin/parsecd"));
    list
}

/// Parsec's program on this computer, if it's installed.
pub fn installed() -> Option<PathBuf> {
    candidates().into_iter().find(|path| path.is_file())
}

/// A Parsec computer ID is letters and digits; anything else is dropped (it becomes one
/// argument to Parsec, never a shell command).
pub fn clean_id(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect()
}

/// Opens Parsec (connecting to the saved computer, if any). No Parsec: its download page.
pub fn connect(app: &AppHandle) {
    let peer = clean_id(
        crate::settings::load(app)
            .parsec_peer_id
            .as_deref()
            .unwrap_or(""),
    );
    let Some(exe) = installed() else {
        log::info!("Parsec isn't installed: opening its download page");
        let _ = app.opener().open_url(DOWNLOAD, None::<&str>);
        return;
    };
    let mut command = Command::new(&exe);
    if !peer.is_empty() {
        command.arg(format!("peer_id={peer}"));
    }
    match command.spawn() {
        Ok(_) => log::info!(
            "Parsec opened{}",
            if peer.is_empty() {
                ""
            } else {
                " for the saved computer"
            }
        ),
        Err(err) => log::warn!("couldn't open Parsec ({}): {err}", exe.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::clean_id;

    #[test]
    fn ids_keep_only_letters_and_digits() {
        assert_eq!(clean_id(" 2AbC9xyz "), "2AbC9xyz");
        assert_eq!(
            clean_id("abc:client_vsync=1 & calc.exe"),
            "abcclientvsync1calcexe"
        );
        assert_eq!(clean_id(""), "");
        assert_eq!(clean_id(&"a".repeat(100)).len(), 64);
    }
}
