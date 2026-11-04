//! Patch 70: "New launch group" in one place. Pick characters, a baseline and a preset; the app
//! makes the launch group in Your accounts, a **new client profile folder of its own**
//! (`settings_<Name>`) built from the baseline, tunes its sound and graphics, sets the pedal keys,
//! and says what to set in the EVE launcher. Nothing in anyone's existing profiles is touched:
//! the old `settings_N` folders stay exactly as they were.
//!
//! A folder made this way carries a `.devilsrage-group` note, so the app only ever removes
//! folders it made (and even then moves them aside rather than deleting them).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::profiles::{settings_file, Kind};

const MARKER: &str = ".devilsrage-group";

#[derive(Debug, Deserialize)]
pub struct NewGroup {
    pub name: String,
    pub character_ids: Vec<u64>,
    pub baseline_id: u64,
    /// "baseline": every character gets the baseline's windows, overview and module layout.
    /// "own": each character starts from its own settings (keeps its module placement).
    pub characters_from: String,
    #[serde(default)]
    pub values: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub prefs: BTreeMap<String, bool>,
    #[serde(default)]
    pub forward_hotkey: Option<String>,
    #[serde(default)]
    pub backward_hotkey: Option<String>,
    #[serde(default)]
    pub login_screens: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    name: String,
    group_id: Option<u64>,
    created: u64,
}

/// "Roll Fleet #2" -> "RollFleet2" (what the launcher's client profile box shows).
pub fn folder_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(30).collect()
}

/// The newest account file in a profile folder (the account that ran there last).
fn newest_user(dir: &Path) -> Option<(PathBuf, u64)> {
    crate::profiles::entries_in(dir, settings_file).into_iter().find(|e| e.kind == Kind::User).map(|e| (dir.join(&e.file), e.id))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))
}

/// Make it all. Returns what the EVE launcher needs: the profile name and the accounts.
#[tauri::command]
pub async fn group_setup_create(app: AppHandle, setup: NewGroup) -> Result<serde_json::Value, String> {
    let name = setup.name.trim().to_string();
    let folder = folder_name(&name);
    if folder.is_empty() {
        return Err("Give the group a name with some letters or numbers in it.".into());
    }
    if setup.character_ids.is_empty() {
        return Err("Pick at least one character.".into());
    }
    if !setup.character_ids.contains(&setup.baseline_id) {
        return Err("The baseline has to be one of the picked characters.".into());
    }

    // Fresh accounts and characters from SeAT (which account each character is on, and its profile).
    let seat = crate::launch::fetch_seat(&app).await?;
    crate::launch::store_seat(&app, &seat)?;
    let account_of = |id: u64| seat.accounts.iter().find(|a| a.characters.iter().any(|c| c.id == id));
    let mut per_account: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for id in &setup.character_ids {
        let account = account_of(*id).ok_or_else(|| format!("Character {id} isn't in Your accounts."))?;
        per_account.entry(account.nickname.clone()).or_default().push(*id);
    }
    if let Some((nick, ids)) = per_account.iter().find(|(_, ids)| ids.len() > 1) {
        return Err(format!("Account {nick} has {} characters picked: the launcher starts one character per account.", ids.len()));
    }

    let root = crate::profiles::root()?;
    let target = root.join(format!("settings_{folder}"));
    if target.exists() && !target.join(MARKER).is_file() {
        return Err(format!("settings_{folder} already exists and wasn't made by the app: pick another name so nothing of yours is overwritten."));
    }
    let profile_dir = |id: u64| -> Option<PathBuf> {
        let profile = account_of(id)?.profile.clone()?;
        let dir = root.join(format!("settings_{profile}"));
        dir.is_dir().then_some(dir)
    };
    let base_dir = profile_dir(setup.baseline_id).ok_or("The baseline's account has no client profile folder yet: start that account once with its profile number, then try again.")?;
    let base_char = base_dir.join(format!("core_char_{}.dat", setup.baseline_id));
    if !base_char.is_file() {
        return Err("The baseline character has no settings yet: log it in once, set it up, log out, then try again.".into());
    }
    let base_user = newest_user(&base_dir);

    // Build it in a scratch folder first, then put it in place (half a profile never shows up).
    let building = root.join(format!(".building_{folder}"));
    let _ = std::fs::remove_dir_all(&building);
    std::fs::create_dir_all(&building).map_err(|e| e.to_string())?;
    let result = (|| -> Result<Vec<serde_json::Value>, String> {
        let mut accounts = Vec::new();
        for (nick, ids) in &per_account {
            let id = ids[0];
            let own_dir = profile_dir(id);
            let from_char = match (setup.characters_from.as_str(), &own_dir) {
                ("own", Some(dir)) if dir.join(format!("core_char_{id}.dat")).is_file() => dir.join(format!("core_char_{id}.dat")),
                _ => base_char.clone(),
            };
            copy(&from_char, &building.join(format!("core_char_{id}.dat")))?;
            // The account file (keys, shortcuts): the baseline's, under this account's own number.
            if let Some((_, user_id)) = own_dir.as_deref().and_then(newest_user) {
                let from_user = match (setup.characters_from.as_str(), &own_dir) {
                    ("own", Some(dir)) => newest_user(dir).map(|(p, _)| p),
                    _ => base_user.as_ref().map(|(p, _)| p.clone()),
                };
                if let Some(from_user) = from_user {
                    copy(&from_user, &building.join(format!("core_user_{user_id}.dat")))?;
                }
            }
            let names: Vec<String> = seat.accounts.iter().find(|a| &a.nickname == nick).map(|a| a.characters.iter().filter(|c| ids.contains(&c.id)).map(|c| c.name.clone()).collect()).unwrap_or_default();
            accounts.push(serde_json::json!({ "account": nick, "characters": names }));
        }
        for file in ["core_public__.yaml", "prefs.ini"] {
            let from = base_dir.join(file);
            if from.is_file() {
                copy(&from, &building.join(file))?;
            }
        }
        crate::tuning::apply_to(&building, &setup.values, &setup.prefs, crate::tuning::file_time_now())?;
        Ok(accounts)
    })();
    let accounts = match result {
        Ok(accounts) => accounts,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&building);
            return Err(e);
        }
    };

    // The launch group in Your accounts.
    let group_id = save_seat_group(&app, &name, &setup.character_ids).await?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    std::fs::write(building.join(MARKER), serde_json::to_string(&Marker { name: name.clone(), group_id: Some(group_id), created: stamp }).unwrap_or_default()).map_err(|e| e.to_string())?;
    if target.exists() {
        let aside = root.join(format!("Devils Rage replaced settings_{folder} {stamp}"));
        std::fs::rename(&target, &aside).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&building, &target).map_err(|e| e.to_string())?;

    // This PC's pedal keys for it, and the fresh list.
    crate::launch::save_seat_group_prefs(
        app.clone(),
        group_id,
        crate::launch::GroupPrefs { forward_hotkey: setup.forward_hotkey, backward_hotkey: setup.backward_hotkey, skipped: Vec::new(), login_screens: setup.login_screens, ..Default::default() },
    )?;
    let _ = crate::launch::seat_refresh(app.clone()).await;
    log::info!("new launch group {name}: settings_{folder}, {} characters", setup.character_ids.len());

    Ok(serde_json::json!({ "groupId": group_id, "profile": folder, "accounts": accounts }))
}

async fn save_seat_group(app: &AppHandle, name: &str, ids: &[u64]) -> Result<u64, String> {
    let (cookies, xsrf) = crate::launch::seat_session(app)?;
    let xsrf = xsrf.ok_or("SeAT's session is missing: open Your accounts once, then try again")?;
    let response = reqwest::Client::new()
        .post(format!("{}/devilsrage/launch-groups/app", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&serde_json::json!({ "name": name, "character_ids": ids }))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("SeAT answered {status}")));
    }
    body["id"].as_u64().ok_or_else(|| "SeAT didn't say which group it made".to_string())
}

/// Change a group made here, in place: characters added get their settings (their own or the
/// baseline's), characters taken out leave the folder, the sound & graphics you ticked are
/// written, and Your accounts gets the new member list. Everything a client saved in the folder
/// since (window layouts while playing) is kept. A copy of the folder is put aside first.
#[tauri::command]
pub async fn group_setup_update(app: AppHandle, profile: String, setup: NewGroup) -> Result<String, String> {
    if !crate::profiles::safe_name(&profile) {
        return Err("not a profile".into());
    }
    let root = crate::profiles::root()?;
    let dir = root.join(format!("settings_{profile}"));
    let mut marker: Marker = std::fs::read_to_string(dir.join(MARKER)).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("Only groups made on this page can be changed here.")?;
    if setup.character_ids.is_empty() {
        return Err("Pick at least one character.".into());
    }
    if crate::profiles::clients_open() > 0 {
        return Err("Close the EVE clients first: a running client writes its settings back when it closes.".into());
    }
    let seat = crate::launch::fetch_seat(&app).await?;
    crate::launch::store_seat(&app, &seat)?;
    let account_of = |id: u64| seat.accounts.iter().find(|a| a.characters.iter().any(|c| c.id == id));
    let mut per_account: BTreeMap<String, u64> = BTreeMap::new();
    for id in &setup.character_ids {
        let account = account_of(*id).ok_or_else(|| format!("Character {id} isn't in Your accounts."))?;
        if per_account.insert(account.nickname.clone(), *id).is_some() {
            return Err(format!("Account {} has two characters picked: the launcher starts one character per account.", account.nickname));
        }
    }
    let profile_dir = |id: u64| -> Option<PathBuf> {
        let p = account_of(id)?.profile.clone()?;
        let d = root.join(format!("settings_{p}"));
        d.is_dir().then_some(d)
    };

    // A copy first (small: a few character files).
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = root.join(format!("Devils Rage before edit settings_{profile} {stamp}"));
    std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            copy(&entry.path(), &backup.join(entry.file_name()))?;
        }
    }

    let present: Vec<u64> = crate::profiles::entries_in(&dir, settings_file).into_iter().filter(|e| e.kind == Kind::Char).map(|e| e.id).collect();
    // Where an added character's settings come from: the baseline's copy in this group if the
    // baseline is already here (that's the one tuned while playing), else its own profile.
    let base_char = if present.contains(&setup.baseline_id) {
        Some(dir.join(format!("core_char_{}.dat", setup.baseline_id)))
    } else {
        profile_dir(setup.baseline_id).map(|d| d.join(format!("core_char_{}.dat", setup.baseline_id))).filter(|p| p.is_file())
    };
    let base_user = newest_user(&dir).map(|(p, _)| p).or_else(|| profile_dir(setup.baseline_id).as_deref().and_then(newest_user).map(|(p, _)| p));
    let (mut added, mut removed) = (0usize, 0usize);
    for id in &setup.character_ids {
        if present.contains(id) {
            continue;
        }
        let own = profile_dir(*id);
        let own_char = own.as_ref().map(|d| d.join(format!("core_char_{id}.dat"))).filter(|p| p.is_file());
        let from = match setup.characters_from.as_str() {
            "own" => own_char.or_else(|| base_char.clone()),
            _ => base_char.clone().or(own_char),
        }
        .ok_or_else(|| format!("No settings to start character {id} from: log it in once, then try again."))?;
        copy(&from, &dir.join(format!("core_char_{id}.dat")))?;
        if let Some((_, user_id)) = own.as_deref().and_then(newest_user) {
            let target = dir.join(format!("core_user_{user_id}.dat"));
            if !target.is_file() {
                let from_user = if setup.characters_from == "own" { own.as_deref().and_then(newest_user).map(|(p, _)| p) } else { base_user.clone() };
                if let Some(from_user) = from_user {
                    copy(&from_user, &target)?;
                }
            }
        }
        added += 1;
    }
    for id in &present {
        if !setup.character_ids.contains(id) {
            let _ = std::fs::remove_file(dir.join(format!("core_char_{id}.dat")));
            removed += 1;
        }
    }
    if !setup.values.is_empty() || !setup.prefs.is_empty() {
        crate::tuning::apply_to(&dir, &setup.values, &setup.prefs, crate::tuning::file_time_now())?;
    }

    let group_id = save_seat_group(&app, &marker.name, &setup.character_ids).await?;
    marker.group_id = Some(group_id);
    std::fs::write(dir.join(MARKER), serde_json::to_string(&marker).unwrap_or_default()).map_err(|e| e.to_string())?;
    let old = crate::launch::seat_group_prefs(&app, group_id);
    crate::launch::save_seat_group_prefs(
        app.clone(),
        group_id,
        crate::launch::GroupPrefs {
            forward_hotkey: setup.forward_hotkey.or(old.forward_hotkey),
            backward_hotkey: setup.backward_hotkey.or(old.backward_hotkey),
            skipped: old.skipped,
            login_screens: setup.login_screens,
            color: old.color,
            remember_windows: old.remember_windows,
            hidden: old.hidden,
        },
    )?;
    let _ = crate::launch::seat_refresh(app.clone()).await;
    let new_accounts: Vec<String> = per_account.keys().cloned().collect();
    Ok(format!(
        "Updated “{}”: {added} added, {removed} taken out, {} kept as they were{}. A copy of the folder before is in “{}”.{}",
        marker.name,
        setup.character_ids.len() - added,
        if setup.values.is_empty() && setup.prefs.is_empty() { "" } else { "; sound & graphics written" },
        backup.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        if added > 0 || removed > 0 { format!(" In the EVE launcher, make the group's accounts match: {}.", new_accounts.join(", ")) } else { String::new() }
    ))
}

/// The groups this page made (folders with the note), newest first.
#[tauri::command]
pub fn group_setups() -> Result<Vec<serde_json::Value>, String> {
    let root = crate::profiles::root()?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let dir = entry.file_name().to_string_lossy().to_string();
        let Some(profile) = dir.strip_prefix("settings_") else { continue };
        let Ok(text) = std::fs::read_to_string(entry.path().join(MARKER)) else { continue };
        let Ok(marker) = serde_json::from_str::<Marker>(&text) else { continue };
        let characters = crate::profiles::entries_in(&entry.path(), settings_file).into_iter().filter(|e| e.kind == Kind::Char).map(|e| e.id).collect::<Vec<_>>();
        out.push(serde_json::json!({ "name": marker.name, "groupId": marker.group_id, "profile": profile, "created": marker.created, "characters": characters }));
    }
    out.sort_by(|a, b| b["created"].as_u64().cmp(&a["created"].as_u64()));
    Ok(out)
}

/// Remove a group made here: from Your accounts, and its folder moved aside (never deleted).
#[tauri::command]
pub async fn group_setup_delete(app: AppHandle, profile: String) -> Result<String, String> {
    if !crate::profiles::safe_name(&profile) {
        return Err("not a profile".into());
    }
    let root = crate::profiles::root()?;
    let dir = root.join(format!("settings_{profile}"));
    let marker: Marker = std::fs::read_to_string(dir.join(MARKER)).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("Only groups made on this page can be removed here.")?;
    if let Some(id) = marker.group_id {
        let (cookies, xsrf) = crate::launch::seat_session(&app)?;
        let _ = reqwest::Client::new()
            .post(format!("{}/devilsrage/launch-groups/{id}/app-delete", crate::SEAT_URL))
            .header("Accept", "application/json")
            .header("Cookie", cookies)
            .header("X-XSRF-TOKEN", xsrf.unwrap_or_default())
            .header("X-Requested-With", "XMLHttpRequest")
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let aside = root.join(format!("Devils Rage removed settings_{profile} {stamp}"));
    std::fs::rename(&dir, &aside).map_err(|e| format!("couldn't move the folder (is a client using it?): {e}"))?;
    let _ = crate::launch::seat_refresh(app).await;
    Ok(format!("Removed “{}”: out of Your accounts, and settings_{profile} moved to “{}”. Remove the group in the EVE launcher too.", marker.name, aside.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Test Launch"), "TestLaunch");
        assert_eq!(folder_name("Roll Fleet #2"), "RollFleet2");
        assert_eq!(folder_name("../../x"), "x");
        assert_eq!(folder_name("   "), "");
    }
}
