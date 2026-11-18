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
                // Patch 57: a D-scan or overview rows with structures go into the pilot's system intel.
                let structures = structure_lines(&text);
                if structures == 0 {
                    // Patch 58: a short copy from EVE (a structure's name from Show Info or a chat
                    // link) goes to the mapper too; only an open intel window uses it (asks the type).
                    if is_short_copy(&text) {
                        if let Some(name) = foreground_eve_character() {
                            let id = crate::location::character_id(&name);
                            deliver_event(&app, "devils-rage-app:copy", id, &name, &text);
                        }
                    }
                    continue;
                }
                let Some(name) = foreground_eve_character() else {
                    log::info!("structures copied, but not from an EVE window in front: left for Ctrl+V");
                    continue;
                };
                let id = crate::location::character_id(&name);
                log::info!("{structures} structure line(s) copied by {name}, handed to the mapper's intel");
                deliver_event(&app, "devils-rage-app:intel", id, &name, &text);
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
    deliver_event(app, "devils-rage-app:scan", character_id, name, text);
}

fn deliver_event(app: &AppHandle, event: &str, character_id: Option<i64>, name: &str, text: &str) {
    let detail = serde_json::json!({
        "character_id": character_id,
        "character_name": name,
        "text": text,
    });
    crate::location::send_to_mapper(
        app,
        &format!("window.dispatchEvent(new CustomEvent('{event}', {{ detail: {detail} }}));"),
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

/// Patch 57: structure type names (lower case) the mapper's intel keeps; POS towers are any
/// "... Control Tower ...". Kept in step with the mapper's StructureScan.
const STRUCTURE_TYPES: [&str; 14] = [
    "astrahus",
    "fortizar",
    "'moreau' fortizar",
    "'draccous' fortizar",
    "'horizon' fortizar",
    "'marginis' fortizar",
    "'prometheus' fortizar",
    "keepstar",
    "upwell palatine keepstar",
    "raitaru",
    "azbel",
    "sotiyo",
    "athanor",
    "tatara",
];

fn is_structure_type(column: &str) -> bool {
    let lower = column.trim().to_lowercase();
    STRUCTURE_TYPES.contains(&lower.as_str()) || lower.contains("control tower")
}

/// Patch 57: how many lines of a D-scan or overview copy are structures we track, or 0 when it
/// isn't one (most lines must be tab-separated rows, so a chat line naming a Fortizar isn't).
pub fn structure_lines(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() || lines.len() > 2000 {
        return 0;
    }
    let rows: Vec<Vec<&str>> = lines
        .iter()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|columns| columns.len() >= 3)
        .collect();
    if rows.len() * 10 < lines.len() * 8 {
        return 0;
    }
    rows.iter().filter(|columns| columns.iter().any(|c| is_structure_type(c))).count()
}

/// Patch 58: a name-sized copy (one or two short lines, no table rows).
pub fn is_short_copy(text: &str) -> bool {
    let trimmed = text.trim();
    let lines = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
    !trimmed.is_empty() && trimmed.chars().count() <= 200 && lines <= 2 && !trimmed.contains('\t')
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

/// Patch 54: the characters of every EVE client open on this PC (the "EVE - <name>" windows).
#[cfg(windows)]
pub fn open_eve_characters() -> Vec<String> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};

    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the &mut Vec<String> passed to EnumWindows below, alive for the call.
        let names = unsafe { &mut *(found as *mut Vec<String>) };
        let mut buf = [0u16; 256];
        // SAFETY: plain Win32 calls on a window handle EnumWindows gave us and a stack buffer.
        let len = unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
        };
        if len > 0 {
            if let Some(name) = eve_character_from_title(&String::from_utf16_lossy(&buf[..len as usize])) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        1
    }

    let mut names: Vec<String> = Vec::new();
    // SAFETY: the callback only touches `names` through the pointer, during this call.
    unsafe {
        EnumWindows(Some(each), &mut names as *mut Vec<String> as LPARAM);
    }
    names
}

#[cfg(not(windows))]
pub fn open_eve_characters() -> Vec<String> {
    Vec::new()
}

/// Every visible top-level window with a title: (title, handle).
#[cfg(windows)]
fn visible_titled_windows() -> Vec<(String, isize)> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};

    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the &mut Vec passed to EnumWindows below, alive for the call.
        let windows = unsafe { &mut *(found as *mut Vec<(String, isize)>) };
        let mut buf = [0u16; 256];
        // SAFETY: plain Win32 calls on a window handle EnumWindows gave us and a stack buffer.
        let len = unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
        };
        if len > 0 {
            windows.push((String::from_utf16_lossy(&buf[..len as usize]), hwnd as isize));
        }
        1
    }

    let mut windows: Vec<(String, isize)> = Vec::new();
    // SAFETY: the callback only touches `windows` through the pointer, during this call.
    unsafe {
        EnumWindows(Some(each), &mut windows as *mut Vec<(String, isize)> as LPARAM);
    }
    windows
}

/// Cameras: each EVE client's window handle with its character, so the share page can tell
/// which client a shared window is (the browser names a shared window "window:<handle>:0").
#[cfg(windows)]
pub fn open_eve_windows() -> Vec<(String, isize)> {
    visible_titled_windows()
        .into_iter()
        .filter_map(|(title, handle)| Some((eve_character_from_title(&title)?, handle)))
        .collect()
}

/// EVE clients sitting at the character-select / login screen (their window is titled just "EVE").
#[cfg(windows)]
pub fn open_login_windows() -> Vec<isize> {
    let mut handles: Vec<isize> = visible_titled_windows()
        .into_iter()
        .filter(|(title, _)| title.trim() == "EVE")
        .map(|(_, handle)| handle)
        .collect();
    handles.sort_unstable();
    handles
}

#[cfg(not(windows))]
pub fn open_login_windows() -> Vec<isize> {
    Vec::new()
}

/// The name a login-screen client goes by in previews ("Login 1", "Login 2", …).
pub fn login_label(index: usize) -> String {
    format!("Login {}", index + 1)
}

/// Every EVE client window: logged-in ones by character, ones still at the login /
/// character-select screen as "Login 1", "Login 2", … (in a steady order while they stay open).
pub fn open_clients() -> Vec<(String, isize)> {
    let mut all = open_eve_windows();
    let logins = open_login_windows();
    let numbers = login_numbers(&logins);
    all.extend(logins.into_iter().zip(numbers).filter_map(|(handle, n)| Some((login_label(n?), handle))));
    all
}

/// Each login-screen window keeps its number while it's open (others logging in or closing
/// don't renumber it).
static LOGIN_NUMBERS: Mutex<Vec<(isize, usize)>> = Mutex::new(Vec::new());

/// Patch 74: login screens are tied to accounts. Account N in Your accounts order (by nickname)
/// owns "Login N"; this holds each account's settings profile in that order (None: no profile).
static ACCOUNT_LOGINS: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

/// When each login window was first seen while its profile was still being read.
static WAITING_SINCE: Mutex<Vec<(isize, std::time::Instant)>> = Mutex::new(Vec::new());

/// How long a new login screen may stay out of the list while we find out its account.
const PROFILE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Set the accounts' profiles in Your accounts order (on start-up and after every SeAT sync).
pub fn set_account_logins(profiles: Vec<Option<String>>) {
    *ACCOUNT_LOGINS.lock().unwrap() = profiles;
}

/// The login number (0-based) for a login screen started with `profile`: its account's own
/// number when that's free, else the lowest free number after every account's.
fn pick_login_number(profile: Option<&str>, accounts: &[Option<String>], used: &[usize]) -> usize {
    if let Some(profile) = profile {
        if let Some(n) = accounts.iter().position(|p| p.as_deref() == Some(profile)) {
            if !used.contains(&n) {
                return n;
            }
        }
    }
    (accounts.len()..).find(|n| !used.contains(n)).unwrap_or(accounts.len())
}

/// Numbers for these login windows; `None` for one whose account we're still finding out
/// (left out of the list for up to five seconds, so it doesn't change name a moment later).
fn login_numbers(handles: &[isize]) -> Vec<Option<usize>> {
    let accounts = ACCOUNT_LOGINS.lock().unwrap().clone();
    let mut known = LOGIN_NUMBERS.lock().unwrap();
    known.retain(|(handle, _)| handles.contains(handle));
    let mut waiting = WAITING_SINCE.lock().unwrap();
    waiting.retain(|(handle, _)| handles.contains(handle));
    handles
        .iter()
        .map(|handle| {
            if let Some((_, n)) = known.iter().find(|(h, _)| h == handle) {
                return Some(*n);
            }
            let profile = match crate::startline::known_profile_of_window(*handle) {
                Some(profile) => profile,
                None => {
                    let since = match waiting.iter().find(|(h, _)| h == handle) {
                        Some((_, t)) => *t,
                        None => {
                            waiting.push((*handle, std::time::Instant::now()));
                            return None;
                        }
                    };
                    if since.elapsed() < PROFILE_WAIT {
                        return None;
                    }
                    None
                }
            };
            let used: Vec<usize> = known.iter().map(|(_, n)| *n).collect();
            let n = pick_login_number(profile.as_deref(), &accounts, &used);
            known.push((*handle, n));
            Some(n)
        })
        .collect()
}

/// The login-screen names that belong to accounts, in Your accounts order: "Login 1" is the
/// first account's, and so on.
pub fn account_login_names() -> Vec<String> {
    (0..ACCOUNT_LOGINS.lock().unwrap().len()).map(login_label).collect()
}

/// Whether a client name is a login screen ("Login 3").
pub fn is_login_label(name: &str) -> bool {
    name.strip_prefix("Login ").is_some_and(|n| n.parse::<usize>().is_ok())
}

/// The EVE client in front right now (a character, or "Login N"), if an EVE window is in front.
#[cfg(windows)]
pub fn foreground_client() -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: a plain Win32 call with no arguments.
    let front = unsafe { GetForegroundWindow() } as isize;
    if front == 0 {
        return None;
    }
    open_clients().into_iter().find(|(_, handle)| *handle == front).map(|(name, _)| name)
}

#[cfg(not(windows))]
pub fn foreground_client() -> Option<String> {
    None
}

/// The window in front (a handle), cheap enough to ask many times a second.
#[cfg(windows)]
pub fn foreground_handle() -> isize {
    // SAFETY: a plain Win32 call with no arguments.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

#[cfg(not(windows))]
pub fn foreground_handle() -> isize {
    0
}

#[cfg(not(windows))]
pub fn open_eve_windows() -> Vec<(String, isize)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN: &str = "ABC-123\tCosmic Signature\tWormhole\tUnstable Wormhole\t100.0%\t4.32 AU\n\
                        DEF-456\tCosmic Signature\t\t\t12.5%\t9.10 AU\n\
                        GHI-789\tCosmic Anomaly\tCombat Site\tPerimeter Ambush Point\t100.0%\t2.01 AU\n";

    #[test]
    fn counts_structures_in_dscan_and_overview_copies() {
        let dscan = "35833\tJ145735 - Old-Daisy\tFortizar\t27.7 AU\n12011\tA Legion\tLegion\t3.1 AU\n16213\tTower\tAmarr Control Tower Large\t4 AU";
        assert_eq!(structure_lines(dscan), 2);
        assert_eq!(structure_lines("27.7 AU\tJ145735 - Old-Daisy\tFortizar\t-\t-\t\t[TDVS]"), 1);
        assert_eq!(structure_lines("3.3 AU\tJ145735 - Star\tSun G5 (Pink)\t-"), 0);
        assert_eq!(structure_lines("anyone seen the Fortizar?"), 0);
        assert_eq!(structure_lines(SCAN), 0);
        assert_eq!(structure_lines(""), 0);
    }

    #[test]
    fn login_numbers_stay_put() {
        assert_eq!(login_numbers(&[101, 102, 103]), vec![Some(0), Some(1), Some(2)]);
        // 102 logged in: the others keep their numbers.
        assert_eq!(login_numbers(&[101, 103]), vec![Some(0), Some(2)]);
        // A new one takes the free number.
        assert_eq!(login_numbers(&[101, 103, 104]), vec![Some(0), Some(2), Some(1)]);
        assert!(is_login_label("Login 12") && !is_login_label("Login Screen Guy"));
        login_numbers(&[]);
    }

    #[test]
    fn login_screens_take_their_accounts_number() {
        let accounts = vec![Some("1".to_string()), None, Some("29".to_string())];
        // Account 01's login screen is Login 1, account 03's is Login 3.
        assert_eq!(pick_login_number(Some("1"), &accounts, &[]), 0);
        assert_eq!(pick_login_number(Some("29"), &accounts, &[0]), 2);
        // Unknown profile, or no profile: after every account's number.
        assert_eq!(pick_login_number(Some("77"), &accounts, &[]), 3);
        assert_eq!(pick_login_number(None, &accounts, &[3]), 4);
        // The same account open twice: the second one gets a free number after the accounts'.
        assert_eq!(pick_login_number(Some("1"), &accounts, &[0]), 3);
        // No accounts synced: plain lowest free number.
        assert_eq!(pick_login_number(Some("1"), &[], &[0, 2]), 1);
    }

    #[test]
    fn knows_a_short_copy() {
        assert!(is_short_copy("J170845 - Main Barracks (E85 Holding Services)"));
        assert!(!is_short_copy(""));
        assert!(!is_short_copy("a\tb\tc"));
        assert!(!is_short_copy("one\ntwo\nthree"));
    }

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
