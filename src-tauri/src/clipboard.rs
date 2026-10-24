//! The clipboard, which a browser can only touch from the focused window:
//!
//! - Writing: the mapper asks (https://dr-app.invalid/clipboard?text=...) and the app writes,
//!   even while EVE has focus. That's how the way back lands on the clipboard right after a jump.
//! - Watching: a probe scan copied in EVE (Ctrl+A, Ctrl+C in the probe scanner) is handed to a
//!   mapper page in the app with the character whose EVE window it came from ("EVE - <name>"),
//!   and the mapper pastes it into that character's system.
//!
//! CCP rules: we only read the clipboard and the title of the window in front. Nothing is ever
//! sent to the EVE client.

use std::sync::Mutex;
use std::time::Duration;

use tauri::AppHandle;

use crate::settings;

/// What the app wrote last, so the watcher doesn't take its own writes for a copy in EVE.
static LAST_WRITTEN: Mutex<Option<String>> = Mutex::new(None);
/// What Settings shows: the last scan handed over ("Tsawind, 12 signatures").
static LAST_SCAN: Mutex<Option<String>> = Mutex::new(None);

pub fn last_scan() -> Option<String> {
    LAST_SCAN.lock().unwrap().clone()
}

/// Puts text on the clipboard.
/// Kept open: on Linux the text stays on the clipboard only while its owner is alive.
static WRITER: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

pub fn write(text: &str) -> Result<(), String> {
    *LAST_WRITTEN.lock().unwrap() = Some(text.to_string());
    let mut writer = WRITER.lock().unwrap();
    if writer.is_none() {
        *writer = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    let clipboard = writer.as_mut().ok_or("no clipboard")?;
    clipboard.set_text(text.to_string()).map_err(|e| e.to_string())
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut clipboard: Option<arboard::Clipboard> = None;
        let mut last_seen: Option<String> = None;
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let wanted = settings::load(&app).clipboard_scans()
                && crate::appconfig::feature_on("clipboard_scans");
            if !wanted {
                clipboard = None;
                last_seen = None;
                continue;
            }
            if clipboard.is_none() {
                match arboard::Clipboard::new() {
                    Ok(c) => clipboard = Some(c),
                    Err(err) => {
                        log::warn!("can't watch the clipboard: {err}");
                        std::thread::sleep(Duration::from_secs(30));
                        continue;
                    }
                }
            }
            let Some(c) = clipboard.as_mut() else { continue };
            // Not text (an image, nothing): not a scan.
            let Ok(text) = c.get_text() else { continue };

            // The first look only learns what's there already (no paste of an old copy at start).
            let first = last_seen.is_none();
            if last_seen.as_deref() == Some(text.as_str()) {
                continue;
            }
            last_seen = Some(text.clone());
            if first || LAST_WRITTEN.lock().unwrap().as_deref() == Some(text.as_str()) {
                continue;
            }

            let count = probe_scan_lines(&text);
            if count == 0 {
                continue;
            }
            // Whose scan: the EVE window in front when it was copied.
            let Some(name) = foreground_eve_character() else {
                log::info!("probe scan copied, but not from an EVE window in front: left for Ctrl+V");
                continue;
            };
            // Patch 52: chat logs off (no id) is fine: the mapper page matches the name.
            let id = crate::location::character_id(&name);
            log::info!(
                "probe scan copied by {name}: {count} signatures, handed to the mapper{}",
                if id.is_none() { " (by name: no chat log)" } else { "" }
            );
            *LAST_SCAN.lock().unwrap() = Some(format!(
                "{name}, {count} signature{}",
                if count == 1 { "" } else { "s" }
            ));
            deliver(&app, id, &name, &text);
        }
    });
}

fn deliver(app: &AppHandle, character_id: Option<i64>, name: &str, text: &str) {
    let detail = serde_json::json!({
        "character_id": character_id,
        "character_name": name,
        "text": text,
    });
    crate::location::send_to_mapper(
        app,
        &format!("window.dispatchEvent(new CustomEvent('devils-rage-app:scan', {{ detail: {detail} }}));"),
    );
}

/// How many lines of `text` are probe scanner rows ("ABC-123<tab>Cosmic Signature<tab>..."),
/// or 0 when it isn't a probe scan (most lines must be rows, so a chat line that happens to
/// look like one isn't taken for a scan).
pub fn probe_scan_lines(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() || lines.len() > 500 {
        return 0;
    }
    let rows = lines.iter().filter(|line| is_scan_row(line)).count();
    if rows * 10 >= lines.len() * 8 {
        rows
    } else {
        0
    }
}

fn is_scan_row(line: &str) -> bool {
    let mut fields = line.split('\t');
    let Some(id) = fields.next() else { return false };
    let b = id.as_bytes();
    let id_ok = b.len() == 7
        && b[..3].iter().all(u8::is_ascii_uppercase)
        && b[3] == b'-'
        && b[4..].iter().all(u8::is_ascii_digit);
    // Rows have the group ("Cosmic Signature", "Cosmic Anomaly", ...) and more columns.
    id_ok && fields.count() >= 3
}

/// The character name from an EVE client window title ("EVE - Tsawind").
pub fn eve_character_from_title(title: &str) -> Option<String> {
    let name = title.trim().strip_prefix("EVE - ")?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The character whose EVE window is in front, from its title.
#[cfg(windows)]
pub fn foreground_eve_character() -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    let mut buf = [0u16; 256];
    // SAFETY: plain Win32 calls on a stack buffer of the length we pass.
    let len = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
    };
    if len <= 0 {
        return None;
    }
    eve_character_from_title(&String::from_utf16_lossy(&buf[..len as usize]))
}

/// Other systems: not yet (EVE runs on Windows for most of us).
#[cfg(not(windows))]
pub fn foreground_eve_character() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN: &str = "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU\n\
                        DEF-456\tCosmic Signature\t\t\t12.5%\t9.10 AU\n\
                        GHI-789\tCosmic Anomaly\tCombat Site\tPerimeter Ambush Point\t100.0%\t2.01 AU\n";

    #[test]
    fn counts_probe_scan_rows() {
        assert_eq!(probe_scan_lines(SCAN), 3);
        assert_eq!(probe_scan_lines("ABC-123\tCosmic Signature\t\t\t0.0%\t1 AU"), 1);
    }

    #[test]
    fn ignores_text_that_is_not_a_scan() {
        assert_eq!(probe_scan_lines(""), 0);
        assert_eq!(probe_scan_lines("  * A2 OPP C3"), 0);
        assert_eq!(probe_scan_lines("ABC-123 is the static"), 0);
        // A d-scan (type id first) is not a probe scan.
        assert_eq!(probe_scan_lines("670\tCapsule\tCapsule\t-\n11176\tCrow\tInterceptor\t1,234 km"), 0);
        // Mostly chat with one row in it.
        let chat = "hi\nwhere are you\nABC-123\tCosmic Signature\t\t\t0.0%\t1 AU\nok\nbrb";
        assert_eq!(probe_scan_lines(chat), 0);
    }

    #[test]
    fn reads_the_character_from_an_eve_window_title() {
        assert_eq!(eve_character_from_title("EVE - Tsawind"), Some("Tsawind".into()));
        assert_eq!(eve_character_from_title("EVE - Mario Squirrel "), Some("Mario Squirrel".into()));
        assert_eq!(eve_character_from_title("EVE"), None);
        assert_eq!(eve_character_from_title("EVE - "), None);
        assert_eq!(eve_character_from_title("Devil's Rage"), None);
    }
}
