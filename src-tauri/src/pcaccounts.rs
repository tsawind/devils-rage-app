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

    fn folder(profile: &str, users: &[u64], chars: &[u64]) -> Folder {
        Folder { profile: profile.into(), users: users.iter().copied().collect(), chars: chars.iter().map(|c| (*c, 1)).collect() }
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
    use tauri::Manager;
    app.path().app_config_dir().ok().map(|dir| dir.join("pc-accounts.json"))
}

fn with_seen<T>(app: &tauri::AppHandle, f: impl FnOnce(&mut BTreeMap<String, u64>) -> T) -> T {
    let mut guard = SEEN.lock().unwrap();
    let seen = guard.get_or_insert_with(|| seen_file(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(seen)
}

/// Note the EVE account of every logged-in client whose start-up line has been read (the rest are
/// read in the background and noted next time).
pub fn note_running(app: &tauri::AppHandle) {
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
