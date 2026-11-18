//! Patch 73: Setups, step 1 — look only. Reads every client profile folder and says, in one
//! place, where each client opens (the "seat"), what each account's keys / overview / effects are
//! and which character layouts were made at which client size, and compares chosen setups side
//! by side. Nothing is written: this is the map we need before seats and templates.
//!
//! Where EVE keeps things (checked 17 Nov on Tsa's PC):
//! - `core_public__.yaml` (one per folder): graphics, main volumes, the client window's spot
//!   (`WindowedSettings`: left/top/width/height) or fixed size, UI scale.
//! - `core_user_<account>.dat` (one per account per folder): `cmd.customCmds` (shortcuts),
//!   `overview`, `ui` (effects, camera…), `audio` (advanced sliders).
//! - `core_char_<character>.dat` (one per character per folder): `windows` — every window's
//!   `(left, top, width, height, client width, client height)`, so a layout says which client
//!   size it was arranged for.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::blue::{self, unstamped, Blue};
use crate::profiles::{entries_in, profile_of, root, settings_file, Kind};

/// The Effects ticks in EVE's Display & Graphics tab (as stored in the account file's `ui`).
const EFFECTS: &[&str] = &[
    "turretsEnabled",
    "effectsEnabled",
    "missilesEnabled",
    "explosionEffectsEnabled",
    "droneModelsEnabled",
    "trailsEnabled",
    "gpuParticlesEnabled",
    "modelSkinsInSpaceEnabled",
];

/// Where the client opens: windowed at a spot, or a fixed window / fullscreen at a size.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Seat {
    /// 0 fullscreen, 1 window, 2 fixed window (EVE's WindowMode).
    pub mode: Option<i64>,
    pub left: Option<i64>,
    pub top: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fixed_width: Option<i64>,
    pub fixed_height: Option<i64>,
    pub ui_scale: Option<f64>,
}

/// `  Key: [stamp, value]` inside `device:` -> value text.
fn device_value<'a>(yaml: &'a str, key: &str) -> Option<&'a str> {
    let mut in_device = false;
    for line in yaml.lines() {
        if !line.starts_with(' ') {
            in_device = line.trim_end() == "device:";
            continue;
        }
        if in_device {
            if let Some(rest) = line.strip_prefix(&format!("  {key}: [")) {
                return rest.trim_end().trim_end_matches(']').split_once(", ").map(|(_, v)| v.trim());
            }
        }
    }
    None
}

/// The `{adapter: 0, height: 1024, left: -7, …}` block under `  Key:` in `device:` (it can wrap).
fn device_block(yaml: &str, key: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    let mut in_device = false;
    let mut grabbing = false;
    let mut text = String::new();
    for line in yaml.lines() {
        if !line.starts_with(' ') {
            if grabbing {
                break;
            }
            in_device = line.trim_end() == "device:";
            continue;
        }
        if !in_device {
            continue;
        }
        if grabbing {
            if line.starts_with("  ") && !line.starts_with("  -") && !line.starts_with("   ") {
                break;
            }
            text.push_str(line.trim());
            text.push(' ');
        } else if line.trim_end() == format!("  {key}:") {
            grabbing = true;
        }
    }
    if let (Some(open), Some(close)) = (text.find('{'), text.rfind('}')) {
        for pair in text[open + 1..close].split(',') {
            if let Some((k, v)) = pair.split_once(':') {
                if let Ok(n) = v.trim().parse::<i64>() {
                    out.insert(k.trim().to_string(), n);
                }
            }
        }
    }
    out
}

pub fn seat_of(yaml: &str) -> Option<Seat> {
    if yaml.trim().is_empty() {
        return None;
    }
    let windowed = device_block(yaml, "WindowedSettings");
    let fixed = device_block(yaml, "FixedWindowSettings");
    Some(Seat {
        mode: device_value(yaml, "WindowMode").and_then(|v| v.parse().ok()),
        left: windowed.get("left").copied(),
        top: windowed.get("top").copied(),
        width: windowed.get("width").copied(),
        height: windowed.get("height").copied(),
        fixed_width: fixed.get("width").copied(),
        fixed_height: fixed.get("height").copied(),
        ui_scale: device_value(yaml, "UIScaleWindowed").and_then(|v| v.parse().ok()),
    })
}

#[derive(Debug, Serialize)]
struct AccountLook {
    id: u64,
    file: String,
    modified: u64,
    error: Option<String>,
    keys: usize,
    keys_fp: String,
    overview_fp: String,
    overview_preset: String,
    effects: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct CharacterLook {
    id: u64,
    file: String,
    modified: u64,
    error: Option<String>,
    /// How many windows are open in this layout.
    open_windows: usize,
    /// [client width, client height, open windows arranged at that size], biggest first.
    sizes: Vec<(i64, i64, usize)>,
    layout_fp: String,
}

#[derive(Debug, Serialize)]
struct FolderLook {
    name: String,
    seat: Option<Seat>,
    accounts: Vec<AccountLook>,
    characters: Vec<CharacterLook>,
}

fn read_blue(path: &Path) -> Result<Blue, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    blue::read(&bytes)
}

fn section<'a>(root: &'a Blue, name: &str) -> Option<&'a Blue> {
    root.get(name).map(unstamped)
}

fn account_look(dir: &Path, file: &str, id: u64, modified: u64) -> AccountLook {
    let mut look = AccountLook {
        id,
        file: file.to_string(),
        modified,
        error: None,
        keys: 0,
        keys_fp: String::new(),
        overview_fp: String::new(),
        overview_preset: String::new(),
        effects: BTreeMap::new(),
    };
    match read_blue(&dir.join(file)) {
        Err(e) => look.error = Some(e),
        Ok(root) => {
            if let Some(keys) = section(&root, "cmd").and_then(|c| c.get("customCmds")).map(unstamped) {
                look.keys = keys.entries().len();
                look.keys_fp = keys.fingerprint();
            }
            if let Some(overview) = section(&root, "overview") {
                look.overview_fp = overview.fingerprint();
                look.overview_preset = overview.get("activeOverviewPreset").map(|v| unstamped(v).text()).unwrap_or_default();
            }
            if let Some(ui) = section(&root, "ui") {
                for key in EFFECTS {
                    if let Some(v) = ui.get(key) {
                        look.effects.insert(key.to_string(), unstamped(v).text());
                    }
                }
            }
        }
    }
    look
}

/// The open windows of a character file and the client sizes they were placed at.
fn layout(root: &Blue) -> (Vec<(String, &Blue)>, BTreeMap<(i64, i64), usize>) {
    let mut open: Vec<(String, &Blue)> = Vec::new();
    let mut sizes = BTreeMap::new();
    let Some(windows) = section(root, "windows") else { return (open, sizes) };
    let is_open: std::collections::HashSet<String> = windows
        .get("openWindows")
        .map(unstamped)
        .map(|o| o.entries().iter().filter(|(_, v)| matches!(unstamped(v), Blue::Bool(true))).map(|(k, _)| k.text()).collect())
        .unwrap_or_default();
    if let Some(places) = windows.get("windowSizesAndPositions_1").map(unstamped) {
        for (key, value) in places.entries() {
            let name = key.text();
            if !is_open.contains(&name) {
                continue;
            }
            let place = unstamped(value);
            if let Blue::Tuple(parts) | Blue::List(parts) = place {
                if let (Some(Blue::Int(w)), Some(Blue::Int(h))) = (parts.get(4), parts.get(5)) {
                    *sizes.entry((*w, *h)).or_insert(0) += 1;
                }
            }
            open.push((name, place));
        }
    }
    open.sort_by(|a, b| a.0.cmp(&b.0));
    (open, sizes)
}

fn character_look(dir: &Path, file: &str, id: u64, modified: u64) -> CharacterLook {
    let mut look = CharacterLook { id, file: file.to_string(), modified, error: None, open_windows: 0, sizes: Vec::new(), layout_fp: String::new() };
    match read_blue(&dir.join(file)) {
        Err(e) => look.error = Some(e),
        Ok(root) => {
            let (open, sizes) = layout(&root);
            look.open_windows = open.len();
            look.layout_fp = Blue::List(open.iter().map(|(n, p)| Blue::Tuple(vec![Blue::Str(n.clone()), (*p).clone()])).collect()).fingerprint();
            look.sizes = sizes.into_iter().map(|((w, h), n)| (w, h, n)).collect();
            look.sizes.sort_by(|a, b| b.2.cmp(&a.2));
        }
    }
    look
}

/// Every folder: its seat, its accounts (keys, overview, effects) and its character layouts.
#[tauri::command]
pub fn setups_look() -> Result<serde_json::Value, String> {
    let root = root()?;
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let Some(name) = profile_of(&dir_name) else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let yaml = std::fs::read_to_string(dir.join("core_public__.yaml")).unwrap_or_default();
        let mut accounts = Vec::new();
        let mut characters = Vec::new();
        for file in entries_in(&dir, settings_file) {
            match file.kind {
                Kind::User => accounts.push(account_look(&dir, &file.file, file.id, file.modified)),
                Kind::Char => characters.push(character_look(&dir, &file.file, file.id, file.modified)),
            }
        }
        folders.push(FolderLook { name: name.to_string(), seat: seat_of(&yaml), accounts, characters });
    }
    folders.sort_by(|a, b| match (a.name.parse::<u32>(), b.name.parse::<u32>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(serde_json::json!({ "folders": folders, "clientsOpen": crate::profiles::clients_open() }))
}

/// One readable line per setting: (part, setting, value).
fn rows_of(kind: Kind, root: &Blue) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    let mut add = |part: &str, key: String, value: String| rows.push((part.to_string(), key, value));
    let short = |v: &Blue| {
        let text = v.text();
        if text.chars().count() <= 80 { text } else { format!("#{}", v.fingerprint()) }
    };
    match kind {
        Kind::User => {
            if let Some(keys) = section(root, "cmd").and_then(|c| c.get("customCmds")).map(unstamped) {
                for (k, v) in keys.entries() {
                    add("Keys", k.text(), v.text());
                }
            }
            if let Some(overview) = section(root, "overview") {
                for (k, v) in overview.entries() {
                    add("Overview", k.text(), short(unstamped(v)));
                }
            }
            for (part, name) in [("Effects, camera & interface", "ui"), ("Audio", "audio")] {
                if let Some(sec) = section(root, name) {
                    for (k, v) in sec.entries() {
                        add(part, k.text(), short(unstamped(v)));
                    }
                }
            }
        }
        Kind::Char => {
            let (open, _) = layout(root);
            for (name, place) in open {
                add("Open windows", name, place.text());
            }
            if let Some(sec) = section(root, "ui") {
                for (k, v) in sec.entries() {
                    add("Character options", k.text(), short(unstamped(v)));
                }
            }
        }
    }
    rows
}

/// Read chosen files ("settings_2/core_user_10979282.dat" or a template's name) as rows to compare.
#[tauri::command]
pub fn setups_compare(files: Vec<String>) -> Result<Vec<serde_json::Value>, String> {
    let root = root()?;
    if files.len() > 12 {
        return Err("Compare up to 12 at a time.".into());
    }
    let mut out = Vec::new();
    for spec in files {
        let (path, kind) = crate::profiles::resolve(&root, &spec)?;
        match read_blue(&path) {
            Ok(value) => out.push(serde_json::json!({ "file": spec, "rows": rows_of(kind, &value) })),
            Err(e) => out.push(serde_json::json!({ "file": spec, "error": e })),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "audio:\n  masterVolume: [1, 0.5]\ndevice:\n  FixedWindow: [1, 0]\n  FixedWindowSettings:\n  - 134394192932409075\n  - {adapter: 0, height: 1056, presentInterval: 0, width: 1920}\n  UIScaleWindowed: [1, 0.9]\n  WindowMode: [1, 1]\n  WindowedResolution: [1, null]\n  WindowedSettings:\n  - 134394164690872917\n  - {adapter: 0, height: 1024, left: -7, presentInterval: 0, showState: 0, top: 1056,\n    width: 1918}\n  antiAliasing: [1, 0]\ngeneric: {}\n";

    #[test]
    fn reads_where_the_client_opens() {
        let seat = seat_of(YAML).unwrap();
        assert_eq!(seat.mode, Some(1));
        assert_eq!((seat.left, seat.top, seat.width, seat.height), (Some(-7), Some(1056), Some(1918), Some(1024)));
        assert_eq!((seat.fixed_width, seat.fixed_height), (Some(1920), Some(1056)));
        assert_eq!(seat.ui_scale, Some(0.9));
    }

    #[test]
    fn missing_spot_is_none_not_a_guess() {
        let seat = seat_of("device:\n  WindowMode: [1, 0]\n  WindowedSettings: [1, null]\n  FixedWindowSettings: [1, null]\n").unwrap();
        assert_eq!(seat.mode, Some(0));
        assert_eq!(seat.left, None);
        assert_eq!(seat.fixed_width, None);
        assert!(seat_of("").is_none());
    }

    #[test]
    fn open_windows_and_their_client_size() {
        let stamp = Blue::Int(134_394_328_018_117_906);
        let place = |l, w| Blue::Tuple(vec![Blue::Int(l), Blue::Int(0), Blue::Int(300), Blue::Int(200), Blue::Int(w), Blue::Int(1024)]);
        let windows = Blue::Dict(vec![
            (Blue::Str("openWindows".into()), Blue::Tuple(vec![stamp.clone(), Blue::Dict(vec![
                (Blue::Str("overview".into()), Blue::Bool(true)),
                (Blue::Str("market".into()), Blue::Bool(false)),
                (Blue::Str("dscan".into()), Blue::Bool(true)),
            ])])),
            (Blue::Str("windowSizesAndPositions_1".into()), Blue::Tuple(vec![stamp, Blue::Dict(vec![
                (Blue::Str("overview".into()), place(10, 1918)),
                (Blue::Str("market".into()), place(20, 1918)),
                (Blue::Str("dscan".into()), place(30, 2131)),
            ])])),
        ]);
        let root = Blue::Dict(vec![(Blue::Str("windows".into()), windows)]);
        let (open, sizes) = layout(&root);
        assert_eq!(open.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["dscan", "overview"]);
        assert_eq!(sizes.get(&(1918, 1024)), Some(&1));
        assert_eq!(sizes.get(&(2131, 1024)), Some(&1));
        let rows = rows_of(Kind::Char, &root);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Open windows");
    }
}
