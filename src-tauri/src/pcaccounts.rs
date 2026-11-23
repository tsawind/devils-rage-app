//! Patch 74: which characters sit on which EVE account on this PC, for Your accounts.
//!
//! Two sources, best first:
//! - the EVE launcher's logs (it lists each account's characters): certain;
//! - EVE's profile folders: a folder used by one account only (settings_11 holds
//!   core_user_<account>.dat once) says every character file in it is that account's; a character
//!   seen in several folders belongs to the one account all of those folders share. Checked
//!   against the launcher on Tsa's PC: every one it could check was right.
//!
//! Nothing here leaves the PC by itself; Your accounts (our SeAT page in the app) shows it.

use std::collections::{BTreeMap, BTreeSet};

/// One profile folder: its name ("11", "Alphas") and the accounts / characters with files in it.
#[derive(Debug, Clone, Default)]
pub struct Folder {
    pub profile: String,
    pub users: BTreeSet<u64>,
    /// Character id → when its file was last written (ms).
    pub chars: BTreeMap<u64, u64>,
    /// EVE account number → when its user file here was last written (ms): an old copy left
    /// from copying folders around looks "shared" but hasn't been used in years.
    pub user_times: BTreeMap<u64, u64>,
}

/// How a character was placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Launcher,
    Profiles,
}

/// An EVE account on this PC: its characters (and how each was placed), and its own profile
/// folder when one folder is used by it alone.
#[derive(Debug, Clone, Default)]
pub struct PcAccount {
    pub characters: Vec<(u64, How)>,
    pub profile: Option<String>,
}

/// EVE accounts hold at most three characters.
const MAX: usize = 3;

pub fn deduce(launcher: &[(u64, Vec<u64>)], folders: &[Folder]) -> BTreeMap<u64, PcAccount> {
    let mut owner: BTreeMap<u64, (u64, How)> = BTreeMap::new();
    for (user, chars) in launcher {
        for c in chars {
            owner.insert(*c, (*user, How::Launcher));
        }
    }
    // Newest first: an account can have more than three character files in its folder (deleted or
    // transferred characters leave theirs behind), and its current characters are the ones played lately.
    let mut newest: BTreeMap<u64, u64> = BTreeMap::new();
    for f in folders {
        for (c, t) in &f.chars {
            let e = newest.entry(*c).or_default();
            *e = (*e).max(*t);
        }
    }
    let mut all_chars: Vec<u64> = newest.keys().copied().collect();
    all_chars.sort_by_key(|c| std::cmp::Reverse(newest[c]));
    // Each round places the characters whose folders leave one account with room; repeat until
    // nothing changes (a placed character can free up the next one).
    for _ in 0..10 {
        let mut count: BTreeMap<u64, usize> = BTreeMap::new();
        for (u, _) in owner.values() {
            *count.entry(*u).or_default() += 1;
        }
        let mut changed = false;
        for c in &all_chars {
            if owner.contains_key(c) {
                continue;
            }
            let mut candidates: Option<BTreeSet<u64>> = None;
            for f in folders.iter().filter(|f| f.chars.contains_key(c)) {
                candidates = Some(match candidates {
                    None => f.users.clone(),
                    Some(s) => s.intersection(&f.users).copied().collect(),
                });
            }
            let room: Vec<u64> = candidates.unwrap_or_default().into_iter().filter(|u| count.get(u).copied().unwrap_or(0) < MAX).collect();
            if let [u] = room[..] {
                owner.insert(*c, (u, How::Profiles));
                *count.entry(u).or_default() += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out: BTreeMap<u64, PcAccount> = BTreeMap::new();
    for (c, (u, how)) in owner {
        out.entry(u).or_default().characters.push((c, how));
    }
    // An account's own profile folder: one only it uses (named ones like "Alphas" are shared).
    for f in folders.iter().filter(|f| f.users.len() == 1) {
        let u = *f.users.iter().next().unwrap_or(&0);
        let entry = out.entry(u).or_default();
        let numbered = f.profile.bytes().all(|b| b.is_ascii_digit());
        if entry.profile.is_none() || numbered {
            entry.profile = Some(f.profile.clone());
        }
    }
    out
}

/// The profile folders on this PC (settings_*), read from EVE's settings folder.
pub fn folders() -> Vec<Folder> {
    let Ok(root) = crate::profiles::root() else { return Vec::new() };
    let Ok(dirs) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    for dir in dirs.flatten() {
        let name = dir.file_name().to_string_lossy().to_string();
        let Some(profile) = crate::profiles::profile_of(&name) else { continue };
        let mut folder = Folder { profile: profile.to_string(), ..Folder::default() };
        for entry in crate::profiles::entries_in(&dir.path(), crate::profiles::settings_file) {
            match entry.kind {
                crate::profiles::Kind::Char => {
                    folder.chars.insert(entry.id, entry.modified);
                }
                crate::profiles::Kind::User => {
                    folder.users.insert(entry.id);
                    folder.user_times.insert(entry.id, entry.modified);
                }
            }
        }
        if !folder.users.is_empty() {
            out.push(folder);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_name_becomes_a_safe_folder_name() {
        assert_eq!(login_folder(Some("Tsawind2")).as_deref(), Some("Tsawind2"));
        assert_eq!(login_folder(Some("tsa wind.12")).as_deref(), Some("tsawind12"));
        assert_eq!(login_folder(Some("...")), None);
        assert_eq!(login_folder(None), None);
    }

    fn folder(profile: &str, users: &[u64], chars: &[u64]) -> Folder {
        Folder { profile: profile.into(), users: users.iter().copied().collect(), chars: chars.iter().map(|c| (*c, 1)).collect(), ..Folder::default() }
    }

    #[test]
    fn places_characters_by_launcher_and_profile_folders() {
        let launcher = vec![(1, vec![10, 11])];
        let folders = vec![
            folder("1", &[1], &[10, 11, 12]),
            folder("2", &[2], &[20]),
            folder("Alphas", &[1, 2, 3], &[10, 20, 30, 31]),
            folder("3", &[2, 3], &[21, 30]),
        ];
        let got = deduce(&launcher, &folders);
        assert_eq!(got[&1].characters, vec![(10, How::Launcher), (11, How::Launcher), (12, How::Profiles)]);
        assert_eq!(got[&1].profile.as_deref(), Some("1"));
        assert_eq!(got[&2].characters, vec![(20, How::Profiles)]);
        // 30 is in folders shared by 2 and 3 and by 1, 2 and 3: account 2 or 3, can't tell; 31 too.
        assert!(!got.values().any(|a| a.characters.iter().any(|(c, _)| *c == 30 || *c == 31)));
        // 21 is only in "3" (accounts 2 and 3): can't tell either.
        assert!(!got.values().any(|a| a.characters.iter().any(|(c, _)| *c == 21)));
    }

    #[test]
    fn an_account_with_old_character_files_keeps_its_newest_three() {
        let mut f = folder("2", &[2], &[]);
        for (c, t) in [(20, 100), (21, 900), (22, 500), (23, 800), (24, 50), (25, 700)] {
            f.chars.insert(c, t);
        }
        let got = deduce(&[], &[f]);
        let mut placed: Vec<u64> = got[&2].characters.iter().map(|(c, _)| *c).collect();
        placed.sort();
        assert_eq!(placed, vec![21, 23, 25]);
    }

    #[test]
    fn a_full_account_takes_no_more() {
        let launcher = vec![(1, vec![10, 11, 12])];
        let folders = vec![folder("Alphas", &[1, 2], &[10, 11, 12, 20])];
        let got = deduce(&launcher, &folders);
        assert_eq!(got[&2].characters, vec![(20, How::Profiles)]);
    }
}

/// Patch 74 (Tsa, 18 Nov): every character seen logged in on this PC, and the EVE account its
/// client was signed in to (from the client's own sign-in token): certain, and it builds up as
/// characters log in. Character name → EVE account number; this PC only (pc-accounts.json).
static SEEN: std::sync::Mutex<Option<BTreeMap<String, u64>>> = std::sync::Mutex::new(None);

fn seen_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    crate::shared::app_config(app, "pc-accounts.json")
}

fn with_seen<T>(app: &tauri::AppHandle, f: impl FnOnce(&mut BTreeMap<String, u64>) -> T) -> T {
    let mut guard = SEEN.lock().unwrap();
    let seen = guard.get_or_insert_with(|| seen_file(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(seen)
}

/// Note the EVE account of every logged-in client whose start-up line has been read (the rest are
/// read in the background and noted next time).
pub fn note_running(app: &tauri::AppHandle) {
    note_info(app);
    let mut changed = false;
    for (name, handle) in crate::clipboard::open_eve_windows() {
        if crate::startline::known_profile_of_window(handle).is_none() {
            continue;
        }
        let Some(user) = crate::startline::known_account_of_window(handle) else { continue };
        changed |= with_seen(app, |seen| seen.insert(name.clone(), user) != Some(user));
    }
    if changed {
        let text = with_seen(app, |seen| serde_json::to_string_pretty(seen).unwrap_or_default());
        if let Some(path) = seen_file(app) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, text);
        }
    }
}

/// Every character seen logged in here, with its EVE account.
pub fn seen(app: &tauri::AppHandle) -> BTreeMap<String, u64> {
    with_seen(app, |seen| seen.clone())
}

/// Patch 74: what each EVE account's sign-in token said (login name, tier, when), and the other
/// characters on this PC the pilot chose to hide from Your accounts. This PC only.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AccountInfo {
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    /// The settings profile its client was last started with.
    #[serde(default)]
    pub profile: Option<String>,
    /// When a client last said so (unix seconds).
    #[serde(default)]
    pub at: u64,
    /// Its own profile folder's name (without "settings_"), as the pilot named it here.
    #[serde(default)]
    pub own: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct InfoStore {
    #[serde(default)]
    accounts: BTreeMap<u64, AccountInfo>,
    #[serde(default)]
    hidden_others: BTreeSet<u64>,
}

static INFO_STORE: std::sync::Mutex<Option<InfoStore>> = std::sync::Mutex::new(None);

fn info_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    crate::shared::app_config(app, "pc-account-info.json")
}

fn with_info<T>(app: &tauri::AppHandle, f: impl FnOnce(&mut InfoStore) -> T) -> T {
    let mut guard = INFO_STORE.lock().unwrap();
    let store = guard.get_or_insert_with(|| info_file(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(store)
}

fn save_info(app: &tauri::AppHandle) {
    let text = with_info(app, |s| serde_json::to_string_pretty(s).unwrap_or_default());
    if let Some(path) = info_file(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

/// Take in what the tokens read so far said about each account (called with `note_running`).
pub fn note_info(app: &tauri::AppHandle) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut changed = false;
    for (user, (login, tier, profile)) in crate::startline::account_info() {
        changed |= with_info(app, |s| {
            let entry = s.accounts.entry(user).or_default();
            let new = AccountInfo { login: login.clone().or(entry.login.clone()), tier: tier.clone().or(entry.tier.clone()), profile: profile.clone().or(entry.profile.clone()), ..entry.clone() };
            if new.login != entry.login || new.tier != entry.tier || new.profile != entry.profile {
                *entry = AccountInfo { at: now, ..new };
                true
            } else {
                false
            }
        });
    }
    if changed {
        save_info(app);
    }
}

/// Every EVE account's login name and tier, as last seen.
pub fn infos(app: &tauri::AppHandle) -> BTreeMap<u64, AccountInfo> {
    with_info(app, |s| s.accounts.clone())
}

/// Hide (or show again) an "other character" in Your accounts.
pub fn hide_other(app: &tauri::AppHandle, id: u64, hidden: bool) {
    with_info(app, |s| {
        if hidden {
            s.hidden_others.insert(id);
        } else {
            s.hidden_others.remove(&id);
        }
    });
    save_info(app);
}

/// Characters with settings on this PC (profile folders) that no account list has: id, when last
/// played (its newest settings file, ms) and the profile folders it's in. Hidden ones left out.
pub fn others(app: &tauri::AppHandle, placed: &BTreeSet<u64>) -> Vec<(u64, u64, Vec<String>)> {
    let hidden = with_info(app, |s| s.hidden_others.clone());
    let mut found: BTreeMap<u64, (u64, Vec<String>)> = BTreeMap::new();
    for f in folders() {
        for (c, t) in &f.chars {
            let e = found.entry(*c).or_default();
            e.0 = e.0.max(*t);
            e.1.push(f.profile.clone());
        }
    }
    let mut out: Vec<(u64, u64, Vec<String>)> = found.into_iter().filter(|(c, _)| !placed.contains(c) && !hidden.contains(c)).map(|(c, (t, p))| (c, t, p)).collect();
    out.sort_by_key(|(_, t, _)| std::cmp::Reverse(*t));
    out
}

/// The folder (beside the profile folders) where removed characters' settings files go. Never
/// deleted: moved aside, each under its profile folder's name, so it can be put back by hand.
pub const ARCHIVE: &str = "Devils Rage archived characters";

/// Move one character's settings files (core_char_<id>.dat in every profile folder) into the
/// archive folder. Refused while that character is logged in (EVE would write them again).
pub fn archive_character(id: u64, name: Option<&str>, only: Option<&str>) -> Result<usize, String> {
    if let Some(name) = name {
        if crate::clipboard::open_eve_characters().iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(format!("{name} is logged in: close that client first."));
        }
    }
    let root = crate::profiles::root()?;
    let file = format!("core_char_{id}.dat");
    let mut moved = 0;
    for (folder, path) in profile_dirs(only)? {
        let from = path.join(&file);
        if !from.is_file() {
            continue;
        }
        let to_dir = root.join(ARCHIVE).join(&folder);
        std::fs::create_dir_all(&to_dir).map_err(|e| e.to_string())?;
        let mut to = to_dir.join(&file);
        let mut n = 2;
        while to.exists() {
            to = to_dir.join(format!("core_char_{id} ({n}).dat"));
            n += 1;
        }
        std::fs::rename(&from, &to).map_err(|e| format!("couldn't move {folder}\\{file}: {e}"))?;
        moved += 1;
    }
    log::info!("archived character {id}'s settings files: {moved}");
    Ok(moved)
}

/// Every character with settings on this PC: id → its profile folders, newest first, each with
/// when its file there was last written (ms).
pub fn all_chars() -> BTreeMap<u64, Vec<(String, u64)>> {
    let mut found: BTreeMap<u64, Vec<(String, u64)>> = BTreeMap::new();
    for f in folders() {
        for (c, t) in &f.chars {
            found.entry(*c).or_default().push((f.profile.clone(), *t));
        }
    }
    for list in found.values_mut() {
        list.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    }
    found
}

/// The profile folders a file action covers: one (`only`) or every profile folder.
fn profile_dirs(only: Option<&str>) -> Result<Vec<(String, std::path::PathBuf)>, String> {
    let root = crate::profiles::root()?;
    let mut out = Vec::new();
    for dir in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let folder = dir.file_name().to_string_lossy().to_string();
        let Some(profile) = crate::profiles::profile_of(&folder) else { continue };
        if only.is_some_and(|o| o != profile) {
            continue;
        }
        out.push((folder, dir.path()));
    }
    Ok(out)
}

/// Patch 74: the account's own profile folder, named after its EVE login (settings_tsawind), made
/// from the profile it has been started with: that profile's game settings (core_public, prefs.ini)
/// plus only this account's files (its core_user and its characters' core_char), not other accounts'.
pub fn make_account_profile(app: &tauri::AppHandle, user: u64, wanted: Option<&str>, from: Option<&str>) -> Result<String, String> {
    let info = infos(app).remove(&user).ok_or("Log this account in once first (the app learns its login there).")?;
    let name = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => w.to_string(),
        None => login_folder(info.login.as_deref()).ok_or("Log this account in once first, or type a name.")?,
    };
    if !crate::profiles::safe_name(&name) {
        return Err("Use letters, numbers, - and _ only (up to 40).".into());
    }
    let root = crate::profiles::root()?;
    let target = root.join(format!("settings_{name}"));
    if target.exists() {
        return Err(format!("settings_{name} is already there: pick another name, or pick {name} in the launcher if it's this account's."));
    }
    // Start from: the profile picked on the page, else the one it last started with; "fresh" = EVE's defaults.
    let from = match from.map(str::trim).filter(|f| !f.is_empty()) {
        Some("fresh") => None,
        Some(f) => Some(f.to_string()),
        None => Some(info.profile.clone().ok_or("Start this account once from the launcher first, or pick a profile to start from.")?),
    };
    let Some(from) = from else {
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        remember_own(app, user, &name);
        log::info!("made an empty settings_{name} for EVE account {user}");
        return Ok(format!("Made settings_{name}, empty: EVE starts it with its default settings. Now pick \"{name}\" as this account's client profile in the EVE launcher (not in the list? close the launcher fully, tray icon too, and open it again)."));
    };
    if !crate::profiles::safe_name(&from) {
        return Err("That profile has an odd name.".into());
    }
    let source = root.join(format!("settings_{from}"));
    if !source.is_dir() {
        return Err(format!("settings_{from} isn't there any more."));
    }
    // This account's characters: the launcher's lists and the clients seen on it.
    let mut chars: BTreeSet<u64> = crate::launch::launcher_accounts().into_iter().filter(|(u, _)| *u == user).flat_map(|(_, c)| c).collect();
    let seen_names: Vec<String> = seen(app).into_iter().filter(|(_, u)| *u == user).map(|(n, _)| n.to_lowercase()).collect();
    let seat = crate::launch::load_seat(app);
    for c in seat.accounts.iter().flat_map(|a| a.characters.iter()) {
        if seen_names.contains(&c.name.to_lowercase()) {
            chars.insert(c.id);
        }
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut copied = 0;
    let mut keep = vec![format!("core_user_{user}.dat"), "core_public__.yaml".to_string(), "prefs.ini".to_string()];
    keep.extend(chars.iter().map(|c| format!("core_char_{c}.dat")));
    for file in keep {
        let from_file = source.join(&file);
        if from_file.is_file() {
            std::fs::copy(&from_file, target.join(&file)).map_err(|e| format!("couldn't copy {file}: {e}"))?;
            copied += 1;
        }
    }
    remember_own(app, user, &name);
    log::info!("made settings_{name} for EVE account {user} from settings_{from}: {copied} files");
    let place = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Ok(format!("Made settings_{name} from settings_{from} ({copied} files: its game settings, the account's and its characters') in {place}. Close the EVE launcher fully (tray icon too), open it again and pick \"{name}\" for this account. If the launcher offers to create \"{name}\" instead, it isn't seeing this folder: don't create it, tell Tsa."))
}

/// A login name as a folder name (letters, numbers, - and _).
pub fn login_folder(login: Option<&str>) -> Option<String> {
    login.map(|l| l.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>()).filter(|n| crate::profiles::safe_name(n))
}

/// An existing profile folder as this account's own (nothing is copied or changed); "" forgets it.
pub fn set_own(app: &tauri::AppHandle, user: u64, name: &str) -> Result<String, String> {
    if name.is_empty() {
        remember_own(app, user, "");
        return Ok("Forgotten: it has no profile of its own here now (nothing on disk changed).".into());
    }
    if !crate::profiles::safe_name(name) || !crate::profiles::root()?.join(format!("settings_{name}")).is_dir() {
        return Err(format!("settings_{name} isn't there."));
    }
    remember_own(app, user, name);
    log::info!("settings_{name} is EVE account {user}'s own profile now");
    Ok(format!("settings_{name} is this account's own profile now. Keep picking \"{name}\" for it in the EVE launcher."))
}

fn remember_own(app: &tauri::AppHandle, user: u64, name: &str) {
    with_info(app, |s| s.accounts.entry(user).or_default().own = Some(name.to_string()));
    save_info(app);
}

/// This account's own profile folder, when it's there: the one named here, else settings_<login>
/// (made before names could be picked).
pub fn own_profile(info: &AccountInfo) -> Option<String> {
    if info.own.as_deref() == Some("") {
        return None; // the pilot said it has none ("Forget" under Advanced editing)
    }
    let root = crate::profiles::root().ok()?;
    [info.own.clone(), login_folder(info.login.as_deref())].into_iter().flatten().find(|n| crate::profiles::safe_name(n) && root.join(format!("settings_{n}")).is_dir())
}

/// Delete one character's settings files for good (Your accounts asks twice first; archiving is
/// what it recommends). Refused while that character is logged in.
pub fn delete_character(id: u64, name: Option<&str>, only: Option<&str>) -> Result<usize, String> {
    if let Some(name) = name {
        if crate::clipboard::open_eve_characters().iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(format!("{name} is logged in: close that client first."));
        }
    }
    let file = format!("core_char_{id}.dat");
    let mut deleted = 0;
    for (folder, dir) in profile_dirs(only)? {
        let path = dir.join(&file);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| format!("couldn't delete {folder}\\{file}: {e}"))?;
            deleted += 1;
        }
    }
    log::info!("deleted character {id}'s settings files: {deleted}");
    Ok(deleted)
}

/// Show one character's newest settings file in File Explorer (else the settings folder).
pub fn open_folder(id: u64, only: Option<&str>) {
    let Ok(root) = crate::profiles::root() else { return };
    let file = format!("core_char_{id}.dat");
    let newest = profile_dirs(only)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, d)| d.join(&file))
        .filter(|p| p.is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("explorer.exe");
        match &newest {
            Some(path) => command.arg(format!("/select,{}", path.display())),
            None => command.arg(root.as_os_str()),
        };
        let _ = command.spawn();
    }
    #[cfg(not(windows))]
    let _ = newest;
}

// ---- 22 Nov: the app's own This PC page (Your accounts, this PC's side) ----

fn clip(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().chars().take(40).collect::<String>()).filter(|t| !t.is_empty())
}

/// An account's own settings profile: use an existing folder, make one (copied from `from`, or
/// "fresh"), or forget it.
#[tauri::command]
pub fn pc_account_profile(app: tauri::AppHandle, user: u64, name: Option<String>, from: Option<String>, existing: Option<String>, forget: Option<bool>) -> Result<String, String> {
    if forget == Some(true) {
        return set_own(&app, user, "");
    }
    if let Some(existing) = clip(existing) {
        return set_own(&app, user, &existing);
    }
    make_account_profile(&app, user, clip(name).as_deref(), clip(from).as_deref())
}

/// EVE's third-party apps page for a character (its own window; shows which account to sign in).
/// Async: it opens a window (a window built in a sync command can lock up on Windows).
#[tauri::command]
pub async fn pc_eve_apps(app: tauri::AppHandle, character: u64, login: Option<String>) {
    let login = login.map(|l| l.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.').take(40).collect::<String>());
    crate::eveapps::open(&app, character, login.as_deref());
}

/// Mark a character as a Spy on this PC (never named to SeAT or the mapper), or not.
#[tauri::command]
pub fn pc_set_spy(app: tauri::AppHandle, name: String, on: bool) -> Result<(), String> {
    let name: String = name.trim().chars().take(40).collect();
    if name.is_empty() {
        return Err("no name".into());
    }
    crate::incognito::set_spy(&app, &name, on);
    Ok(())
}
