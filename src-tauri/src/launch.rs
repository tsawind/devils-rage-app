//! Patch 63: launch groups, defined right in the app. A group is a named set of characters;
//! "Open launcher" (option A) starts the official EVE launcher, which does all the sign-in
//! itself — the app never holds any EVE password or token. (Direct launch, option B, is a
//! separate, experimental path discussed with Tsa; it is not wired here.)
//!
//! The list lives in launch-groups.json next to settings.json; characters are the EVE clients
//! open now plus whatever a group already names, so you build groups from who you actually run.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    /// Members switched off: kept in the group (one click to turn back on) but skipped when
    /// cycling. EVE-O Preview's "!Name" entries come in this way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Patch 63b: a hotkey (e.g. the pedal's "F13") that cycles this group forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_hotkey: Option<String>,
    /// And one that cycles it backward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backward_hotkey: Option<String>,
    /// Patch 116 (stacks): the color of this group's square / frame on the previews (#rrggbb).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Patch 116: move a client's EVE window into this stack's spot when it joins (its square
    /// on the preview) and back to where it was when it leaves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember_windows: bool,
    /// Patch 120: in the group and set up, but put away for now: not cycled, no square or frame
    /// on its preview. Shown again from the Launch groups page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Groups {
    #[serde(default)]
    groups: Vec<Group>,
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join("launch-groups.json"))
}

fn load(app: &AppHandle) -> Groups {
    store_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Your launch groups.
#[tauri::command]
pub fn launch_groups(app: AppHandle) -> Vec<Group> {
    load(&app).groups
}

/// Save the whole list (the editor sends it back after any change).
#[tauri::command]
pub fn save_launch_groups(app: AppHandle, groups: Vec<Group>) -> Result<(), String> {
    let path = store_path(&app).ok_or("no config folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let cleaned: Vec<Group> = groups
        .into_iter()
        .take(50)
        .map(|g| {
            let members: Vec<String> = g.members.into_iter().take(150).collect();
            let skipped = g.skipped.into_iter().filter(|name| members.contains(name)).collect();
            let hidden = g.hidden.into_iter().filter(|name| members.contains(name)).collect();
            Group {
            id: g.id,
            name: g.name.chars().take(60).collect(),
            members,
            skipped,
            forward_hotkey: g.forward_hotkey.filter(|h| !h.trim().is_empty()),
            backward_hotkey: g.backward_hotkey.filter(|h| !h.trim().is_empty()),
            color: clean_color(g.color),
            remember_windows: g.remember_windows,
            hidden,
            }
        })
        .collect();
    let text = serde_json::to_string_pretty(&Groups { groups: cleaned }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    #[cfg(desktop)]
    register_cycle_hotkeys(&app);
    crate::previews::refresh_modes(&app);
    Ok(())
}

/// Patch 63c: groups brought in from an EVE-O Preview config. A group with the same name is
/// replaced (so importing again updates it); the rest stay.
pub fn merge_imported(app: &AppHandle, imported: Vec<Group>) -> Result<usize, String> {
    let mut groups = load(app).groups;
    let count = imported.len();
    for group in imported {
        groups.retain(|known| known.name != group.name);
        crate::cycle::forget(&group.id);
        groups.push(group);
    }
    save_launch_groups(app.clone(), groups)?;
    Ok(count)
}

// ---- Patch 66: launch groups from SeAT (Your accounts) ------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatCharacter {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatAccount {
    pub id: u64,
    pub nickname: String,
    /// The client profile number it stands for ("29" → settings_29).
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub characters: Vec<SeatCharacter>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatMember {
    pub name: String,
    pub account: String,
    #[serde(default)]
    pub character_id: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatGroup {
    pub id: u64,
    pub name: String,
    /// own | corp | corp-slot (Main / Second / Third).
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub members: Vec<SeatMember>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeatData {
    #[serde(default)]
    pub synced_at: i64,
    #[serde(default)]
    pub accounts: Vec<SeatAccount>,
    #[serde(default)]
    pub groups: Vec<SeatGroup>,
}

/// What this PC adds to a SeAT group: pedal keys, switched-off members, login screens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backward_hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub login_screens: bool,
    /// Patch 116 (stacks): the color of this group's square / frame on the previews (#rrggbb).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Patch 116: move a client's EVE window into this stack's spot when it joins (its square
    /// on the preview) and back to where it was when it leaves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember_windows: bool,
    /// Patch 120: in the group and set up, but put away for now: not cycled, no square or frame
    /// on its preview. Shown again from the Launch groups page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SeatPrefs {
    #[serde(default)]
    groups: std::collections::BTreeMap<String, GroupPrefs>,
}

fn config_file(app: &AppHandle, name: &str) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(name))
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn load_seat(app: &AppHandle) -> SeatData {
    config_file(app, "seat-groups.json").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn load_prefs(app: &AppHandle) -> SeatPrefs {
    config_file(app, "seat-group-prefs.json").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// This PC's pedal keys and switches for one SeAT group (defaults when none saved).
pub(crate) fn seat_group_prefs(app: &AppHandle, id: u64) -> GroupPrefs {
    load_prefs(app).groups.remove(&id.to_string()).unwrap_or_default()
}

/// The Your accounts page handed us fresh groups (seatsync.rs).
pub fn store_seat(app: &AppHandle, data: &SeatData) -> Result<(), String> {
    write_json(config_file(app, "seat-groups.json").ok_or("no config folder")?, data)?;
    log::info!("launch groups from SeAT: {} accounts, {} groups", data.accounts.len(), data.groups.len());
    #[cfg(desktop)]
    register_cycle_hotkeys(app);
    crate::previews::refresh_modes(app);
    Ok(())
}

/// The groups made on this PC, for SeAT to bring in (names only; login screens aren't characters).
pub fn local_for_import(app: &AppHandle) -> Vec<serde_json::Value> {
    load(app)
        .groups
        .into_iter()
        .map(|g| {
            let members: Vec<String> = g.members.into_iter().filter(|m| m != crate::cycle::LOGIN_SCREENS && !g.skipped.contains(m)).collect();
            serde_json::json!({ "name": g.name, "members": members })
        })
        .filter(|g| g["members"].as_array().is_some_and(|m| !m.is_empty()))
        .collect()
}

/// After SeAT took them: keep a copy (launch-groups.imported.json) and clear the PC's list,
/// carrying each group's pedal keys over to the SeAT group of the same name.
pub fn retire_local(app: &AppHandle) -> Result<(), String> {
    let local = load(app).groups;
    let seat = load_seat(app);
    let mut prefs = load_prefs(app);
    for group in &local {
        if let Some(target) = seat.groups.iter().find(|s| s.name == group.name || s.name == format!("{} (mine)", group.name)) {
            let entry = prefs.groups.entry(target.id.to_string()).or_default();
            entry.forward_hotkey = entry.forward_hotkey.take().or(group.forward_hotkey.clone());
            entry.backward_hotkey = entry.backward_hotkey.take().or(group.backward_hotkey.clone());
            entry.login_screens = entry.login_screens || group.members.iter().any(|m| m == crate::cycle::LOGIN_SCREENS);
        }
    }
    write_json(config_file(app, "seat-group-prefs.json").ok_or("no config folder")?, &prefs)?;
    if let Some(path) = store_path(app) {
        if path.exists() {
            let _ = std::fs::copy(&path, path.with_file_name("launch-groups.imported.json"));
        }
        write_json(path, &Groups { groups: Vec::new() })?;
    }
    #[cfg(desktop)]
    register_cycle_hotkeys(app);
    Ok(())
}

/// Every group the pedal can cycle: SeAT's (with this PC's keys and switches), then the PC's own.
pub fn all_groups(app: &AppHandle) -> Vec<Group> {
    let prefs = load_prefs(app).groups;
    let mut out: Vec<Group> = load_seat(app)
        .groups
        .into_iter()
        .map(|g| {
            let p = prefs.get(&g.id.to_string()).cloned().unwrap_or_default();
            let mut members: Vec<String> = g.members.into_iter().map(|m| m.name).collect();
            if p.login_screens {
                members.push(crate::cycle::LOGIN_SCREENS.to_string());
            }
            Group { id: format!("seat-{}", g.id), name: g.name, members, skipped: p.skipped, forward_hotkey: p.forward_hotkey, backward_hotkey: p.backward_hotkey, color: p.color, remember_windows: p.remember_windows, hidden: p.hidden }
        })
        .collect();
    out.extend(load(app).groups);
    out
}

/// Patch 66b: the app asks SeAT itself, signed in as you (the SeAT page's own cookies), instead
/// of waiting for the Your accounts page to hand the groups over.
pub(crate) fn seat_session(app: &AppHandle) -> Result<(String, Option<String>), String> {
    let view = app.get_webview(crate::PAGES_VIEW).ok_or("the app's page view isn't open")?;
    let url: tauri::Url = crate::SEAT_URL.parse().map_err(|e| format!("{e}"))?;
    let cookies = view.cookies_for_url(url).map_err(|e| e.to_string())?;
    if cookies.is_empty() {
        return Err("not signed in to SeAT in the app yet: open SeAT or Your accounts once".into());
    }
    let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
    // Laravel takes its XSRF-TOKEN cookie back as X-XSRF-TOKEN for anything that saves.
    let xsrf = cookies.iter().find(|c| c.name() == "XSRF-TOKEN").map(|c| percent_decode(c.value()));
    Ok((header, xsrf))
}

/// Patch 149: a session that can save. The page view's XSRF cookie expires with the SeAT session,
/// so ask SeAT for a fresh one first (the remembered login signs us back in) and use the cookies
/// it hands back; fall back to what the page view has.
pub(crate) async fn seat_write_session(app: &AppHandle) -> Result<(String, String), String> {
    let (cookies, xsrf) = seat_session(app)?;
    let mut jar: Vec<(String, String)> = cookies.split("; ").filter_map(|c| c.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
    if let Ok(response) = crate::http_client()
        .get(format!("{}/devilsrage/accounts/mine.json", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", &cookies)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
            let Ok(text) = value.to_str() else { continue };
            let Some((name, rest)) = text.split(';').next().and_then(|c| c.split_once('=')) else { continue };
            jar.retain(|(k, _)| k != name);
            jar.push((name.to_string(), rest.to_string()));
        }
    }
    let fresh = jar.iter().find(|(k, _)| k == "XSRF-TOKEN").map(|(_, v)| percent_decode(v)).or(xsrf);
    let header = jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
    Ok((header, fresh.ok_or("SeAT's session is missing: open SeAT in the app once, then try again")?))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Patch 123: your characters' assets and your division hangar from SeAT, saved on this PC
/// (assets-snapshot.json); with `offline` (or SeAT unreachable), the last saved copy.
#[tauri::command]
pub async fn seat_assets(app: AppHandle, offline: bool, system: Option<String>, as_user: Option<i64>) -> Result<serde_json::Value, String> {
    // Viewing as another member (SeAT admins) or another system keeps its own saved copy.
    let file = match (&system, as_user) {
        (None, None) => "assets-snapshot.json".to_string(),
        _ => format!("assets-snapshot-{}-{}.json", system.as_deref().unwrap_or("home").replace(|c: char| !c.is_ascii_alphanumeric(), "_"), as_user.unwrap_or(0)),
    };
    let path = config_file(&app, &file).ok_or("no config folder")?;
    let saved = || std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    if offline {
        return saved().ok_or_else(|| "Nothing saved on this PC yet: load from SeAT once.".to_string());
    }
    let fresh: Result<serde_json::Value, String> = async {
        let (cookies, _) = seat_session(&app)?;
        let response = crate::http_client()
            .get(format!("{}/devilsrage/assets.json", crate::SEAT_URL))
            .query(&[("system", system.clone().unwrap_or_default()), ("as", as_user.map(|u| u.to_string()).unwrap_or_default())])
            .header("Accept", "application/json")
            .header("Cookie", cookies)
            .timeout(std::time::Duration::from_secs(90))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("SeAT answered {} (sign in to SeAT in the app again?)", response.status()));
        }
        response.json::<serde_json::Value>().await.map_err(|_| "SeAT didn't send assets (signed out? open SeAT once)".to_string())
    }
    .await;
    match fresh {
        Ok(data) => {
            write_json(path.clone(), &data)?;
            Ok(data)
        }
        Err(err) => saved().map(|mut d| {
            d["offlineBecause"] = serde_json::Value::String(err.clone());
            d
        }).ok_or(err),
    }
}

/// Patch 124: set or clear a plan on SeAT: scope "corp" (War / Being evicted, directors) or
/// "me" (Vacation, the others' directors see it).
#[tauri::command]
pub async fn seat_asset_mode(app: AppHandle, scope: String, mode: Option<String>, threat: Option<u8>, days: Option<u32>) -> Result<String, String> {
    let (cookies, xsrf) = crate::launch::seat_write_session(&app).await?;
    let response = crate::http_client()
        .post(format!("{}/devilsrage/assets/mode", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&serde_json::json!({ "scope": scope, "mode": mode, "threat": threat, "days": days }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("SeAT answered {status}")));
    }
    Ok(body["message"].as_str().unwrap_or("Saved.").to_string())
}

/// Patch 125: which characters the EVE launcher loaded for each account, from its logs on this
/// PC (%APPDATA%\EVE Online\logs): "Fetching details for N character(s) { characterIds: [...] }"
/// then "Fetched N character details { userId: ... }". The latest log wins.
pub fn launcher_accounts() -> Vec<(u64, Vec<u64>)> {
    let Some(dir) = dirs::config_dir().map(|d| d.join("EVE Online").join("logs")) else { return Vec::new() };
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|r| {
            r.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("eve-online-launcher-") && n.ends_with(".log")))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut accounts: std::collections::BTreeMap<u64, Vec<u64>> = std::collections::BTreeMap::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        accounts_from_log(&text, &mut accounts);
    }
    accounts.into_iter().filter(|(_, c)| !c.is_empty()).collect()
}

/// One launcher log: each "characterIds: [...]" belongs to the next "character details ... userId".
pub fn accounts_from_log(text: &str, accounts: &mut std::collections::BTreeMap<u64, Vec<u64>>) {
    const IDS: &str = "characterIds: [";
    const DONE: &str = "character details";
    let mut pending: Vec<u64> = Vec::new();
    let mut rest = text;
    loop {
        let fetching = rest.find(IDS);
        let fetched = rest.find(DONE);
        match (fetching, fetched) {
            (Some(a), b) if b.is_none_or(|b| a < b) => {
                let after = &rest[a + IDS.len()..];
                let end = after.find(']').unwrap_or(0);
                pending = after[..end].split(',').filter_map(|n| n.trim().parse().ok()).collect();
                rest = &after[end..];
            }
            (_, Some(b)) => {
                let after = &rest[b + DONE.len()..];
                let line = after.lines().next().unwrap_or("");
                let user = line.find("userId:").and_then(|i| line[i + 7..].trim_start().split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse::<u64>().ok()));
                if let Some(user) = user.filter(|_| !pending.is_empty()) {
                    // A character moved to another account: drop it from the old one.
                    for list in accounts.values_mut() {
                        list.retain(|c| !pending.contains(c));
                    }
                    accounts.insert(user, std::mem::take(&mut pending));
                }
                rest = after;
            }
            _ => break,
        }
    }
}

/// Patch 125: group Your accounts on SeAT from the launcher's logs. Only characters SeAT already
/// has for you are sent (characters it doesn't know never leave this PC).
#[tauri::command]
pub async fn group_accounts_from_launcher(app: AppHandle) -> Result<String, String> {
    let found = launcher_accounts();
    if found.is_empty() {
        return Err("No accounts in the EVE launcher's logs on this PC (open the launcher once).".into());
    }
    let (cookies, xsrf) = crate::launch::seat_write_session(&app).await?;
    let client = crate::http_client();
    let mine: serde_json::Value = client
        .get(format!("{}/devilsrage/accounts/mine.json", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|_| "SeAT didn't answer (signed out? open SeAT once)".to_string())?;
    let known: std::collections::HashSet<u64> = mine["character_ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
    let accounts: Vec<serde_json::Value> = found
        .into_iter()
        .filter_map(|(user, chars)| {
            let chars: Vec<u64> = chars.into_iter().filter(|c| known.contains(c)).take(3).collect();
            (!chars.is_empty()).then(|| serde_json::json!({ "user_id": user, "character_ids": chars }))
        })
        .collect();
    if accounts.is_empty() {
        return Err("None of the launcher's characters are on SeAT for you.".into());
    }
    let response = client
        .post(format!("{}/devilsrage/accounts/launcher-import", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&serde_json::json!({ "accounts": accounts }))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("SeAT answered {status}")));
    }
    Ok(body["message"].as_str().unwrap_or("Done.").to_string())
}

pub(crate) async fn fetch_seat(app: &AppHandle) -> Result<SeatData, String> {
    let (cookies, _) = seat_session(app)?;
    let response = crate::http_client()
        .get(format!("{}/devilsrage/launch-groups.json", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("SeAT answered {} (sign in to SeAT in the app again?)", response.status()));
    }
    response.json::<SeatData>().await.map_err(|_| "SeAT didn't send launch groups (signed out? open Your accounts once)".to_string())
}

/// Read your accounts and launch groups from SeAT now.
#[tauri::command]
pub async fn seat_refresh(app: AppHandle) -> Result<String, String> {
    let data = fetch_seat(&app).await?;
    store_seat(&app, &data)?;
    Ok(format!("{} accounts, {} launch groups from Your accounts.", data.accounts.len(), data.groups.len()))
}

/// Bring this PC's groups (e.g. from EVE-O Preview) into Your accounts, then read SeAT again.
#[tauri::command]
pub async fn seat_import_local(app: AppHandle) -> Result<String, String> {
    let groups = local_for_import(&app);
    if groups.is_empty() {
        return Err("No groups on this PC to bring in.".into());
    }
    let (cookies, xsrf) = crate::launch::seat_write_session(&app).await?;
    let response = crate::http_client()
        .post(format!("{}/devilsrage/launch-groups/import", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Referer", format!("{}/devilsrage/your-accounts", crate::SEAT_URL))
        .json(&serde_json::json!({ "groups": groups }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("SeAT answered {}", response.status()));
    }
    let report: serde_json::Value = response.json().await.unwrap_or_default();
    let data = fetch_seat(&app).await?;
    store_seat(&app, &data)?;
    retire_local(&app)?;
    let missing = report["missing"].as_array().map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "Brought in: {} new group(s), {} character(s) ticked{}.",
        report["made"].as_i64().unwrap_or(0),
        report["ticked"].as_i64().unwrap_or(0),
        if missing > 0 { format!(", {missing} not found in your accounts") } else { String::new() }
    ))
}

/// SeAT's accounts and groups with this PC's settings for each, for the Launch groups page.
#[tauri::command]
pub fn seat_launch_groups(app: AppHandle) -> serde_json::Value {
    let seat = load_seat(&app);
    let prefs = load_prefs(&app).groups;
    let groups: Vec<serde_json::Value> = seat
        .groups
        .iter()
        .map(|g| serde_json::json!({ "group": g, "prefs": prefs.get(&g.id.to_string()).cloned().unwrap_or_default() }))
        .collect();
    serde_json::json!({ "syncedAt": seat.synced_at, "accounts": seat.accounts, "groups": groups, "seat": crate::SEAT_URL })
}

/// Save this PC's settings for one SeAT group.
#[tauri::command]
pub fn save_seat_group_prefs(app: AppHandle, id: u64, prefs: GroupPrefs) -> Result<(), String> {
    let mut all = load_prefs(&app);
    let clean = GroupPrefs {
        forward_hotkey: prefs.forward_hotkey.filter(|h| !h.trim().is_empty()),
        backward_hotkey: prefs.backward_hotkey.filter(|h| !h.trim().is_empty()),
        skipped: prefs.skipped.into_iter().take(150).collect(),
        login_screens: prefs.login_screens,
        color: clean_color(prefs.color),
        remember_windows: prefs.remember_windows,
        hidden: prefs.hidden.into_iter().take(150).collect(),
    };
    crate::cycle::forget(&format!("seat-{id}"));
    all.groups.insert(id.to_string(), clean);
    write_json(config_file(&app, "seat-group-prefs.json").ok_or("no config folder")?, &all)?;
    #[cfg(desktop)]
    register_cycle_hotkeys(&app);
    crate::previews::refresh_modes(&app);
    Ok(())
}

fn has_hotkey(group: &Group) -> bool {
    [&group.forward_hotkey, &group.backward_hotkey].iter().any(|h| h.as_deref().is_some_and(|h| !h.trim().is_empty()))
}

/// Colors for stacks that haven't picked one: orange, green, blue, purple, cyan, pink.
const STACK_COLORS: [&str; 6] = ["#ff8c1a", "#3ccf4e", "#3d8bff", "#b36bff", "#20c8d8", "#ff5fa2"];

fn clean_color(color: Option<String>) -> Option<String> {
    color.filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
}

/// Patch 116: the stacks (groups with a pedal / hotkey), in the stack order (patch 120: set on the
/// Launch groups page; stacks not in it yet follow, as the page lists them).
pub fn stack_groups(app: &AppHandle) -> Vec<Group> {
    let order = load_stack_order(app);
    let mut stacks: Vec<Group> = all_groups(app).into_iter().filter(has_hotkey).collect();
    stacks.sort_by_key(|g| order.iter().position(|id| *id == g.id).unwrap_or(usize::MAX));
    stacks
}

fn load_stack_order(app: &AppHandle) -> Vec<String> {
    config_file(app, "stack-order.json").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Patch 120: the stacks in order (first = the square furthest right on the previews).
#[tauri::command]
pub fn stack_order(app: AppHandle) -> Vec<serde_json::Value> {
    stack_groups(&app)
        .iter()
        .enumerate()
        .map(|(i, g)| serde_json::json!({ "id": g.id, "name": g.name, "color": g.color.clone().unwrap_or_else(|| STACK_COLORS[i % STACK_COLORS.len()].to_string()) }))
        .collect()
}

/// Patch 120: save the stack order (the page's ▲ ▼).
#[tauri::command]
pub fn save_stack_order(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    write_json(config_file(&app, "stack-order.json").ok_or("no config folder")?, &ids.into_iter().take(100).collect::<Vec<_>>())?;
    crate::previews::refresh_modes(&app);
    Ok(())
}

/// Where this character stands in each stack, for its preview's squares: "on" (cycled), "off"
/// (in the stack, switched off: dead, flying something else), "out" (not in it).
pub fn stacks_for(app: &AppHandle, name: &str) -> Vec<serde_json::Value> {
    stack_groups(app)
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let state = if !g.members.iter().any(|m| m == name) || g.hidden.iter().any(|h| h == name) {
                "out"
            } else if g.skipped.iter().any(|s| s == name) {
                "off"
            } else {
                "on"
            };
            serde_json::json!({
                "id": g.id,
                "name": g.name,
                "color": g.color.clone().unwrap_or_else(|| STACK_COLORS[i % STACK_COLORS.len()].to_string()),
                "remember": g.remember_windows,
                "state": state,
            })
        })
        .collect()
}

/// Patch 116: a preview's square for one stack. On: the character joins it (switched back on,
/// or added at the end of its list); off: switched off in it. With the stack's "remember window
/// locations", its EVE window moves into the stack's spot and back out (clientwindows.rs).
pub fn set_stack_member(app: &AppHandle, group_id: &str, name: &str, on: bool) -> Result<(), String> {
    let group = stack_groups(app).into_iter().find(|g| g.id == group_id).ok_or("That stack is gone (no pedal key any more?)")?;
    let toggle = |skipped: &mut Vec<String>| {
        skipped.retain(|s| s != name);
        if !on {
            skipped.push(name.to_string());
        }
    };
    if let Some(seat_id) = group_id.strip_prefix("seat-") {
        if !group.members.iter().any(|m| m == name) {
            return Err(format!("{name} isn't in {}: tick that group on {name} in Your accounts first.", group.name));
        }
        let mut prefs = load_prefs(app);
        toggle(&mut prefs.groups.entry(seat_id.to_string()).or_default().skipped);
        write_json(config_file(app, "seat-group-prefs.json").ok_or("no config folder")?, &prefs)?;
    } else {
        let mut groups = load(app).groups;
        let g = groups.iter_mut().find(|g| g.id == group_id).ok_or("That stack is gone")?;
        if g.members.iter().any(|m| m == name) {
            toggle(&mut g.skipped);
        } else if on {
            g.members.push(name.to_string());
        }
        save_launch_groups(app.clone(), groups)?;
    }
    // Stacks it's still cycled in that move windows (for where its window goes on leaving).
    let still_on: Vec<String> = stack_groups(app)
        .into_iter()
        .filter(|g| g.id != group_id && g.remember_windows && g.members.iter().any(|m| m == name) && !g.skipped.iter().any(|s| s == name) && !g.hidden.iter().any(|s| s == name))
        .map(|g| g.id)
        .collect();
    crate::clientwindows::stack_changed(app, name, group_id, on, group.remember_windows, &still_on);
    Ok(())
}

/// Every group (SeAT's and this PC's), e.g. for Client settings' "tick a launch group".
#[tauri::command]
pub fn all_launch_groups(app: AppHandle) -> Vec<Group> {
    all_groups(&app)
}

/// Characters to build groups from: the EVE clients open now (so you pick who you actually run).
#[tauri::command]
pub fn launch_known_characters() -> Vec<String> {
    crate::clipboard::open_eve_windows().into_iter().map(|(name, _)| name).collect()
}

/// Live state for the launch-groups page: which characters are open, how many clients sit at
/// the login screen, and which client is in front.
#[tauri::command]
pub fn launch_status() -> serde_json::Value {
    serde_json::json!({
        "open": crate::clipboard::open_eve_windows().into_iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "logins": crate::clipboard::open_login_windows().len(),
        "active": last_in_front(),
    })
}

/// The EVE client in front, or the last one that was (this page is in front while you look at it).
fn last_in_front() -> Option<String> {
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if let Some(front) = crate::clipboard::foreground_client() {
        *last = Some(front);
    }
    last.clone()
}

/// The official EVE launcher's exe: the saved path, else the usual install spots.
fn find_launcher(app: &AppHandle) -> Option<PathBuf> {
    if let Some(saved) = crate::settings::load(app).launcher_path {
        let p = PathBuf::from(saved);
        if p.is_file() {
            return Some(p);
        }
    }
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let candidates = [
        local.as_ref().map(|d| d.join(r"eve-online\eve-online.exe")),
        local.as_ref().map(|d| d.join(r"Programs\eve-online\eve-online.exe")),
        Some(PathBuf::from(r"C:\Program Files\EVE\eve-online.exe")),
        Some(PathBuf::from(r"C:\Program Files (x86)\EVE\eve-online.exe")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// Option A: open the official EVE launcher (it signs in and launches). The group is for your
/// reference — click its clients in the launcher. Returns the exe used.
#[tauri::command]
pub fn open_launcher(app: AppHandle) -> Result<String, String> {
    let exe = find_launcher(&app).ok_or_else(|| {
        "Couldn't find the EVE launcher. Set its path in Settings (point to eve-online.exe).".to_string()
    })?;
    Command::new(&exe)
        .spawn()
        .map_err(|e| format!("couldn't start the launcher: {e}"))?;
    log::info!("opened the EVE launcher: {}", exe.display());
    Ok(exe.display().to_string())
}

/// Whether we can find the launcher (so the UI can nudge you to set the path).
#[tauri::command]
pub fn launcher_ready(app: AppHandle) -> bool {
    find_launcher(&app).is_some()
}

/// Patch 63b: the foot-pedal / hotkey cycling. Registers every group's cycle hotkeys as global
/// shortcuts; when one fires, brings the next open client in that group to the front.
#[cfg(desktop)]
mod hotkeys {
    use super::{all_groups, Group};
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::Mutex;
    use tauri::AppHandle;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    /// Each registered shortcut -> (group id, forward?).
    static BOUND: Mutex<Vec<(Shortcut, String, bool)>> = Mutex::new(Vec::new());

    pub fn register(app: &AppHandle) {
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let mut bound = BOUND.lock().unwrap();
        bound.clear();
        let mut taken: HashSet<String> = HashSet::new();
        for group in all_groups(app) {
            for (hotkey, forward) in [(group.forward_hotkey.as_deref(), true), (group.backward_hotkey.as_deref(), false)] {
                let Some(hotkey) = hotkey.filter(|h| !h.trim().is_empty()) else { continue };
                // One key drives one group: the first one listed (SeAT's, then this PC's) keeps it.
                if !taken.insert(hotkey.to_ascii_uppercase()) {
                    log::warn!("cycle hotkey {hotkey} is already used by another group; {} skipped", group.name);
                    continue;
                }
                match Shortcut::from_str(hotkey) {
                    Ok(shortcut) => {
                        if gs.register(shortcut).is_ok() {
                            bound.push((shortcut, group.id.clone(), forward));
                        } else {
                            log::warn!("couldn't register cycle hotkey {hotkey} (in use by another app?)");
                        }
                    }
                    Err(_) => log::warn!("cycle hotkey {hotkey} isn't a key we understand"),
                }
            }
        }
        log::info!("cycle hotkeys registered: {}", bound.len());
    }

    /// A registered cycle shortcut fired: advance that group and bring the next client forward.
    pub fn fired(app: &AppHandle, shortcut: &Shortcut) {
        let hit = BOUND.lock().unwrap().iter().find(|(s, _, _)| s == shortcut).map(|(_, id, fwd)| (id.clone(), *fwd));
        let Some((group_id, forward)) = hit else { return };
        let group: Option<Group> = all_groups(app).into_iter().find(|g| g.id == group_id);
        let Some(group) = group else { return };
        let logins = crate::clipboard::open_login_windows();
        let active: Vec<String> = group.members.iter().filter(|m| !group.skipped.contains(m) && !group.hidden.contains(m)).cloned().collect();
        let order = crate::cycle::expand_login(&active, &logins);
        let mut open: HashSet<String> = crate::clipboard::open_eve_windows().into_iter().map(|(name, _)| name).collect();
        open.extend(logins.iter().map(|h| format!("login:{h}")));
        // Step from the client in front now (a login window counts by its handle).
        let front = crate::clipboard::foreground_handle();
        let focused = if logins.contains(&front) { Some(format!("login:{front}")) } else { crate::clipboard::foreground_client() };
        if let Some(next) = crate::cycle::advance(&group_id, &order, &open, focused.as_deref(), forward) {
            match crate::cycle::login_handle(&next) {
                Some(handle) => crate::capture::focus_handle(handle),
                None => crate::capture::focus_client(&next),
            };
            crate::previews::push_active(app, Some(&next).filter(|n| !n.starts_with("login:")).map(String::as_str));
            log::info!("cycle {}: brought {next} forward", group.name);
        }
    }
}

#[cfg(desktop)]
pub fn register_cycle_hotkeys(app: &AppHandle) {
    hotkeys::register(app);
}

/// Let go of the cycle hotkeys while the editor records a new key (else the pedal never reaches it).
#[tauri::command]
pub fn pause_cycle_hotkeys(app: AppHandle) {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let _ = app.global_shortcut().unregister_all();
    }
    #[cfg(not(desktop))]
    let _ = app;
}

/// Take the cycle hotkeys back after recording.
#[tauri::command]
pub fn resume_cycle_hotkeys(app: AppHandle) {
    #[cfg(desktop)]
    register_cycle_hotkeys(&app);
    #[cfg(not(desktop))]
    let _ = app;
}

#[cfg(desktop)]
pub fn cycle_shortcut_fired(app: &AppHandle, shortcut: &tauri_plugin_global_shortcut::Shortcut) {
    hotkeys::fired(app, shortcut);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_log_groups_characters_by_account() {
        let log = "2026-11-14 16:29:13.100    app     info:    [esi] Fetching details for 3 character(s)    { characterIds: [ 94408493, 96155001, 2122190658 ] }\n\
2026-11-14 16:29:13.630    app     info:    [virtual-goods] Fetched Plex status    {\n  userId: 10979282,\n  balance: 0\n}\n\
2026-11-14 16:29:13.700    app     info:    [esi] Fetched 3 character details    { userId: 10979282 }\n\
2026-11-14 16:38:42.000    app     info:    [esi] Fetching details for 2 character(s)    { characterIds: [ 93976142, 96155001 ] }\n\
2026-11-14 16:38:42.300    app     info:    [esi] Fetched 2 character details    { userId: 7205846 }\n";
        let mut accounts = std::collections::BTreeMap::new();
        accounts_from_log(log, &mut accounts);
        // 96155001 moved to the second account in the later lines.
        assert_eq!(accounts[&10979282], vec![94408493, 2122190658]);
        assert_eq!(accounts[&7205846], vec![93976142, 96155001]);
    }

    #[test]
    fn groups_round_trip_through_json() {
        let groups = Groups {
            groups: vec![Group { id: "a".into(), name: "Roll fleet".into(), members: vec!["Tsawind".into(), "LuigiSquirrel".into()], skipped: vec![], forward_hotkey: Some("F13".into()), backward_hotkey: None, ..Default::default() }],
        };
        let text = serde_json::to_string(&groups).unwrap();
        let back: Groups = serde_json::from_str(&text).unwrap();
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].members, vec!["Tsawind", "LuigiSquirrel"]);
    }

    #[test]
    fn a_missing_members_field_is_an_empty_group() {
        let back: Groups = serde_json::from_str(r#"{"groups":[{"id":"x","name":"Empty"}]}"#).unwrap();
        assert_eq!(back.groups[0].members.len(), 0);
    }
}
