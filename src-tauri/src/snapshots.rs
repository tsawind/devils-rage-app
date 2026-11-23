//! 23 Nov: one profile folder at a time: snapshots (save points you can put back), archiving a
//! folder out of the launcher's sight, and deleting one.
//!
//! Everything lives beside the profiles in EVE's settings folder, under names the launcher
//! ignores (it only lists `settings_*`):
//! - `Devils Rage snapshots\<profile>\<YYYY-MM-DD HHMM UTC>[ (n)]\` with the profile's files and a
//!   `snapshot.json` (note, when, how many files). Only the files directly in the profile are
//!   kept (core_*.dat, core_public__.yaml, prefs.ini); the Browser cache is left out.
//! - `Devils Rage archive\settings_<profile> <YYYY-MM-DD HHMM UTC>\`: the whole folder, moved.
//!
//! Anything that changes a profile needs every EVE client and the launcher closed
//! (`pcaccounts::require_eve_closed`). Putting a snapshot back or deleting a folder first takes a
//! snapshot of what's there, so both can be undone.

use crate::profiles::{backup_name, profile_of, root, safe_name, BACKUP_PREFIX};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SNAPSHOTS: &str = "Devils Rage snapshots";
pub const ARCHIVE: &str = "Devils Rage archive";
const INFO: &str = "snapshot.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Info {
    #[serde(default)]
    note: String,
    /// When it was taken (ms).
    #[serde(default)]
    at: u64,
    #[serde(default)]
    files: usize,
    /// Taken by the app itself (before a restore or a delete).
    #[serde(default)]
    auto: bool,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub profile: String,
    pub id: String,
    pub note: String,
    pub at: u64,
    pub files: usize,
    pub auto: bool,
}

#[derive(Debug, Serialize)]
pub struct Archived {
    pub id: String,
    pub profile: String,
    pub at: u64,
    pub files: usize,
}

fn now() -> (i64, u64) {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() as i64, d.as_millis() as u64)
}

/// "2026-11-23 1702 UTC" (the backup naming, without its prefix).
fn stamp(secs: i64) -> String {
    format!("{} UTC", backup_name(secs, 0).trim_start_matches(BACKUP_PREFIX))
}

fn free_name(parent: &Path, base: &str) -> PathBuf {
    let mut path = parent.join(base);
    let mut n = 2;
    while path.exists() {
        path = parent.join(format!("{base} ({n})"));
        n += 1;
    }
    path
}

fn profile_dir(root: &Path, profile: &str) -> Result<PathBuf, String> {
    if !safe_name(profile) {
        return Err("Not a profile name.".into());
    }
    let dir = root.join(format!("settings_{profile}"));
    if !dir.is_dir() {
        return Err(format!("settings_{profile} isn't there."));
    }
    Ok(dir)
}

/// A snapshot / archive folder name from the page: one plain folder name, nothing that climbs out.
fn plain(id: &str) -> Result<&str, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") || id.len() > 120 {
        return Err("Not one of the app's snapshots.".into());
    }
    Ok(id)
}

/// The files directly in a folder (never its subfolders).
fn files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).map(|e| e.path()).collect()
}

/// Copy a profile's files into a new snapshot. Reading only, so clients may be open (it then has
/// what EVE last saved, which the note says).
fn take(root: &Path, profile: &str, note: &str, auto: bool) -> Result<(String, usize), String> {
    let dir = profile_dir(root, profile)?;
    let (secs, ms) = now();
    let parent = root.join(SNAPSHOTS).join(profile);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let target = free_name(&parent, &stamp(secs));
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut files = 0;
    for file in files_in(&dir) {
        let Some(name) = file.file_name() else { continue };
        std::fs::copy(&file, target.join(name)).map_err(|e| format!("couldn't copy {}: {e}", name.to_string_lossy()))?;
        files += 1;
    }
    let info = Info { note: note.trim().chars().take(80).collect(), at: ms, files, auto };
    std::fs::write(target.join(INFO), serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let id = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    log::info!("snapshot of settings_{profile}: {id} ({files} files)");
    Ok((id, files))
}

/// A snapshot the app takes itself before changing a profile's files (Place clients).
pub(crate) fn auto_snapshot(profile: &str, note: &str) -> Result<String, String> {
    take(&root()?, profile, note, true).map(|(id, _)| id)
}

/// Take a snapshot of one profile now, with a note ("before the overview change").
#[tauri::command]
pub fn profile_snapshot(profile: String, note: String) -> Result<String, String> {
    let root = root()?;
    let (id, files) = take(&root, &profile, &note, false)?;
    let open = crate::profiles::clients_open();
    Ok(format!(
        "Snapshot of settings_{profile} saved ({id}, {files} files).{}",
        if open > 0 { " EVE clients are open: it has what EVE last saved; a client's changes since it started go in when it closes." } else { "" }
    ))
}

/// Every snapshot, newest first.
#[tauri::command]
pub fn profile_snapshots() -> Result<Vec<Snapshot>, String> {
    let base = root()?.join(SNAPSHOTS);
    let mut out = Vec::new();
    for profile in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let name = profile.file_name().to_string_lossy().to_string();
        if !safe_name(&name) || !profile.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for snap in std::fs::read_dir(profile.path()).into_iter().flatten().flatten() {
            if !snap.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let info: Info = std::fs::read_to_string(snap.path().join(INFO)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let files = if info.files > 0 { info.files } else { files_in(&snap.path()).len() };
            out.push(Snapshot { profile: name.clone(), id: snap.file_name().to_string_lossy().to_string(), note: info.note, at: info.at, files, auto: info.auto });
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    Ok(out)
}

/// Put a snapshot back: the profile's files become exactly the snapshot's (files it didn't have
/// are removed; the Browser cache is left alone). What was there is snapshotted first.
#[tauri::command]
pub fn profile_snapshot_restore(profile: String, id: String) -> Result<String, String> {
    crate::pcaccounts::require_eve_closed()?;
    let root = root()?;
    let from = root.join(SNAPSHOTS).join(&profile).join(plain(&id)?);
    if !safe_name(&profile) || !from.is_dir() {
        return Err("That snapshot is gone.".into());
    }
    let dir = root.join(format!("settings_{profile}"));
    let before = if dir.is_dir() { Some(take(&root, &profile, "before putting back a snapshot", true)?.0) } else { None };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for file in files_in(&dir) {
        std::fs::remove_file(&file).map_err(|e| e.to_string())?;
    }
    let mut files = 0;
    for file in files_in(&from) {
        let Some(name) = file.file_name() else { continue };
        if name == INFO {
            continue;
        }
        std::fs::copy(&file, dir.join(name)).map_err(|e| e.to_string())?;
        files += 1;
    }
    log::info!("settings_{profile} put back to snapshot {id} ({files} files)");
    Ok(match before {
        Some(b) => format!("settings_{profile} is back to {id} ({files} files). What was there is snapshot {b}."),
        None => format!("settings_{profile} is back ({files} files from {id})."),
    })
}

/// Delete one snapshot for good.
#[tauri::command]
pub fn profile_snapshot_delete(profile: String, id: String) -> Result<String, String> {
    let root = root()?;
    let dir = root.join(SNAPSHOTS).join(&profile).join(plain(&id)?);
    if !safe_name(&profile) || !dir.is_dir() {
        return Err("That snapshot is gone.".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    log::info!("deleted snapshot {id} of settings_{profile}");
    Ok(format!("Deleted snapshot {id}."))
}

/// Move a whole profile folder into the archive: the launcher stops listing it, nothing is lost.
#[tauri::command]
pub fn profile_folder_archive(profile: String) -> Result<String, String> {
    crate::pcaccounts::require_eve_closed()?;
    let root = root()?;
    let dir = profile_dir(&root, &profile)?;
    let (secs, _) = now();
    let parent = root.join(ARCHIVE);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let target = free_name(&parent, &format!("settings_{profile} {}", stamp(secs)));
    std::fs::rename(&dir, &target).map_err(|e| format!("couldn't move settings_{profile}: {e}"))?;
    log::info!("archived settings_{profile} as {}", target.display());
    Ok(format!("Archived settings_{profile}. The launcher won't list it any more (restart it); Put back under Archived profiles brings it back."))
}

/// Archived folders, newest first.
#[tauri::command]
pub fn profile_archived() -> Result<Vec<Archived>, String> {
    let base = root()?.join(ARCHIVE);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let Some(profile) = id.split(' ').next().and_then(profile_of).map(str::to_string) else { continue };
        let at = std::fs::metadata(entry.path()).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
        out.push(Archived { files: files_in(&entry.path()).len(), id, profile, at });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

/// Put an archived folder back as settings_<profile> (only when that name is free).
#[tauri::command]
pub fn profile_folder_unarchive(id: String) -> Result<String, String> {
    crate::pcaccounts::require_eve_closed()?;
    let root = root()?;
    let from = root.join(ARCHIVE).join(plain(&id)?);
    let profile = id.split(' ').next().and_then(profile_of).ok_or("Not one of the app's archived profiles.")?.to_string();
    if !from.is_dir() {
        return Err("That archived profile is gone.".into());
    }
    let target = root.join(format!("settings_{profile}"));
    if target.exists() {
        return Err(format!("settings_{profile} is there again: archive or rename that one first."));
    }
    std::fs::rename(&from, &target).map_err(|e| e.to_string())?;
    log::info!("put back settings_{profile} from {id}");
    Ok(format!("settings_{profile} is back. Restart the EVE launcher to see it."))
}

/// Delete an archived folder for good.
#[tauri::command]
pub fn profile_archived_delete(id: String) -> Result<String, String> {
    let root = root()?;
    let dir = root.join(ARCHIVE).join(plain(&id)?);
    if !dir.is_dir() {
        return Err("That archived profile is gone.".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    log::info!("deleted archived {id}");
    Ok(format!("Deleted {id} for good."))
}

/// Delete a profile folder. A snapshot of its files is kept first (delete that too to remove
/// every trace); the Browser cache goes with the folder.
#[tauri::command]
pub fn profile_folder_delete(profile: String) -> Result<String, String> {
    crate::pcaccounts::require_eve_closed()?;
    let root = root()?;
    let dir = profile_dir(&root, &profile)?;
    let (snap, _) = take(&root, &profile, "before deleting the folder", true)?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("couldn't delete settings_{profile}: {e}"))?;
    log::info!("deleted settings_{profile} (snapshot {snap} kept)");
    Ok(format!("Deleted settings_{profile}. Its files are kept in snapshot {snap} (Put back brings the folder back); restart the EVE launcher."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside() {
        assert!(plain("2026-11-23 1702 UTC").is_ok());
        assert!(plain("..").is_err());
        assert!(plain("a\\b").is_err());
        assert!(plain("").is_err());
        assert_eq!(stamp(0), "1970-01-01 0000 UTC");
    }
}
