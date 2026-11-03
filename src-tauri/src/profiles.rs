//! Patch 64: EVE client settings profiles, like eve-settings-manager.
//!
//! The EVE launcher keeps each "client profile" as a folder `settings_<name>` in the shared
//! cache (`%LOCALAPPDATA%\CCP\EVE\c_eve_sharedcache_tq_tranquility`). Inside, the client writes
//! one `core_char_<character id>.dat` per character (windows, overview, chat…) and one
//! `core_user_<account id>.dat` per account (keys, audio, graphics…).
//!
//! This module lists those profiles and the characters / accounts in them, lets you label a
//! profile (which account or launch group it's for), copies one settings file over others
//! (char → chars, user → users), clones a profile into a new one, and backs everything up first
//! into a dated folder beside them ("Devils Rage backup 2026-11-02 1633"), restorable in one click.
//!
//! The game rewrites these files when a client closes, so copying while clients are open is
//! refused unless you say so. Only files directly inside a profile are touched (the Browser
//! cache folder is left alone), and every name is checked so nothing outside the folder is.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const BACKUP_PREFIX: &str = "Devils Rage backup ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Char,
    User,
}

/// "core_char_93976142.dat" -> (Char, 93976142). Junk names ("core_char__.dat",
/// "core_char_('char', None, 'dat').dat", "… - Copy.dat") are not settings files.
pub fn settings_file(name: &str) -> Option<(Kind, u64)> {
    let stem = name.strip_suffix(".dat")?;
    let (kind, id) = if let Some(id) = stem.strip_prefix("core_char_") {
        (Kind::Char, id)
    } else {
        (Kind::User, stem.strip_prefix("core_user_")?)
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((kind, id.parse().ok()?))
}

/// A template file at the top of the cache folder: any name that ends in a settings file name
/// ("Tsawind Template 4-9-2024 core_char_1793390976.dat").
pub fn template_file(name: &str) -> Option<(Kind, u64)> {
    let at = name.rfind("core_")?;
    settings_file(&name[at..])
}

/// A profile folder name: "settings_12" -> "12", "settings_Alphas" -> "Alphas".
pub fn profile_of(dir: &str) -> Option<&str> {
    let name = dir.strip_prefix("settings_")?;
    safe_name(name).then_some(name)
}

/// Letters, digits, '-' and '_' only (so a name can never climb out of the folder).
pub fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// "Devils Rage backup 2026-11-02 2333" for a time (UTC seconds) plus an offset in seconds
/// (0 = UTC; the folder name says "UTC" so it's never mistaken for local time).
pub fn backup_name(unix: i64, offset: i64) -> String {
    let t = unix + offset;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{BACKUP_PREFIX}{y:04}-{m:02}-{d:02} {:02}{:02}", secs / 3600, secs % 3600 / 60)
}

/// Days since 1970-01-01 -> (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- Where things are -------------------------------------------------------------------------

fn cache_root() -> Option<PathBuf> {
    let root = dirs::data_local_dir()?.join(r"CCP\EVE\c_eve_sharedcache_tq_tranquility");
    root.is_dir().then_some(root)
}

pub(crate) fn root() -> Result<PathBuf, String> {
    cache_root().ok_or_else(|| "Couldn't find EVE's settings folder (CCP\\EVE\\c_eve_sharedcache_tq_tranquility under your local app data).".to_string())
}

fn labels_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("profiles.json"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Labels {
    /// Profile name -> what it's for ("Tsawind's main account", "Roll fleet").
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

fn load_labels(app: &AppHandle) -> Labels {
    labels_path(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn clients_open() -> usize {
    crate::clipboard::open_eve_windows().len() + crate::clipboard::open_login_windows().len()
}

fn refuse_while_open(force: bool) -> Result<(), String> {
    let open = clients_open();
    if open > 0 && !force {
        return Err(format!(
            "{open} EVE client{} open. EVE rewrites its settings when a client closes, so close them first (or choose to go ahead anyway).",
            if open == 1 { " is" } else { "s are" }
        ));
    }
    Ok(())
}

// ---- Listing -----------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SettingsEntry {
    pub file: String,
    pub kind: Kind,
    pub id: u64,
    pub modified: u64,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct Profile {
    pub name: String,
    pub label: Option<String>,
    pub chars: Vec<SettingsEntry>,
    pub users: Vec<SettingsEntry>,
    pub modified: u64,
}

pub(crate) fn entries_in(dir: &Path, read: impl Fn(&str) -> Option<(Kind, u64)>) -> Vec<SettingsEntry> {
    let mut out: Vec<SettingsEntry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().to_string();
            let (kind, id) = read(&file)?;
            let path = e.path();
            Some(SettingsEntry { modified: modified_ms(&path), size: e.metadata().map(|m| m.len()).unwrap_or(0), file, kind, id })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Every profile with its characters and accounts, the template files beside them, the backups,
/// and how many EVE clients are open (copying is refused while they are).
#[tauri::command]
pub fn profiles_scan(app: AppHandle) -> Result<serde_json::Value, String> {
    let root = root()?;
    let labels = load_labels(&app).labels;
    let mut profiles: Vec<Profile> = Vec::new();
    let mut backups: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().to_string();
        if dir.starts_with(BACKUP_PREFIX) {
            backups.push(dir);
            continue;
        }
        let Some(name) = profile_of(&dir) else { continue };
        let all = entries_in(&entry.path(), settings_file);
        let modified = all.iter().map(|e| e.modified).max().unwrap_or(0);
        let (chars, users) = all.into_iter().partition(|e| e.kind == Kind::Char);
        profiles.push(Profile { label: labels.get(name).cloned(), name: name.to_string(), chars, users, modified });
    }
    // settings_1, settings_2 … settings_33 in number order, named ones after.
    profiles.sort_by(|a, b| match (a.name.parse::<u32>(), b.name.parse::<u32>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    backups.sort();
    backups.reverse();
    let templates = entries_in(&root, template_file);
    Ok(serde_json::json!({
        "root": root.display().to_string(),
        "profiles": profiles,
        "templates": templates,
        "backups": backups,
        "clientsOpen": clients_open(),
    }))
}

/// Character names for ids (public ESI lookup, remembered for the session). Account ids have no
/// public name; they're shown as numbers you can label.
#[tauri::command]
pub async fn profiles_names(ids: Vec<u64>) -> HashMap<u64, String> {
    static KNOWN: Mutex<Option<HashMap<u64, String>>> = Mutex::new(None);
    let missing: Vec<u64> = {
        let guard = KNOWN.lock().unwrap();
        let known = guard.as_ref();
        ids.iter().copied().filter(|id| known.map_or(true, |k| !k.contains_key(id))).collect()
    };
    for chunk in missing.chunks(500) {
        let found = esi_names(chunk).await;
        let mut guard = KNOWN.lock().unwrap();
        guard.get_or_insert_with(HashMap::new).extend(found);
    }
    let guard = KNOWN.lock().unwrap();
    let known = guard.as_ref();
    ids.into_iter().filter_map(|id| Some((id, known?.get(&id)?.clone()))).collect()
}

async fn esi_names(ids: &[u64]) -> HashMap<u64, String> {
    #[derive(Deserialize)]
    struct Named {
        id: u64,
        name: String,
    }
    let response = reqwest::Client::new()
        .post("https://esi.evetech.net/latest/universe/names/?datasource=tranquility")
        .json(ids)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;
    match response {
        Ok(r) if r.status().is_success() => r.json::<Vec<Named>>().await.map(|v| v.into_iter().map(|n| (n.id, n.name)).collect()).unwrap_or_default(),
        Ok(r) => {
            log::warn!("ESI names answered {}", r.status());
            HashMap::new()
        }
        Err(e) => {
            log::warn!("ESI names failed: {e}");
            HashMap::new()
        }
    }
}

/// Say what a profile is for (empty clears it).
#[tauri::command]
pub fn profiles_label(app: AppHandle, profile: String, label: String) -> Result<(), String> {
    if !safe_name(&profile) {
        return Err("not a profile name".into());
    }
    let mut labels = load_labels(&app);
    let label: String = label.trim().chars().take(60).collect();
    if label.is_empty() {
        labels.labels.remove(&profile);
    } else {
        labels.labels.insert(profile, label);
    }
    let path = labels_path(&app).ok_or("no config folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&labels).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

// ---- Backup / restore ---------------------------------------------------------------------------

fn copy_profile_files(from: &Path, to: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let mut count = 0;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            std::fs::copy(entry.path(), to.join(entry.file_name())).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    Ok(count)
}

/// Copy every profile's files (not the Browser caches) into a new dated backup folder.
pub(crate) fn backup_all(root: &Path, note: &str) -> Result<String, String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut name = format!("{} UTC{note}", backup_name(now, 0));
    let mut n = 2;
    while root.join(&name).exists() {
        name = format!("{} UTC{note} ({n})", backup_name(now, 0));
        n += 1;
    }
    let target = root.join(&name);
    let mut files = 0;
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())?.flatten() {
        let dir = entry.file_name().to_string_lossy().to_string();
        if profile_of(&dir).is_some() && entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            files += copy_profile_files(&entry.path(), &target.join(&dir))?;
        }
    }
    log::info!("settings backed up: {name} ({files} files)");
    Ok(name)
}

/// Back up every profile now.
#[tauri::command]
pub fn profiles_backup() -> Result<String, String> {
    backup_all(&root()?, "")
}

/// Put a backup back (the current settings are backed up first, so this can be undone too).
#[tauri::command]
pub fn profiles_restore(backup: String, force: bool) -> Result<String, String> {
    let root = root()?;
    if !backup.starts_with(BACKUP_PREFIX) || backup.contains(['/', '\\']) || backup.contains("..") {
        return Err("not one of our backups".into());
    }
    let from = root.join(&backup);
    if !from.is_dir() {
        return Err("that backup is gone".into());
    }
    refuse_while_open(force)?;
    let safety = backup_all(&root, " before restore")?;
    let mut files = 0;
    for entry in std::fs::read_dir(&from).map_err(|e| e.to_string())?.flatten() {
        let dir = entry.file_name().to_string_lossy().to_string();
        if profile_of(&dir).is_some() && entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            files += copy_profile_files(&entry.path(), &root.join(&dir))?;
        }
    }
    Ok(format!("Restored {files} files from “{backup}”. What was there before is in “{safety}”."))
}

// ---- Copying settings ---------------------------------------------------------------------------

/// "settings_12/core_char_93976142.dat" or a template's file name -> its path, checked.
fn resolve(root: &Path, spec: &str) -> Result<(PathBuf, Kind), String> {
    match spec.split_once('/') {
        Some((dir, file)) => {
            profile_of(dir).ok_or("not a profile")?;
            let (kind, _) = settings_file(file).ok_or("not a settings file")?;
            Ok((root.join(dir).join(file), kind))
        }
        None => {
            if spec.contains(['\\', '/']) || spec.contains("..") {
                return Err("not a template".into());
            }
            let (kind, _) = template_file(spec).ok_or("not a template file")?;
            Ok((root.join(spec), kind))
        }
    }
}

/// Copy one settings file over others of the same kind (a character's onto characters, an
/// account's onto accounts). Everything is backed up first.
#[tauri::command]
pub fn profiles_copy(source: String, targets: Vec<String>, force: bool) -> Result<String, String> {
    let root = root()?;
    let (from, kind) = resolve(&root, &source)?;
    if !from.is_file() {
        return Err("the source file is gone".into());
    }
    let mut to: Vec<PathBuf> = Vec::new();
    for spec in &targets {
        if !spec.contains('/') {
            return Err("targets must be inside a profile".into());
        }
        let (path, target_kind) = resolve(&root, spec)?;
        if target_kind != kind {
            return Err("a character's settings go onto characters, an account's onto accounts".into());
        }
        if path != from {
            to.push(path);
        }
    }
    if to.is_empty() {
        return Err("pick at least one character or account to copy onto".into());
    }
    refuse_while_open(force)?;
    let backup = backup_all(&root, "")?;
    let bytes = std::fs::read(&from).map_err(|e| e.to_string())?;
    for path in &to {
        std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(format!("Copied onto {} file{}. Backup first: “{backup}”.", to.len(), if to.len() == 1 { "" } else { "s" }))
}

/// Patch 68: one target of "set up a launch group": a character and the client profile it runs in.
#[derive(Debug, Deserialize)]
pub struct GroupTarget {
    pub profile: String,
    pub character_id: u64,
}

/// What to copy from the baseline: its character file (windows, overview, chat), its account
/// file (keys, audio and graphics choices kept per account) and the profile's graphics / window /
/// volume file (core_public__.yaml).
#[derive(Debug, Deserialize)]
pub struct CopyParts {
    #[serde(default)]
    pub character: bool,
    #[serde(default)]
    pub account: bool,
    #[serde(default)]
    pub graphics: bool,
}

/// The newest account file in a profile (the one the client wrote last).
fn newest_user_file(dir: &Path) -> Option<PathBuf> {
    entries_in(dir, settings_file).into_iter().find(|e| e.kind == Kind::User).map(|e| dir.join(e.file))
}

/// Patch 68: give a whole launch group one baseline character's settings. Character files are
/// written for every target character (made if the character never logged in with that profile);
/// account and graphics files go to every target profile. Backed up first.
#[tauri::command]
pub fn profiles_apply_group(baseline_profile: String, baseline_character: u64, targets: Vec<GroupTarget>, parts: CopyParts, force: bool) -> Result<String, String> {
    let root = root()?;
    if !safe_name(&baseline_profile) {
        return Err("not a profile".into());
    }
    let base_dir = root.join(format!("settings_{baseline_profile}"));
    let base_char = base_dir.join(format!("core_char_{baseline_character}.dat"));
    let base_user = newest_user_file(&base_dir);
    let base_public = base_dir.join("core_public__.yaml");
    if parts.character && !base_char.is_file() {
        return Err("The baseline character has no settings in its profile yet: log it in once, set it up, log out, then try again.".into());
    }
    if !(parts.character || parts.account || parts.graphics) {
        return Err("Pick at least one kind of settings to copy.".into());
    }
    let mut char_targets: Vec<PathBuf> = Vec::new();
    let mut profiles: Vec<PathBuf> = Vec::new();
    for target in &targets {
        if !safe_name(&target.profile) {
            return Err(format!("{} isn't a profile name", target.profile));
        }
        let dir = root.join(format!("settings_{}", target.profile));
        if !dir.is_dir() {
            return Err(format!("settings_{} doesn't exist yet (make it under Make a profile, or set the profile in the EVE launcher and start that account once)", target.profile));
        }
        let file = dir.join(format!("core_char_{}.dat", target.character_id));
        if file != base_char {
            char_targets.push(file);
        }
        if dir != base_dir && !profiles.contains(&dir) {
            profiles.push(dir);
        }
    }
    if char_targets.is_empty() && profiles.is_empty() {
        return Err("Nothing to copy onto: the group is only the baseline.".into());
    }
    refuse_while_open(force)?;
    let backup = backup_all(&root, "")?;
    let mut done: Vec<String> = Vec::new();
    if parts.character {
        let bytes = std::fs::read(&base_char).map_err(|e| e.to_string())?;
        for path in &char_targets {
            std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        done.push(format!("{} character{}", char_targets.len(), if char_targets.len() == 1 { "" } else { "s" }));
    }
    if parts.account {
        if let Some(from) = &base_user {
            let bytes = std::fs::read(from).map_err(|e| e.to_string())?;
            let mut count = 0;
            for dir in &profiles {
                // Every account file in that profile (one per account that ever ran in it).
                for entry in entries_in(dir, settings_file).into_iter().filter(|e| e.kind == Kind::User) {
                    std::fs::write(dir.join(&entry.file), &bytes).map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
            done.push(format!("{count} account file{}", if count == 1 { "" } else { "s" }));
        }
    }
    if parts.graphics && base_public.is_file() {
        let bytes = std::fs::read(&base_public).map_err(|e| e.to_string())?;
        for dir in &profiles {
            std::fs::write(dir.join("core_public__.yaml"), &bytes).map_err(|e| e.to_string())?;
        }
        done.push(format!("graphics / window / sound for {} profile{}", profiles.len(), if profiles.len() == 1 { "" } else { "s" }));
    }
    Ok(format!("Copied: {}. Backup first: “{backup}”.", done.join(", ")))
}

/// Patch 69: the tuning levers and each profile's current values (Client tuning page).
#[tauri::command]
pub fn tuning_read() -> Result<serde_json::Value, String> {
    let root = root()?;
    let mut profiles = serde_json::Map::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let dir = entry.file_name().to_string_lossy().to_string();
        let Some(name) = profile_of(&dir) else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let yaml = std::fs::read_to_string(entry.path().join("core_public__.yaml")).unwrap_or_default();
        let prefs = std::fs::read_to_string(entry.path().join("prefs.ini")).unwrap_or_default();
        profiles.insert(name.to_string(), serde_json::json!({
            "values": crate::tuning::read_values(&yaml),
            "prefs": crate::tuning::read_prefs(&prefs),
            "hasFile": !yaml.is_empty(),
        }));
    }
    Ok(serde_json::json!({
        "levers": crate::tuning::LEVERS,
        "prefs": crate::tuning::PREFS.iter().map(|(k, l)| serde_json::json!({ "key": k, "label": l })).collect::<Vec<_>>(),
        "profiles": profiles,
        "clientsOpen": clients_open(),
    }))
}

/// Patch 69: set levers (and prefs.ini switches) on these profiles. Backed up first.
#[tauri::command]
pub fn tuning_apply(profiles: Vec<String>, values: BTreeMap<String, serde_json::Value>, prefs: BTreeMap<String, bool>, force: bool) -> Result<String, String> {
    let root = root()?;
    if profiles.is_empty() {
        return Err("Pick at least one client profile.".into());
    }
    if values.is_empty() && prefs.is_empty() {
        return Err("Nothing to change: switch on at least one setting.".into());
    }
    let mut dirs = Vec::new();
    for name in &profiles {
        if !safe_name(name) {
            return Err(format!("{name} isn't a profile"));
        }
        let dir = root.join(format!("settings_{name}"));
        if !dir.is_dir() {
            return Err(format!("settings_{name} doesn't exist"));
        }
        dirs.push(dir);
    }
    refuse_while_open(force)?;
    let backup = backup_all(&root, " before tuning")?;
    let stamp = crate::tuning::file_time_now();
    let mut changed = 0;
    for dir in &dirs {
        changed += crate::tuning::apply_to(dir, &values, &prefs, stamp)?;
    }
    Ok(format!("{changed} setting{} changed on {} profile{}. Backup first: “{backup}”.", if changed == 1 { "" } else { "s" }, dirs.len(), if dirs.len() == 1 { "" } else { "s" }))
}

/// Make a new profile `settings_<name>` from an existing one (for an account or a launch group).
#[tauri::command]
pub fn profiles_clone(from: String, name: String) -> Result<String, String> {
    let root = root()?;
    if !safe_name(&from) || !safe_name(&name) {
        return Err("profile names are letters, digits, - and _".into());
    }
    let source = root.join(format!("settings_{from}"));
    let target = root.join(format!("settings_{name}"));
    if !source.is_dir() {
        return Err("that profile doesn't exist".into());
    }
    if target.exists() {
        return Err(format!("settings_{name} already exists"));
    }
    let files = copy_profile_files(&source, &target)?;
    Ok(format!("Made settings_{name} from settings_{from} ({files} files). Pick it as the client profile in the EVE launcher."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_settings_file_names() {
        assert_eq!(settings_file("core_char_93976142.dat"), Some((Kind::Char, 93976142)));
        assert_eq!(settings_file("core_user_7205846.dat"), Some((Kind::User, 7205846)));
        assert_eq!(settings_file("core_char__.dat"), None);
        assert_eq!(settings_file("core_char_('char', None, 'dat').dat"), None);
        assert_eq!(settings_file("core_char_1793390976 - Copy.dat"), None);
        assert_eq!(settings_file("core_public__.yaml"), None);
        assert_eq!(settings_file("prefs.ini"), None);
    }

    #[test]
    fn reads_template_names() {
        assert_eq!(template_file("Tsawind Template 4-9-2024 core_char_1793390976.dat"), Some((Kind::Char, 1793390976)));
        assert_eq!(template_file("Tsawind (Template) core_user_7205846.dat"), Some((Kind::User, 7205846)));
        assert_eq!(template_file("EANM.jar"), None);
    }

    #[test]
    fn profile_names_are_safe() {
        assert_eq!(profile_of("settings_12"), Some("12"));
        assert_eq!(profile_of("settings_Alphas"), Some("Alphas"));
        assert_eq!(profile_of("settings"), None);
        assert_eq!(profile_of("settings_../x"), None);
        assert!(!safe_name("a b"));
        assert!(!safe_name(""));
    }

    #[test]
    fn backup_names_sort_by_time() {
        // 2026-11-02 23:33 UTC
        assert_eq!(backup_name(1_793_662_380, 0), "Devils Rage backup 2026-11-02 2333");
        assert_eq!(backup_name(0, 0), "Devils Rage backup 1970-01-01 0000");
        assert!(backup_name(1_793_662_380, 0) < backup_name(1_793_662_380 + 3600, 0));
    }

    #[test]
    fn resolves_only_inside_the_folder() {
        let root = Path::new("/cache");
        assert!(resolve(root, "settings_3/core_char_1.dat").is_ok());
        assert!(resolve(root, "settings_3/../x/core_char_1.dat").is_err());
        assert!(resolve(root, "../core_char_1.dat").is_err());
        assert!(resolve(root, "settings_3/prefs.ini").is_err());
        assert_eq!(resolve(root, "Tsawind (Template) core_user_7205846.dat").unwrap().1, Kind::User);
    }
}
