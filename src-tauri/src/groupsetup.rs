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
    /// 27 Nov: accounts (by nickname) that join at the login screen, with no character picked
    /// (Character "None" in the launcher): e.g. a daily-login group of every account.
    #[serde(default)]
    pub login_accounts: Vec<String>,
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
    /// 27 Nov: one window spot for every client of the group (the folder's core_public__.yaml).
    #[serde(default)]
    pub window: Option<WindowSpot>,
}

/// EVE's window for the whole group: Fixed Window or Window mode, its size as EVE shows it (the
/// inside in Window mode) and, in Window mode, the outside's corner.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct WindowSpot {
    pub fixed: bool,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    name: String,
    group_id: Option<u64>,
    created: u64,
    #[serde(default)]
    logins: Vec<String>,
}

/// "Roll Fleet #2" -> "RollFleet2" (what the launcher's client profile box shows).
pub fn folder_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(30).collect()
}

/// The newest account file in a profile folder (the account that ran there last).
fn newest_user(dir: &Path) -> Option<(PathBuf, u64)> {
    crate::profiles::entries_in(dir, settings_file).into_iter().find(|e| e.kind == Kind::User).map(|e| (dir.join(&e.file), e.id))
}

/// 27 Nov: the newest copy of a settings file in any profile folder (accounts have moved to
/// profiles of their own, so the SeAT profile number isn't always where they play now).
fn newest_anywhere(root: &Path, file: &str, skip: Option<&Path>) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("settings_") || skip.is_some_and(|s| s == entry.path().as_path()) {
            continue;
        }
        let path = entry.path().join(file);
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| at > *t) {
            best = Some((at, path));
        }
    }
    best.map(|(_, p)| p)
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))
}

/// One account in the group: its characters' files to put in the folder, and its account file.
struct Member {
    nickname: String,
    /// Characters whose settings go in the folder (the picked one, or all of a login-screen account's).
    chars: Vec<u64>,
    names: Vec<String>,
    login: bool,
    user: Option<u64>,
}

/// Who's in the group: picked characters (one per account) and whole accounts at the login screen.
fn members(app: &AppHandle, seat: &crate::launch::SeatData, setup: &NewGroup) -> Result<Vec<Member>, String> {
    let mut accounts: Vec<&crate::launch::SeatAccount> = seat.accounts.iter().collect();
    accounts.sort_by_key(|a| crate::launch::nickname_key(&a.nickname));
    let users = crate::launch::account_users(app, &accounts);
    let mut out: Vec<Member> = Vec::new();
    for id in &setup.character_ids {
        let i = accounts.iter().position(|a| a.characters.iter().any(|c| c.id == *id)).ok_or_else(|| format!("Character {id} isn't in Your accounts."))?;
        let account = accounts[i];
        if out.iter().any(|m| m.nickname == account.nickname) {
            return Err(format!("Account {} has two characters picked: the launcher starts one character per account.", account.nickname));
        }
        let names = account.characters.iter().filter(|c| c.id == *id).map(|c| c.name.clone()).collect();
        out.push(Member { nickname: account.nickname.clone(), chars: vec![*id], names, login: false, user: users[i] });
    }
    for nick in &setup.login_accounts {
        let i = accounts.iter().position(|a| &a.nickname == nick).ok_or_else(|| format!("Account {nick} isn't in Your accounts."))?;
        if out.iter().any(|m| &m.nickname == nick) {
            return Err(format!("Account {nick} has a character and its login screen picked: pick one."));
        }
        let account = accounts[i];
        out.push(Member { nickname: nick.clone(), chars: account.characters.iter().map(|c| c.id).collect(), names: Vec::new(), login: true, user: users[i] });
    }
    if out.is_empty() {
        return Err("Pick at least one character or login screen.".into());
    }
    out.sort_by_key(|m| crate::launch::nickname_key(&m.nickname));
    Ok(out)
}

/// The files to put in the folder (file name → where from), leaving out `have` (already there).
fn plan(root: &Path, setup: &NewGroup, members: &[Member], base_char: &Path, base_user: Option<&Path>, have: &dyn Fn(&str) -> bool, skip: Option<&Path>) -> Vec<(String, PathBuf)> {
    let own = setup.characters_from == "own";
    let mut files = Vec::new();
    for m in members {
        for id in &m.chars {
            let file = format!("core_char_{id}.dat");
            if have(&file) {
                continue;
            }
            let from = if own { newest_anywhere(root, &file, skip).unwrap_or_else(|| base_char.to_path_buf()) } else { base_char.to_path_buf() };
            files.push((file, from));
        }
        if let Some(user) = m.user {
            let file = format!("core_user_{user}.dat");
            if have(&file) {
                continue;
            }
            let from = if own { newest_anywhere(root, &file, skip).or_else(|| base_user.map(Path::to_path_buf)) } else { base_user.map(Path::to_path_buf) };
            if let Some(from) = from {
                files.push((file, from));
            }
        }
    }
    files
}

fn summary(members: &[Member]) -> Vec<serde_json::Value> {
    members.iter().map(|m| serde_json::json!({ "account": m.nickname, "characters": m.names, "login": m.login })).collect()
}

/// The group's window, written into its folder's core_public__.yaml.
fn write_spot(dir: &Path, spot: WindowSpot) -> Result<(), String> {
    let path = dir.join("core_public__.yaml");
    let text = std::fs::read_to_string(&path).map_err(|_| "The baseline's profile has no core_public__.yaml to set the window in: start a client with it once.".to_string())?;
    let text = crate::placeclients::yaml_window(&text, spot.fixed, spot.left, spot.top, spot.width.max(200), spot.height.max(150), crate::tuning::file_time_now());
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Make it all. Returns what the EVE launcher needs: the profile name and the accounts.
#[tauri::command]
pub async fn group_setup_create(app: AppHandle, setup: NewGroup) -> Result<serde_json::Value, String> {
    let name = setup.name.trim().to_string();
    let folder = folder_name(&name);
    if folder.is_empty() {
        return Err("Give the group a name with some letters or numbers in it.".into());
    }

    // Fresh accounts and characters from SeAT (which account each character is on).
    let seat = crate::launch::fetch_seat(&app).await?;
    crate::launch::store_seat(&app, &seat)?;
    let members = members(&app, &seat, &setup)?;
    if !seat.accounts.iter().any(|a| a.characters.iter().any(|c| c.id == setup.baseline_id)) {
        return Err("Pick the character whose settings to start from (step 4).".into());
    }

    let root = crate::profiles::root()?;
    let target = root.join(format!("settings_{folder}"));
    if target.exists() && !target.join(MARKER).is_file() {
        return Err(format!("settings_{folder} already exists and wasn't made by the app: pick another name so nothing of yours is overwritten."));
    }
    let base_char = newest_anywhere(&root, &format!("core_char_{}.dat", setup.baseline_id), Some(target.as_path())).ok_or("The baseline character has no settings yet: log it in once, set it up, log out, then try again.")?;
    let base_dir = base_char.parent().map(Path::to_path_buf).ok_or("no folder")?;
    let base_user = newest_user(&base_dir).map(|(p, _)| p);

    // Build it in a scratch folder first, then put it in place (half a profile never shows up).
    let building = root.join(format!(".building_{folder}"));
    let _ = std::fs::remove_dir_all(&building);
    std::fs::create_dir_all(&building).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        for (file, from) in plan(&root, &setup, &members, &base_char, base_user.as_deref(), &|_| false, Some(target.as_path())) {
            copy(&from, &building.join(file))?;
        }
        for file in ["core_public__.yaml", "prefs.ini"] {
            let from = base_dir.join(file);
            if from.is_file() {
                copy(&from, &building.join(file))?;
            }
        }
        crate::tuning::apply_to(&building, &setup.values, &setup.prefs, crate::tuning::file_time_now())?;
        if let Some(spot) = setup.window {
            write_spot(&building, spot)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&building);
        return Err(e);
    }

    // The launch group in Your accounts (its characters; login screens are this PC's).
    let group_id = save_seat_group(&app, &name, &setup.character_ids).await?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let marker = Marker { name: name.clone(), group_id: Some(group_id), created: stamp, logins: setup.login_accounts.clone() };
    std::fs::write(building.join(MARKER), serde_json::to_string(&marker).unwrap_or_default()).map_err(|e| e.to_string())?;
    if target.exists() {
        let aside = root.join(format!("Devils Rage replaced settings_{folder} {stamp}"));
        std::fs::rename(&target, &aside).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&building, &target).map_err(|e| e.to_string())?;

    // This PC's pedal keys for it, and the fresh list.
    crate::launch::save_seat_group_prefs(
        app.clone(),
        group_id,
        crate::launch::GroupPrefs {
            forward_hotkey: setup.forward_hotkey,
            backward_hotkey: setup.backward_hotkey,
            skipped: Vec::new(),
            login_screens: setup.login_screens || !setup.login_accounts.is_empty(),
            ..Default::default()
        },
    )?;
    let _ = crate::launch::seat_refresh(app.clone()).await;
    log::info!("new launch group {name}: settings_{folder}, {} characters, {} login screens", setup.character_ids.len(), setup.login_accounts.len());

    Ok(serde_json::json!({ "groupId": group_id, "profile": folder, "accounts": summary(&members), "window": setup.window.is_some() }))
}

async fn save_seat_group(app: &AppHandle, name: &str, ids: &[u64]) -> Result<u64, String> {
    let (cookies, xsrf) = crate::launch::seat_session(app)?;
    let xsrf = xsrf.ok_or("SeAT's session is missing: open Your accounts once, then try again")?;
    let response = crate::http_client()
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

/// Change a group made here, in place: members added get their settings (their own or the
/// baseline's), characters no member needs leave the folder, the sound & graphics you ticked and
/// the window are written, and Your accounts gets the new member list. Everything a client saved
/// in the folder since (window layouts while playing) is kept. A copy of the folder is put aside
/// first.
#[tauri::command]
pub async fn group_setup_update(app: AppHandle, profile: String, setup: NewGroup) -> Result<String, String> {
    if !crate::profiles::safe_name(&profile) {
        return Err("not a profile".into());
    }
    let root = crate::profiles::root()?;
    let dir = root.join(format!("settings_{profile}"));
    let mut marker: Marker = std::fs::read_to_string(dir.join(MARKER)).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("Only groups made on this page can be changed here.")?;
    if crate::profiles::clients_open() > 0 {
        return Err("Close the EVE clients first: a running client writes its settings back when it closes.".into());
    }
    let seat = crate::launch::fetch_seat(&app).await?;
    crate::launch::store_seat(&app, &seat)?;
    let members = members(&app, &seat, &setup)?;

    // A copy first (small: a few character files).
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = root.join(format!("Devils Rage before edit settings_{profile} {stamp}"));
    std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            copy(&entry.path(), &backup.join(entry.file_name()))?;
        }
    }

    // An added character starts from the baseline's copy in this group if it's here (the one
    // tuned while playing), else its newest copy anywhere.
    let here = dir.join(format!("core_char_{}.dat", setup.baseline_id));
    let base_char = if here.is_file() { Some(here) } else { newest_anywhere(&root, &format!("core_char_{}.dat", setup.baseline_id), Some(dir.as_path())) };
    let base_char = base_char.ok_or("The baseline character has no settings yet: log it in once, then try again.")?;
    let base_user = newest_user(&dir).map(|(p, _)| p).or_else(|| base_char.parent().and_then(newest_user).map(|(p, _)| p));
    let files = plan(&root, &setup, &members, &base_char, base_user.as_deref(), &|f| dir.join(f).is_file(), Some(dir.as_path()));
    let added = files.iter().filter(|(f, _)| f.starts_with("core_char_")).count();
    for (file, from) in &files {
        copy(from, &dir.join(file))?;
    }
    let needed: Vec<u64> = members.iter().flat_map(|m| m.chars.iter().copied()).collect();
    let mut removed = 0usize;
    for e in crate::profiles::entries_in(&dir, settings_file).into_iter().filter(|e| e.kind == Kind::Char) {
        if !needed.contains(&e.id) {
            let _ = std::fs::remove_file(dir.join(format!("core_char_{}.dat", e.id)));
            removed += 1;
        }
    }
    if !setup.values.is_empty() || !setup.prefs.is_empty() {
        crate::tuning::apply_to(&dir, &setup.values, &setup.prefs, crate::tuning::file_time_now())?;
    }
    if let Some(spot) = setup.window {
        write_spot(&dir, spot)?;
    }

    let group_id = save_seat_group(&app, &marker.name, &setup.character_ids).await?;
    marker.group_id = Some(group_id);
    marker.logins = setup.login_accounts.clone();
    std::fs::write(dir.join(MARKER), serde_json::to_string(&marker).unwrap_or_default()).map_err(|e| e.to_string())?;
    let old = crate::launch::seat_group_prefs(&app, group_id);
    crate::launch::save_seat_group_prefs(
        app.clone(),
        group_id,
        crate::launch::GroupPrefs {
            forward_hotkey: setup.forward_hotkey.or(old.forward_hotkey),
            backward_hotkey: setup.backward_hotkey.or(old.backward_hotkey),
            skipped: old.skipped,
            login_screens: setup.login_screens || !setup.login_accounts.is_empty(),
            color: old.color,
            remember_windows: old.remember_windows,
            hidden: old.hidden,
            order: old.order,
        },
    )?;
    let _ = crate::launch::seat_refresh(app.clone()).await;
    let list: Vec<String> = members.iter().map(|m| if m.login { format!("{} (login screen)", m.nickname) } else { m.nickname.clone() }).collect();
    Ok(format!(
        "Updated “{}”: {added} character files added, {removed} taken out{}{}. A copy of the folder before is in “{}”.{}",
        marker.name,
        if setup.values.is_empty() && setup.prefs.is_empty() { "" } else { "; sound & graphics written" },
        if setup.window.is_some() { "; window set" } else { "" },
        backup.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        if added > 0 || removed > 0 { format!(" In the EVE launcher, make the group's accounts match: {}.", list.join(", ")) } else { String::new() }
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
        out.push(serde_json::json!({ "name": marker.name, "groupId": marker.group_id, "profile": profile, "created": marker.created, "characters": characters, "logins": marker.logins }));
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
        let _ = crate::http_client()
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

    #[test]
    fn takes_the_newest_copy_from_any_profile_but_the_group() {
        let root = std::env::temp_dir().join(format!("dr-groupsetup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["settings_1", "settings_tsawind", "settings_Daily", "other"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let file = "core_char_42.dat";
        std::fs::write(root.join("settings_1").join(file), "old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(root.join("settings_tsawind").join(file), "new").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(root.join("settings_Daily").join(file), "group").unwrap();
        std::fs::write(root.join("other").join(file), "not a profile").unwrap();
        let skip = root.join("settings_Daily");
        let found = newest_anywhere(&root, file, Some(skip.as_path())).unwrap();
        assert_eq!(std::fs::read_to_string(found).unwrap(), "new");
        assert!(newest_anywhere(&root, "core_char_7.dat", None).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
