//! Patch 74: the Devil's window mode. A Window-mode client switches to EVE's Fixed Window at the
//! same size (1918×1024), in the same spot, with our title bar and edge round it (clientbar.rs) —
//! so Windows can never resize or half-snap it by accident. And back again.
//!
//! EVE keeps the Fixed Window size in the profile's `core_public__.yaml` (`FixedWindowSettings`)
//! and only reads it at start, and it rewrites that file when the client closes. So: the client
//! is closed (EVE may ask to confirm), we wait until EVE has finished writing the file (it's the
//! profile whose file changed after the close; then quiet for 2 s), back it up, write WindowMode
//! and the size, and the client is launched again by the player. When it opens, it's put in its
//! spot (clientbar's follower).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::clientwindows::Spot;

/// Clients converted and waiting to be launched again: name -> (Fixed Window?, where the window goes).
static PENDING: Mutex<Option<HashMap<String, (bool, Spot)>>> = Mutex::new(None);

pub fn pending(name: &str) -> Option<(bool, Spot)> {
    PENDING.lock().unwrap().as_ref().and_then(|m| m.get(name).copied())
}

/// The spot of the one converted Fixed Window client of this size waiting to be launched again
/// (None when there's none, or more than one: then the login screen can't be told apart).
pub fn pending_fixed_of_size(w: i32, h: i32) -> Option<Spot> {
    let guard = PENDING.lock().unwrap();
    let mut hits = guard.as_ref()?.values().filter(|(fixed, to)| *fixed && (to.w - w).abs() <= 2 && (to.h - h).abs() <= 2);
    let first = hits.next().map(|(_, to)| *to);
    if hits.next().is_some() {
        return None;
    }
    first
}

pub fn done(name: &str) {
    if let Some(m) = PENDING.lock().unwrap().as_mut() {
        m.remove(name);
    }
}

/// `  WindowMode: [stamp, 2]` in `device:` (added under `device:` when missing).
pub fn set_window_mode(text: &str, mode: i64, stamp: u64) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut in_device = false;
    for line in lines.iter_mut() {
        if !line.starts_with(' ') {
            in_device = line.trim_end() == "device:";
            continue;
        }
        if in_device && line.starts_with("  WindowMode: [") {
            *line = format!("  WindowMode: [{stamp}, {mode}]");
            return finish(lines, text);
        }
    }
    if let Some(i) = lines.iter().position(|l| l.trim_end() == "device:") {
        lines.insert(i + 1, format!("  WindowMode: [{stamp}, {mode}]"));
    }
    finish(lines, text)
}

fn finish(lines: Vec<String>, original: &str) -> String {
    let mut out = lines.join("\n");
    if original.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A size block in `device:` (`FixedWindowSettings` / `WindowedSettings`) set to `width`×`height`,
/// keeping its other values (adapter, left, top…). A `[stamp, null]` or missing one becomes a block.
pub fn set_block_size(text: &str, key: &str, width: i64, height: i64, stamp: u64) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut in_device = false;
    let mut found: Option<(usize, usize)> = None; // block lines [start, end)
    let mut single: Option<usize> = None;
    let mut device_at: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !line.starts_with(' ') {
            in_device = line.trim_end() == "device:";
            if in_device {
                device_at = Some(i);
            }
            i += 1;
            continue;
        }
        if in_device && line.trim_end() == format!("  {key}:") {
            let mut end = i + 1;
            while end < lines.len() && (lines[end].starts_with("  -") || lines[end].starts_with("   ")) {
                end += 1;
            }
            found = Some((i, end));
            break;
        }
        if in_device && line.starts_with(&format!("  {key}: [")) {
            single = Some(i);
            break;
        }
        i += 1;
    }
    let block = |adapter: i64, rest: &str| {
        vec![format!("  {key}:"), format!("  - {stamp}"), format!("  - {{adapter: {adapter}, height: {height}{rest}, width: {width}}}")]
    };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (found, single) {
        (Some((start, end)), _) => {
            let joined = lines[start + 1..end].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
            // "- 1343… - {adapter: 0, height: 1056, …}"
            let body = joined.find('{').map(|o| &joined[o..]).unwrap_or("{}");
            let mut values: Vec<(String, String)> = body
                .trim_matches(|c| c == '{' || c == '}')
                .split(',')
                .filter_map(|p| p.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
                .collect();
            for (k, v) in [("width", width), ("height", height)] {
                match values.iter_mut().find(|(name, _)| name == k) {
                    Some(entry) => entry.1 = v.to_string(),
                    None => values.push((k.to_string(), v.to_string())),
                }
            }
            values.sort_by(|a, b| a.0.cmp(&b.0));
            let inner = values.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ");
            let new = vec![format!("  {key}:"), format!("  - {stamp}"), format!("  - {{{inner}}}")];
            out.splice(start..end, new);
        }
        (None, Some(at)) => {
            out.splice(at..at + 1, block(0, ", presentInterval: 0"));
        }
        (None, None) => {
            if let Some(at) = device_at {
                let new = block(0, ", presentInterval: 0");
                for (k, l) in new.into_iter().enumerate() {
                    out.insert(at + 1 + k, l);
                }
            }
        }
    }
    finish(out, text)
}

fn yamls(root: &Path) -> HashMap<PathBuf, SystemTime> {
    let mut out = HashMap::new();
    if let Ok(dirs) = std::fs::read_dir(root) {
        for dir in dirs.flatten() {
            let name = dir.file_name().to_string_lossy().to_string();
            if crate::profiles::profile_of(&name).is_none() {
                continue;
            }
            let file = dir.path().join("core_public__.yaml");
            if let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) {
                out.insert(file, modified);
            }
        }
    }
    out
}

fn tell(app: &AppHandle, title: &str, text: String, kind: MessageDialogKind) {
    app.dialog().message(text).title(title).kind(kind).show(|_| {});
}

/// Close the client, wait for EVE to write its profile, then switch that profile's display mode.
/// `to` = where the window should be when it opens again; `profile_hint` = the account's own
/// profile, used only if no profile file changed when the client closed.
pub fn run(app: AppHandle, name: String, handle: isize, to_fixed: bool, width: i64, height: i64, to: Spot, profile_hint: Option<String>) -> Result<(), String> {
    let root = crate::profiles::root()?;
    let before = yamls(&root);
    if !crate::clientbar::close_window(handle) {
        return Err("couldn't ask the client to close".into());
    }
    std::thread::Builder::new()
        .name("window-fix".into())
        .spawn(move || {
            let title = format!("{name} · window mode");
            // 1. The client closes (EVE may ask to confirm first: up to 90 s).
            let started = Instant::now();
            while crate::clipboard::open_clients().iter().any(|(_, h)| *h == handle) {
                if started.elapsed() > Duration::from_secs(90) {
                    tell(&app, &title, format!("{name} didn't close, so nothing was changed. If EVE asked to confirm, try again and pick Quit."), MessageDialogKind::Warning);
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let closed = Instant::now();
            // 2. EVE writes its profile as it closes: the file that changed is this client's.
            let mut file: Option<PathBuf> = None;
            while closed.elapsed() < Duration::from_secs(20) {
                let now = yamls(&root);
                let changed: Vec<&PathBuf> = now.iter().filter(|(p, t)| before.get(*p).is_none_or(|b| b != *t)).map(|(p, _)| p).collect();
                if changed.len() == 1 {
                    file = Some(changed[0].clone());
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            let file = match file.or_else(|| profile_hint.as_ref().map(|p| root.join(format!("settings_{p}")).join("core_public__.yaml"))) {
                Some(f) if f.exists() => f,
                _ => {
                    tell(&app, &title, format!("{name} closed, but I couldn't tell which profile it uses, so nothing was changed."), MessageDialogKind::Warning);
                    return;
                }
            };
            // 3. Quiet for 2 s (and at least 2 s after closing) before touching it.
            let mut last = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            let mut quiet_since = Instant::now();
            while closed.elapsed() < Duration::from_secs(2) || quiet_since.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(250));
                let now = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
                if now != last {
                    last = now;
                    quiet_since = Instant::now();
                }
                if closed.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
            if crate::clipboard::open_clients().iter().any(|(n, _)| *n == name) {
                tell(&app, &title, format!("{name} is open again, so nothing was changed."), MessageDialogKind::Warning);
                return;
            }
            // 4. Back up, then write the mode and size.
            let Ok(text) = std::fs::read_to_string(&file) else {
                tell(&app, &title, "Couldn't read the profile file; nothing was changed.".into(), MessageDialogKind::Error);
                return;
            };
            let stamp = crate::tuning::file_time_now();
            let backup = file.with_file_name(format!("core_public__.yaml.before-window-mode-{}", chrono_like_now()));
            if std::fs::write(&backup, &text).is_err() {
                tell(&app, &title, "Couldn't back up the profile file; nothing was changed.".into(), MessageDialogKind::Error);
                return;
            }
            let key = if to_fixed { "FixedWindowSettings" } else { "WindowedSettings" };
            let changed = set_block_size(&set_window_mode(&text, if to_fixed { 2 } else { 1 }, stamp), key, width, height, stamp);
            if let Err(err) = std::fs::write(&file, changed) {
                tell(&app, &title, format!("Couldn't write the profile file ({err}); nothing was changed."), MessageDialogKind::Error);
                return;
            }
            PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.clone(), (to_fixed, to));
            if to_fixed {
                crate::clientwindows::remember_one(&app, &name, to);
            }
            let profile = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mode = if to_fixed { "Fixed Window" } else { "Window mode" };
            tell(
                &app,
                &title,
                format!("{profile} is now {mode} at {width}×{height} (backup beside it).\n\nLaunch {name} again: it opens in the same spot."),
                MessageDialogKind::Info,
            );
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A filename-safe local-ish time stamp (seconds since 1970; no extra crate).
fn chrono_like_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSAWIND: &str = "audio:\n  masterVolume: [1, 0.7]\ndevice:\n  DeviceSettings: [1, null]\n  FixedWindow: [1, 0]\n  FixedWindowSettings:\n  - 134394192932409075\n  - {adapter: 0, height: 1056, presentInterval: 0, width: 1920}\n  UIScaleWindowed: [1, 0.9]\n  WindowMode: [134394192932409075, 1]\n  WindowedSettings:\n  - 134394192932409075\n  - {adapter: 0, height: 1024, left: -7, presentInterval: 0, showState: 0, top: 0,\n    width: 1918}\n  antiAliasing: [1, 0]\ngeneric: {}\n";

    #[test]
    fn fixed_window_at_the_window_mode_size() {
        let out = set_block_size(&set_window_mode(TSAWIND, 2, 7), "FixedWindowSettings", 1918, 1024, 7);
        assert!(out.contains("  WindowMode: [7, 2]\n"));
        assert!(out.contains("  FixedWindowSettings:\n  - 7\n  - {adapter: 0, height: 1024, presentInterval: 0, width: 1918}\n  UIScaleWindowed"));
        // nothing else touched
        assert!(out.contains("  - {adapter: 0, height: 1024, left: -7, presentInterval: 0, showState: 0, top: 0,\n    width: 1918}"));
        assert!(out.contains("  antiAliasing: [1, 0]\ngeneric: {}\n"));
        let seat = crate::setups::seat_of(&out).unwrap();
        assert_eq!((seat.mode, seat.fixed_width, seat.fixed_height), (Some(2), Some(1918), Some(1024)));
    }

    #[test]
    fn wrapped_windowed_block_keeps_its_spot() {
        let out = set_block_size(TSAWIND, "WindowedSettings", 1280, 720, 9);
        let seat = crate::setups::seat_of(&out).unwrap();
        assert_eq!((seat.left, seat.top, seat.width, seat.height), (Some(-7), Some(0), Some(1280), Some(720)));
        assert!(out.contains("  FixedWindowSettings:\n  - 134394192932409075\n"));
    }

    #[test]
    fn null_or_missing_blocks_are_made() {
        let null = "device:\n  FixedWindowSettings: [1, null]\n  WindowMode: [1, 1]\n";
        let out = set_block_size(null, "FixedWindowSettings", 1918, 1024, 5);
        assert_eq!(out, "device:\n  FixedWindowSettings:\n  - 5\n  - {adapter: 0, height: 1024, presentInterval: 0, width: 1918}\n  WindowMode: [1, 1]\n");
        let missing = set_window_mode("device:\n  antiAliasing: [1, 0]\n", 2, 5);
        assert_eq!(missing, "device:\n  WindowMode: [5, 2]\n  antiAliasing: [1, 0]\n");
    }
}
