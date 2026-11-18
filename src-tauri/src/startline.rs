//! Patch 74: which settings profile a running EVE client was started with, read from its start-up
//! line (the official launcher passes it there), so a login screen ("Login 2") can be told apart:
//! its account, its name on the bar, its spot. Only the profile is kept; the rest of the line
//! (which also carries the launcher's sign-in token) is dropped at once and never stored or logged.
//! Option names (never values) are written to the log once per client, in case EVE names the
//! profile option differently.

use std::collections::HashMap;
use std::sync::Mutex;

static CACHE: Mutex<Option<HashMap<u32, Option<String>>>> = Mutex::new(None);

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
    let profile = read_line(pid).and_then(|line| {
        log::info!("EVE client {pid} start-up options (names only): {}", option_names(&line).join(" "));
        profile_in(&line)
    });
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if cache.len() > 200 {
        cache.clear();
    }
    cache.insert(pid, profile.clone());
    profile
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
}
