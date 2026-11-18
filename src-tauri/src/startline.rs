//! Patch 74: which settings profile a running EVE client was started with, read from its start-up
//! line (the official launcher passes it there), so a login screen ("Login 2") can be told apart:
//! its account, its name on the bar, its spot. Only the profile is kept; the rest of the line
//! (which also carries the launcher's sign-in token) is dropped at once and never stored or logged.
//! Option names (never values) are written to the log once per client, in case EVE names the
//! profile option differently.

use std::collections::HashMap;
use std::sync::Mutex;

static CACHE: Mutex<Option<HashMap<u32, Option<String>>>> = Mutex::new(None);
/// Patch 74 (Tsa, 18 Nov: "I want guarantees"): the EVE account each client was signed in to, by
/// process. Read from the launcher's sign-in token on the start-up line: only the account number
/// is kept; the token itself is dropped at once and never stored or logged.
static ACCOUNTS: Mutex<Option<HashMap<u32, Option<u64>>>> = Mutex::new(None);
/// Patch 74 (Tsa, 18 Nov: "do those"): what the token says about each EVE account: its login name
/// (for Your accounts' login hint, this PC only) and its tier (Omega / Alpha). Account → (login, tier).
static INFO: Mutex<Option<HashMap<u64, (Option<String>, Option<String>)>>> = Mutex::new(None);

/// The login name and tier the tokens named for each EVE account seen so far.
pub fn account_info() -> HashMap<u64, (Option<String>, Option<String>)> {
    INFO.lock().unwrap().clone().unwrap_or_default()
}

/// A short plain text claim (letters, digits, a few marks), else None.
fn plain(v: Option<&serde_json::Value>, max: usize) -> Option<String> {
    let text = v?.as_str()?.trim();
    (!text.is_empty() && text.len() <= max && text.chars().all(|c| c.is_alphanumeric() || " _-.@".contains(c))).then(|| text.to_string())
}

/// Base64url (no padding) to bytes; None on anything else.
fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The EVE account number in a start-up line's sign-in token, and a description of the token's
/// shape for the log (part count and claim names: never a value).
pub fn account_in(line: &str) -> (Option<u64>, String) {
    let (account, shape, _) = signed_in(line);
    (account, shape)
}

/// `account_in` plus the token's login name and tier claims.
fn signed_in(line: &str) -> (Option<u64>, String, (Option<String>, Option<String>)) {
    let all = args(line);
    let token = all.iter().enumerate().find_map(|(i, arg)| {
        let (key, value) = key_of(arg)?;
        (key == "ssotoken").then(|| value.filter(|v| !v.is_empty()).map(str::to_string).or_else(|| all.get(i + 1).cloned()))?
    });
    let Some(token) = token else { return (None, "no ssotoken".into(), (None, None)) };
    let parts: Vec<&str> = token.split('.').collect();
    let Some(payload) = parts.get(1).filter(|_| parts.len() == 3).and_then(|p| base64url(p)).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return (None, format!("ssotoken: {} part(s), not a readable JWT", parts.len()), (None, None));
    };
    let names: Vec<String> = payload.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    // "sub": "USER:EVE:12345678" (an account) or "CHARACTER:EVE:…" (a character).
    let sub = payload.get("sub").and_then(|v| v.as_str()).unwrap_or("");
    let kind = sub.split(':').next().unwrap_or("").to_string();
    let account = sub.strip_prefix("USER:EVE:").and_then(|n| n.parse::<u64>().ok());
    let tier = plain(payload.get("tier"), 20).or_else(|| payload.get("tier").filter(|v| !v.is_null()).map(|v| v.to_string().chars().filter(|c| c.is_alphanumeric()).take(20).collect()));
    let extra = (plain(payload.get("name"), 64), tier);
    (account, format!("ssotoken: JWT, claims [{}], sub kind {:?}", names.join(", "), kind), extra)
}

/// Like `account_of_window`, but never waits (None until its start-up line has been read).
pub fn known_account_of_window(handle: isize) -> Option<u64> {
    let pid = pid_of(handle)?;
    ACCOUNTS.lock().unwrap().as_ref().and_then(|c| c.get(&pid).copied()).flatten()
}

/// The EVE account a client window was signed in to (cached per process; read with its profile).
pub fn account_of_window(handle: isize) -> Option<u64> {
    let pid = pid_of(handle)?;
    if !CACHE.lock().unwrap().as_ref().is_some_and(|c| c.contains_key(&pid)) {
        let _ = profile_of_window(handle);
    }
    ACCOUNTS.lock().unwrap().as_ref().and_then(|c| c.get(&pid).copied()).flatten()
}

/// Split a Windows command line into arguments (quotes respected).
fn args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut now = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !now.is_empty() {
                    out.push(std::mem::take(&mut now));
                }
            }
            c => now.push(c),
        }
    }
    if !now.is_empty() {
        out.push(now);
    }
    out
}

fn key_of(arg: &str) -> Option<(String, Option<&str>)> {
    let bare = arg.strip_prefix('/').or_else(|| arg.strip_prefix("--")).or_else(|| arg.strip_prefix('-'))?;
    let (key, value) = match bare.find(['=', ':']) {
        Some(i) => (&bare[..i], Some(&bare[i + 1..])),
        None => (bare, None),
    };
    Some((key.to_ascii_lowercase(), value))
}

/// The settings profile in a start-up line (`/settingsprofile=1` → "1", `settings_RollFleet` → "RollFleet").
pub fn profile_in(line: &str) -> Option<String> {
    let all = args(line);
    for (i, arg) in all.iter().enumerate() {
        let Some((key, value)) = key_of(arg) else { continue };
        if key == "settingsprofile" || key == "settings_profile" || key == "profile" {
            let value = match value {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => all.get(i + 1)?.clone(),
            };
            let value = value.trim().trim_start_matches("settings_").to_string();
            if !value.is_empty() && value.len() <= 60 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ') {
                return Some(value);
            }
        }
    }
    None
}

/// Option names only (for the log): never their values.
pub fn option_names(line: &str) -> Vec<String> {
    args(line).iter().skip(1).filter_map(|a| key_of(a).map(|(k, _)| k)).filter(|k| k.len() <= 40).collect()
}

#[cfg(windows)]
fn read_line(pid: u32) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').CommandLine")])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(not(windows))]
fn read_line(_pid: u32) -> Option<String> {
    None
}

#[cfg(windows)]
fn pid_of(handle: isize) -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    // SAFETY: reads which process owns a window.
    unsafe { GetWindowThreadProcessId(handle as windows_sys::Win32::Foundation::HWND, &mut pid) };
    (pid != 0).then_some(pid)
}

#[cfg(not(windows))]
fn pid_of(_handle: isize) -> Option<u32> {
    None
}

/// The settings profile of the client that owns this window (cached per process).
pub fn profile_of_window(handle: isize) -> Option<String> {
    let pid = pid_of(handle)?;
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&pid).cloned()) {
        return hit;
    }
    let mut account = None;
    let profile = read_line(pid).and_then(|line| {
        log::info!("EVE client {pid} start-up options (names only): {}", option_names(&line).join(" "));
        let (found, shape, extra) = signed_in(&line);
        if let Some(user) = found {
            INFO.lock().unwrap().get_or_insert_with(HashMap::new).insert(user, extra);
        }
        log::info!("EVE client {pid} {shape}; account {}", if found.is_some() { "found" } else { "not found" });
        account = found;
        profile_in(&line)
    });
    {
        let mut guard = ACCOUNTS.lock().unwrap();
        let accounts = guard.get_or_insert_with(HashMap::new);
        if accounts.len() > 200 {
            accounts.clear();
        }
        accounts.insert(pid, account);
    }
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if cache.len() > 200 {
        cache.clear();
    }
    cache.insert(pid, profile.clone());
    profile
}

/// Processes whose start-up line is being read in the background right now.
static READING: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Like `profile_of_window`, but never waits: `None` while the line is still being read (a
/// background thread reads it), `Some(profile)` once known. For the 150 ms window loops.
pub fn known_profile_of_window(handle: isize) -> Option<Option<String>> {
    let Some(pid) = pid_of(handle) else { return Some(None) };
    if let Some(hit) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&pid).cloned()) {
        return Some(hit);
    }
    let mut reading = READING.lock().unwrap();
    if !reading.contains(&pid) {
        reading.push(pid);
        std::thread::spawn(move || {
            let _ = profile_of_window(handle);
            READING.lock().unwrap().retain(|p| *p != pid);
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_profile_and_nothing_else() {
        let line = r#""C:\EVE\SharedCache\tq\bin64\exefile.exe" /noconsole /ssoToken=abc.def.ghi /settingsprofile=1 /triPlatform=dx12"#;
        assert_eq!(profile_in(line), Some("1".into()));
        assert_eq!(option_names(line), vec!["noconsole", "ssotoken", "settingsprofile", "triplatform"]);
        assert_eq!(profile_in(r#"exefile.exe /settingsprofile="settings_RollFleet" /x"#), Some("RollFleet".into()));
        assert_eq!(profile_in("exefile.exe /settingsprofile settings_3"), Some("3".into()));
        assert_eq!(profile_in("exefile.exe /noconsole"), None);
        assert_eq!(profile_in("exefile.exe /settingsprofile=../../evil"), None);
    }

    #[test]
    fn finds_the_account_in_the_sign_in_token() {
        // header.payload.signature, payload {"sub":"USER:EVE:7205846","name":"x"} in base64url.
        let payload = "eyJzdWIiOiJVU0VSOkVWRTo3MjA1ODQ2IiwibmFtZSI6IngifQ";
        let line = format!("exefile.exe /noconsole /ssoToken=aGVhZA.{payload}.c2ln /settingsprofile=1");
        let (account, shape) = account_in(&line);
        assert_eq!(account, Some(7_205_846));
        assert_eq!(signed_in(&line).2, (Some("x".to_string()), None));
        assert!(shape.contains("sub kind \"USER\"") && !shape.contains("7205846"));
        assert_eq!(account_in("exefile.exe /ssoToken=opaque").0, None);
        assert_eq!(account_in("exefile.exe /noconsole").1, "no ssotoken");
    }
}
