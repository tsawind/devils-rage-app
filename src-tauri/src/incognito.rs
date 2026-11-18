//! Patch 74: incognito by default. The app uses every character and account it finds on this PC
//! (EVE's files, open clients), but only characters the corp's sites already know about ever
//! leave it: the ones on your SeAT accounts (Your accounts) and the ones linked to your mapper
//! login (the mapper page tells us). Everything else (spies, extra alts, someone else's account
//! logged in once) is incognito: previews, bars, stacks and Setups work as usual, but its name,
//! id and location are never handed to the mapper or SeAT. You can also hide a known character
//! by hand (incognito.json, this PC only).
//!
//! Every place that hands a character to a web page goes through `is_public_name` /
//! `is_public_id` (fail closed: nothing known yet means nothing goes out).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    /// Characters the mapper says are linked to your login (id, name).
    #[serde(default)]
    mapper: Vec<(u64, String)>,
    /// Characters hidden by hand on this PC (names).
    #[serde(default)]
    hidden: Vec<String>,
}

#[derive(Default)]
struct Known {
    ids: HashSet<u64>,
    /// id → name (lower case), to hide by name.
    id_names: HashMap<u64, String>,
    names: HashSet<String>,
    hidden: HashSet<String>,
    saved: Saved,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

fn file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("incognito.json"))
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Rebuild what's public from SeAT's copy (seat-groups.json) and the mapper's list.
pub fn load(app: &AppHandle) {
    let saved: Saved = file(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let seat = crate::launch::load_seat(app);
    let mut known = Known::default();
    for character in seat.accounts.iter().flat_map(|a| a.characters.iter()) {
        known.ids.insert(character.id);
        known.id_names.insert(character.id, key(&character.name));
        known.names.insert(key(&character.name));
    }
    for (id, name) in &saved.mapper {
        known.ids.insert(*id);
        known.id_names.insert(*id, key(name));
        known.names.insert(key(name));
    }
    known.hidden = saved.hidden.iter().map(|n| key(n)).collect();
    known.saved = saved;
    log::info!("incognito: {} characters known to SeAT / the mapper, {} hidden by hand", known.names.len(), known.hidden.len());
    *KNOWN.lock().unwrap() = Some(known);
}

fn save(app: &AppHandle, saved: &Saved) {
    if let (Some(path), Ok(text)) = (file(app), serde_json::to_string_pretty(saved)) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

/// The mapper page told us which characters are linked to your mapper login.
pub fn from_mapper(app: &AppHandle, characters: Vec<(u64, String)>) {
    let characters: Vec<(u64, String)> = characters.into_iter().filter(|(id, name)| *id > 0 && !name.trim().is_empty()).take(200).collect();
    if characters.is_empty() {
        return;
    }
    let changed = {
        let guard = KNOWN.lock().unwrap();
        guard.as_ref().is_none_or(|k| {
            let mut now = characters.clone();
            let mut before = k.saved.mapper.clone();
            now.sort();
            before.sort();
            now != before
        })
    };
    if !changed {
        return;
    }
    let mut saved = KNOWN.lock().unwrap().as_ref().map(|k| k.saved.clone()).unwrap_or_default();
    saved.mapper = characters;
    save(app, &saved);
    load(app);
    // Locations held back while these weren't known yet.
    crate::location::resend_all(app);
}

/// Whether this character may be named to the mapper or SeAT.
pub fn is_public_name(name: &str) -> bool {
    let name = key(name);
    KNOWN.lock().unwrap().as_ref().is_some_and(|k| !k.hidden.contains(&name) && k.names.contains(&name))
}

/// Whether this character id may be handed to the mapper or SeAT.
pub fn is_public_id(id: u64) -> bool {
    let guard = KNOWN.lock().unwrap();
    let Some(known) = guard.as_ref() else { return false };
    if !known.ids.contains(&id) {
        return false;
    }
    // Hidden by hand: by name, so look the id up.
    !known.id_names.get(&id).is_some_and(|n| known.hidden.contains(n))
}

/// Whether a character is incognito (shown in the app's own pages).
#[tauri::command]
pub fn incognito_state() -> serde_json::Value {
    let guard = KNOWN.lock().unwrap();
    let (known, hidden): (Vec<String>, Vec<String>) = guard.as_ref().map(|k| (k.names.iter().cloned().collect(), k.hidden.iter().cloned().collect())).unwrap_or_default();
    serde_json::json!({ "known": known, "hidden": hidden })
}

/// Hide a known character by hand (or show it again). This PC only.
#[tauri::command]
pub fn incognito_hide(app: AppHandle, name: String, hidden: bool) {
    let mut saved = KNOWN.lock().unwrap().as_ref().map(|k| k.saved.clone()).unwrap_or_default();
    saved.hidden.retain(|n| key(n) != key(&name));
    if hidden {
        saved.hidden.push(name.trim().to_string());
    }
    save(&app, &saved);
    load(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_characters_are_public() {
        let mut known = Known::default();
        known.ids.insert(90_000_001);
        known.names.insert(key("Tsawind"));
        known.id_names.insert(90_000_001, key("Tsawind"));
        *KNOWN.lock().unwrap() = Some(known);
        assert!(is_public_name("Tsawind") && is_public_name(" tsawind "));
        assert!(!is_public_name("Martha Scepter"));
        assert!(is_public_id(90_000_001) && !is_public_id(90_000_002));
        // Hidden by hand: no longer public, by name or id.
        KNOWN.lock().unwrap().as_mut().unwrap().hidden.insert(key("Tsawind"));
        assert!(!is_public_name("Tsawind") && !is_public_id(90_000_001));
        // Nothing known yet: nothing goes out.
        *KNOWN.lock().unwrap() = None;
        assert!(!is_public_name("Tsawind") && !is_public_id(90_000_001));
    }
}
