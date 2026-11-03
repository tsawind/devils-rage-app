//! Patch 69: client tuning. Sound, graphics and interface levers for EVE clients, set per client
//! profile (`settings_<n>`), with presets for multiboxing (main, alts, hole watchers).
//!
//! EVE keeps these in each profile's `core_public__.yaml` as `key: [stamp, value]` under `audio:`,
//! `device:` and `ui:` (the stamp is a Windows file time). Only the keys listed in [`LEVERS`] are
//! ever touched; everything else in the file is left exactly as it was. `prefs.ini` holds mostly
//! internal client switches; the one player-facing one ([`PREFS`]) can be switched too.
//!
//! The client rewrites these files when it closes, so changes are refused while clients are open
//! (unless you insist), and everything is backed up first (profiles.rs).

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One lever: section in the yaml, key, what kind of value, and its plain-words name.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Lever {
    pub section: &'static str,
    pub key: &'static str,
    /// "volume" (0 … 1), "switch" (0 / 1), "bool" (false / true), "level" (whole number), "scale" (0.5 … 2)
    pub kind: &'static str,
    pub label: &'static str,
}

pub const LEVERS: &[Lever] = &[
    // Sound: keep what matters (wormhole activations are world sound, alarms are UI and Aura).
    Lever { section: "audio", key: "masterVolume", kind: "volume", label: "Master volume" },
    Lever { section: "audio", key: "worldVolume", kind: "volume", label: "World (ships, gates, wormhole activations)" },
    Lever { section: "audio", key: "uiGain", kind: "volume", label: "Interface (alarms, warnings, notifications)" },
    Lever { section: "audio", key: "evevoiceGain", kind: "volume", label: "Aura voice (warnings)" },
    Lever { section: "audio", key: "eveampGain", kind: "volume", label: "Music" },
    Lever { section: "audio", key: "useCombatMusic", kind: "switch", label: "Combat music" },
    Lever { section: "audio", key: "suppressTurret", kind: "switch", label: "Quiet turret sounds" },
    // Graphics: lower = lighter on the PC (alts don't need the pretty).
    Lever { section: "device", key: "textureQuality", kind: "level", label: "Texture quality (0 high · 1 medium · 2 low)" },
    Lever { section: "device", key: "shaderQuality", kind: "level", label: "Shader quality (1 low · 2 medium · 3 high)" },
    Lever { section: "device", key: "lodQuality", kind: "level", label: "Level of detail (1 low · 2 medium · 3 high)" },
    Lever { section: "device", key: "shadowQuality", kind: "level", label: "Shadows (0 off · 1 low · 2 high)" },
    Lever { section: "device", key: "reflectionQuality", kind: "level", label: "Reflections (0 off … 3 high)" },
    Lever { section: "device", key: "aoQuality", kind: "level", label: "Ambient occlusion (0 off … 2 high)" },
    Lever { section: "device", key: "postProcessingQuality", kind: "level", label: "Post-processing (0 off … 2 high)" },
    Lever { section: "device", key: "volumetricQuality", kind: "level", label: "Volumetrics, nebula clouds (0 low … 2 high)" },
    Lever { section: "device", key: "antiAliasing", kind: "level", label: "Anti-aliasing (0 off … 3 high)" },
    Lever { section: "device", key: "charTextureQuality", kind: "level", label: "Character textures (0 high … 2 low)" },
    Lever { section: "device", key: "dofEnabled", kind: "switch", label: "Depth of field" },
    Lever { section: "device", key: "charClothSimulation", kind: "switch", label: "Cloth simulation (character screen)" },
    Lever { section: "device", key: "frameGeneration", kind: "bool", label: "Frame generation" },
    Lever { section: "device", key: "brightness", kind: "scale", label: "Brightness" },
    // Interface.
    Lever { section: "device", key: "UIScaleWindowed", kind: "scale", label: "Interface scale (windowed)" },
    Lever { section: "device", key: "UIScaleFullscreen", kind: "scale", label: "Interface scale (full screen)" },
    Lever { section: "ui", key: "clientFontSize", kind: "level", label: "Font size (0 small · 1 medium · 2 large)" },
];

/// prefs.ini switches worth offering (the rest of that file is the client's own plumbing).
pub const PREFS: &[(&str, &str)] = &[
    ("bracketsAlwaysShowShipText", "Always show ship names on brackets"),
    ("breakpadUpload", "Send crash reports to CCP"),
];

pub fn lever(key: &str) -> Option<&'static Lever> {
    LEVERS.iter().find(|l| l.key == key)
}

/// A value written the way EVE writes it ("0.7", "1", "false"); None when it doesn't fit the lever.
pub fn format_value(lever: &Lever, value: &serde_json::Value) -> Option<String> {
    match lever.kind {
        "volume" => value.as_f64().filter(|v| (0.0..=1.0).contains(v)).map(|v| trim_float(v)),
        "scale" => value.as_f64().filter(|v| (0.5..=2.0).contains(v)).map(|v| trim_float(v)),
        "switch" => value.as_i64().filter(|v| *v == 0 || *v == 1).map(|v| v.to_string()),
        "level" => value.as_i64().filter(|v| (0..=4).contains(v)).map(|v| v.to_string()),
        "bool" => value.as_bool().map(|v| v.to_string()),
        _ => None,
    }
}

fn trim_float(v: f64) -> String {
    let text = format!("{:.2}", (v * 100.0).round() / 100.0);
    let text = text.trim_end_matches('0');
    if text.ends_with('.') { format!("{text}0") } else { text.to_string() }
}

/// Now as a Windows file time (100 ns since 1601), the stamp EVE puts next to each value.
pub fn file_time_now() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    (unix / 100) as u64 + 116_444_736_000_000_000
}

/// The current values of every lever in a core_public__.yaml (as the text after the stamp).
pub fn read_values(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = "";
    for line in text.lines() {
        if !line.starts_with(' ') {
            section = line.split(':').next().unwrap_or("");
            continue;
        }
        let trimmed = line.trim_start();
        let Some((key, rest)) = trimmed.split_once(": [") else { continue };
        if let Some(l) = lever(key).filter(|l| l.section == section) {
            if let Some((_, value)) = rest.trim_end_matches(']').split_once(", ") {
                out.insert(l.key.to_string(), value.trim().to_string());
            }
        }
    }
    out
}

/// Set one lever in a core_public__.yaml text. Replaces `  key: [stamp, value]` in its section,
/// or adds it at the top of the section (making the section when it's missing or `{}`).
pub fn set_value(text: &str, lever: &Lever, value: &str, stamp: u64) -> String {
    let line_for = format!("  {}: [{stamp}, {value}]", lever.key);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut section_at: Option<usize> = None;
    let mut in_section = false;
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with(' ') {
            in_section = line == &format!("{}:", lever.section) || line == &format!("{}: {{}}", lever.section);
            if in_section {
                section_at = Some(i);
            }
            continue;
        }
        if in_section && line.trim_start().starts_with(&format!("{}: [", lever.key)) && line.starts_with("  ") && !line.starts_with("   ") {
            lines[i] = line_for;
            return join(lines, text);
        }
    }
    match section_at {
        Some(i) => {
            lines[i] = format!("{}:", lever.section);
            lines.insert(i + 1, line_for);
        }
        None => {
            lines.push(format!("{}:", lever.section));
            lines.push(line_for);
        }
    }
    join(lines, text)
}

fn join(lines: Vec<String>, original: &str) -> String {
    let mut out = lines.join("\n");
    if original.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Set a `key=value` line in prefs.ini (added when missing).
pub fn set_pref(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.split('=').next().map(str::trim) == Some(key) {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{key}={value}"));
    }
    lines.join("\n")
}

pub fn read_prefs(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(k, _)| PREFS.iter().any(|(p, _)| p == &k.trim()))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Apply levers and prefs to one profile folder. Returns how many values changed.
pub fn apply_to(dir: &Path, values: &BTreeMap<String, serde_json::Value>, prefs: &BTreeMap<String, bool>, stamp: u64) -> Result<usize, String> {
    let mut changed = 0;
    if !values.is_empty() {
        let path = dir.join("core_public__.yaml");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        let before = read_values(&text);
        for (key, value) in values {
            let lever = lever(key).ok_or_else(|| format!("{key} isn't a setting we change"))?;
            let formatted = format_value(lever, value).ok_or_else(|| format!("{} can't be {value}", lever.label))?;
            if before.get(key) != Some(&formatted) {
                text = set_value(&text, lever, &formatted, stamp);
                changed += 1;
            }
        }
        if changed > 0 {
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    if !prefs.is_empty() {
        let path = dir.join("prefs.ini");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        let before = read_prefs(&text);
        let mut any = false;
        for (key, on) in prefs {
            if !PREFS.iter().any(|(k, _)| k == key) {
                return Err(format!("{key} isn't a prefs.ini switch we change"));
            }
            let value = if *on { "1" } else { "0" };
            if before.get(key).map(String::as_str) != Some(value) {
                text = set_pref(&text, key, value);
                changed += 1;
                any = true;
            }
        }
        if any {
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "audio:\n  eveampGain: [134381185702295163, 0.0]\n  masterVolume: [134381185702295163, 0.7]\ndevice:\n  FullScreenSettings:\n  - 134381185153995013\n  - {adapter: 0, height: 2160, presentInterval: 0, width: 3840}\n  shadowQuality: [134381370407820373, 0]\n  textureQuality: [134381370407820373, 2]\ngeneric: {}\nui:\n  clientFontSize: [134381370407655102, 1]\n";

    #[test]
    fn reads_the_levers() {
        let v = read_values(YAML);
        assert_eq!(v.get("masterVolume").map(String::as_str), Some("0.7"));
        assert_eq!(v.get("textureQuality").map(String::as_str), Some("2"));
        assert_eq!(v.get("clientFontSize").map(String::as_str), Some("1"));
        assert!(v.get("FullScreenSettings").is_none());
    }

    #[test]
    fn replaces_a_value_and_keeps_everything_else() {
        let out = set_value(YAML, lever("masterVolume").unwrap(), "0.5", 9);
        assert!(out.contains("  masterVolume: [9, 0.5]\n"));
        assert!(out.contains("  - {adapter: 0, height: 2160, presentInterval: 0, width: 3840}\n"));
        assert_eq!(out.lines().count(), YAML.lines().count());
    }

    #[test]
    fn adds_a_missing_key_to_its_section() {
        let out = set_value(YAML, lever("worldVolume").unwrap(), "0.8", 9);
        assert!(out.starts_with("audio:\n  worldVolume: [9, 0.8]\n"));
        let out = set_value("generic: {}\n", lever("clientFontSize").unwrap(), "2", 9);
        assert_eq!(out, "generic: {}\nui:\n  clientFontSize: [9, 2]\n");
        let out = set_value("ui: {}\n", lever("clientFontSize").unwrap(), "2", 9);
        assert_eq!(out, "ui:\n  clientFontSize: [9, 2]\n");
    }

    #[test]
    fn values_must_fit_the_lever() {
        let vol = lever("masterVolume").unwrap();
        assert_eq!(format_value(vol, &serde_json::json!(0.7)).as_deref(), Some("0.7"));
        assert_eq!(format_value(vol, &serde_json::json!(0)).as_deref(), Some("0.0"));
        assert_eq!(format_value(vol, &serde_json::json!(1.5)), None);
        assert_eq!(format_value(lever("useCombatMusic").unwrap(), &serde_json::json!(2)), None);
        assert_eq!(format_value(lever("frameGeneration").unwrap(), &serde_json::json!(false)).as_deref(), Some("false"));
    }

    #[test]
    fn prefs_lines_are_switched_or_added() {
        let text = "bracketsAlwaysShowShipText=0\nport=26000";
        assert_eq!(set_pref(text, "bracketsAlwaysShowShipText", "1"), "bracketsAlwaysShowShipText=1\nport=26000");
        assert_eq!(set_pref("port=26000", "breakpadUpload", "0"), "port=26000\nbreakpadUpload=0");
        assert_eq!(read_prefs(text).get("bracketsAlwaysShowShipText").map(String::as_str), Some("0"));
        assert!(read_prefs(text).get("port").is_none());
    }

    #[test]
    fn file_time_is_after_2026() {
        assert!(file_time_now() > 134_000_000_000_000_000);
    }
}
