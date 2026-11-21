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

/// Close the clients, wait for EVE to write their profile, then switch that profile's display mode.
/// `clients` = (name, window, where it should be when it opens again); more than one only when
/// they share the profile (each rewrites it as it closes). `known` = the profile folder read from
/// the client's start-up line; `profile_hint` = the account's own profile, used only if neither
/// that nor a changed file says which profile it is.
pub fn run(app: AppHandle, clients: Vec<(String, isize, Spot)>, to_fixed: bool, width: i64, height: i64, known: Option<String>, profile_hint: Option<String>) -> Result<(), String> {
    let root = crate::profiles::root()?;
    let before = yamls(&root);
    let Some((name, _, _)) = clients.first().cloned() else { return Err("no client".into()) };
    for (client, handle, _) in &clients {
        if !crate::clientbar::close_window(*handle) {
            return Err(format!("couldn't ask {client} to close"));
        }
    }
    std::thread::Builder::new()
        .name("window-fix".into())
        .spawn(move || {
            let names: Vec<String> = clients.iter().map(|(n, _, _)| n.clone()).collect();
            let who = names.join(", ");
            let title = format!("{name} · window mode");
            // 1. The clients close (EVE may ask to confirm first: up to 90 s).
            let started = Instant::now();
            loop {
                let open = crate::clipboard::open_clients();
                let still: Vec<&String> = clients.iter().filter(|(_, h, _)| open.iter().any(|(_, o)| o == h)).map(|(n, _, _)| n).collect();
                if still.is_empty() {
                    break;
                }
                if started.elapsed() > Duration::from_secs(90) {
                    let still = still.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ");
                    tell(&app, &title, format!("{still} didn't close, so nothing was changed. If EVE asked to confirm, try again and pick Quit."), MessageDialogKind::Warning);
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let closed = Instant::now();
            // 2. Which profile: the start-up line says; else EVE writes its profile as it closes,
            // so the one file that changed is this client's; else the account's own profile.
            let mut file: Option<PathBuf> = known.as_ref().map(|p| root.join(format!("settings_{p}")).join("core_public__.yaml"));
            if let Some(f) = &file {
                // Known: still wait (up to 15 s) for EVE to write it, so ours comes after.
                let was = before.get(f).copied();
                while closed.elapsed() < Duration::from_secs(15) && std::fs::metadata(f).and_then(|m| m.modified()).ok() == was {
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
            while file.is_none() && closed.elapsed() < Duration::from_secs(20) {
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
                    tell(&app, &title, format!("{who} closed, but I couldn't tell which profile it uses (or its file isn't there yet), so nothing was changed."), MessageDialogKind::Warning);
                    return;
                }
            };
            // 3. Quiet for 3 s (and at least 3 s after the last one closed) before touching it.
            let mut last = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            let mut quiet_since = Instant::now();
            while closed.elapsed() < Duration::from_secs(3) || quiet_since.elapsed() < Duration::from_secs(3) {
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
            let profile = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let open = crate::clipboard::open_clients();
            if let Some((again, _)) = open.iter().find(|(n, _)| names.contains(n)) {
                tell(&app, &title, format!("{again} is open again, so nothing was changed."), MessageDialogKind::Warning);
                return;
            }
            // Someone else opened in the same profile meanwhile: it would write it back as it closes.
            let short = profile.trim_start_matches("settings_");
            let sharing: Vec<String> = open.iter().filter(|(_, h)| crate::startline::settings_folder_of_window(*h).is_some_and(|p| p.eq_ignore_ascii_case(short))).map(|(n, _)| n.clone()).collect();
            if !sharing.is_empty() {
                tell(&app, &title, format!("{} opened in {profile} meanwhile, so nothing was changed (it would undo it when it closes). Close it and try again.", sharing.join(", ")), MessageDialogKind::Warning);
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
            for (client, _, to) in &clients {
                PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(client.clone(), (to_fixed, *to));
                if to_fixed {
                    crate::clientwindows::remember_one(&app, client, *to);
                    crate::clientwindows::note_mode(&app, client, true);
                }
            }
            log::info!("window mode: {profile} -> {} for {} client(s)", if to_fixed { "fixed" } else { "windowed" }, clients.len());
            let mode = if to_fixed { "Fixed Window" } else { "Window mode" };
            let shared = if profile.eq_ignore_ascii_case("settings_Default") {
                "\n\nsettings_Default is EVE's shared profile: every account without its own profile opens this way now."
            } else {
                ""
            };
            let again = if clients.len() > 1 { format!("Launch {who} again: each opens in its same spot.") } else { format!("Launch {name} again: it opens in the same spot.") };
            tell(&app, &title, format!("{profile} is now {mode} at {width}×{height} (backup beside it).\n\n{again}{shared}"), MessageDialogKind::Info);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

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
