//! Instant location: follows EVE's Local chat logs (chatlog.rs) and tells the mapper the
//! moment one of your characters changes system, instead of waiting for ESI.
//!
//! The app holds no EVE login: it hands each change to a mapper page open in the app
//! (an event the page listens for), and the page reports it to the mapper with your own
//! session. With no mapper page open, the latest systems are kept and sent as soon as one is.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::chatlog::{self, ChatEvent, Watcher};
use crate::settings;

/// Latest system per character id (what the logs say right now).
static LATEST: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);
/// Character ids by name (from the log file names), for the clipboard watcher.
static IDS: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
/// What Settings shows: the folder, and how many characters the logs cover.
static STATUS: Mutex<Option<(Option<String>, usize)>> = Mutex::new(None);
/// Patch 54: the last "who writes chat logs" report sent to the mapper (JSON), resent when a
/// mapper page opens.
static CHAT_LOG_REPORT: Mutex<Option<String>> = Mutex::new(None);

/// An EVE window must be open this long before a missing chat log counts as "Log Chat to File
/// off" (EVE writes the Local log a few seconds after you log in).
const SETTLE: Duration = Duration::from_secs(60);

/// A character's id from its name, when its chat logs were seen (case doesn't matter).
pub fn character_id(name: &str) -> Option<i64> {
    let ids = IDS.lock().unwrap();
    let ids = ids.as_ref()?;
    ids.get(name).copied().or_else(|| {
        ids.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, id)| *id)
    })
}

pub fn status() -> (Option<String>, usize) {
    STATUS.lock().unwrap().clone().unwrap_or((None, 0))
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut watcher: Option<Watcher> = None;
        // Patch 54: when each open EVE window was first seen, and when we last looked.
        let mut windows_seen: HashMap<String, Instant> = HashMap::new();
        let mut windows_checked = Instant::now() - Duration::from_secs(60);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            // Off in Settings, or switched off for everyone by the directors.
            let wanted = settings::load(&app).chat_log_location()
                && crate::appconfig::feature_on("chat_log_location");
            if !wanted {
                watcher = None;
                *STATUS.lock().unwrap() = Some((None, 0));
                continue;
            }
            if watcher.is_none() {
                match chatlog::default_dir() {
                    Some(dir) => {
                        log::info!("following EVE chat logs in {}", dir.display());
                        watcher = Some(Watcher::new(dir));
                    }
                    None => {
                        *STATUS.lock().unwrap() = Some((None, 0));
                        // Look again in a while (EVE may not have written logs yet).
                        std::thread::sleep(Duration::from_secs(30));
                        continue;
                    }
                }
            }
            let Some(w) = watcher.as_mut() else { continue };
            let events = match w.poll() {
                Ok(events) => events,
                Err(err) => {
                    log::warn!("couldn't read the chat logs: {err}");
                    watcher = None;
                    continue;
                }
            };

            for event in &events {
                if let ChatEvent::Jumped { character, to, .. } = event {
                    log::info!("{character} jumped to {to}");
                }
            }

            // Where everyone is now, by character id (from the log file names, newer
            // EVE clients). Send only what changed since the last look.
            let current: HashMap<i64, String> = w
                .locations()
                .filter_map(|(_, system, id)| id.map(|id| (id, system.to_string())))
                .collect();
            *STATUS.lock().unwrap() = Some((Some(w.dir().display().to_string()), current.len()));
            *IDS.lock().unwrap() = Some(
                w.locations()
                    .filter_map(|(name, _, id)| id.map(|id| (name.to_string(), id)))
                    .collect(),
            );

            let changed: Vec<(i64, String)> = {
                let mut latest = LATEST.lock().unwrap();
                let latest = latest.get_or_insert_with(HashMap::new);
                let changed: Vec<(i64, String)> = current
                    .iter()
                    .filter(|(id, system)| latest.get(id) != Some(system))
                    .map(|(id, system)| (*id, system.clone()))
                    .collect();
                for (id, system) in &changed {
                    latest.insert(*id, system.clone());
                }
                changed
            };
            if !changed.is_empty() {
                deliver(&app, &changed);
            }

            if windows_checked.elapsed() >= Duration::from_secs(15) {
                windows_checked = Instant::now();
                report_chat_logs(&app, w, &mut windows_seen);
            }
        }
    });
}

/// Patch 54: for each EVE client open on this PC, whether its character writes chat logs
/// (EVE's "Log Chat to File"). Sent to the mapper when it changes; Your accounts warns about
/// the ones that don't.
fn report_chat_logs(app: &AppHandle, w: &Watcher, seen: &mut HashMap<String, Instant>) {
    let open = crate::clipboard::open_eve_characters();
    seen.retain(|name, _| open.contains(name));
    let logging: Vec<String> = w.locations().map(|(name, _, _)| name.to_lowercase()).collect();
    let mut rows: Vec<(String, bool)> = Vec::new();
    for name in open {
        let first = *seen.entry(name.clone()).or_insert_with(Instant::now);
        let has_log = logging.contains(&name.to_lowercase());
        // A fresh login gets a minute for EVE to start its Local log.
        if has_log || first.elapsed() >= SETTLE {
            rows.push((name, has_log));
        }
    }
    if rows.is_empty() {
        return;
    }
    rows.sort();
    let json = serde_json::to_string(
        &rows
            .iter()
            .map(|(name, logging)| serde_json::json!({ "name": name, "logging": logging }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    {
        let mut last = CHAT_LOG_REPORT.lock().unwrap();
        if last.as_deref() == Some(json.as_str()) {
            return;
        }
        *last = Some(json.clone());
    }
    for (name, logging) in &rows {
        if !logging {
            log::info!("{name} has EVE's Log Chat to File off (no Local chat log)");
        }
    }
    send_chat_logs(app, &json);
}

fn send_chat_logs(app: &AppHandle, json: &str) {
    send_to_mapper(
        app,
        &format!("window.dispatchEvent(new CustomEvent('devils-rage-app:chat-logs', {{ detail: {{ characters: {json} }} }}));"),
    );
}

/// A mapper page just opened somewhere in the app: send it where everyone is.
pub fn resend_all(app: &AppHandle) {
    let all: Vec<(i64, String)> = LATEST
        .lock()
        .unwrap()
        .as_ref()
        .map(|m| m.iter().map(|(id, s)| (*id, s.clone())).collect())
        .unwrap_or_default();
    if !all.is_empty() {
        deliver(app, &all);
    }
    let report = CHAT_LOG_REPORT.lock().unwrap().clone();
    if let Some(json) = report {
        send_chat_logs(app, &json);
    }
}

/// Hands the changes to one mapper page in the app.
fn deliver(app: &AppHandle, changes: &[(i64, String)]) {
    let mut js = String::new();
    for (id, system) in changes {
        let system = serde_json::to_string(system).unwrap_or_else(|_| "\"\"".into());
        js.push_str(&format!(
            "window.dispatchEvent(new CustomEvent('devils-rage-app:location', {{ detail: {{ character_id: {id}, solarsystem: {system} }} }}));"
        ));
    }
    send_to_mapper(app, &js);
}

/// Runs `js` in one mapper page open in the app: a map page first (the main window's, then
/// any other), else any mapper page. Never in a scanner pop-out (it's drawn by its parent).
pub fn send_to_mapper(app: &AppHandle, js: &str) {
    let mapper_host = crate::host_of(crate::MAPPER_URL);
    let mut windows: Vec<_> = app
        .webviews()
        .into_iter()
        .filter(|(label, _)| !label.starts_with("scanner-") && label != "bar")
        .filter_map(|(label, w)| {
            let url = w.url().ok()?;
            (url.host_str() == Some(mapper_host)).then(|| (label, url.path().starts_with("/maps/"), w))
        })
        .collect();
    // Map pages first, the main window first among equals.
    windows.sort_by_key(|(label, on_map, _)| (!*on_map, label != "main"));
    if let Some((_, _, window)) = windows.into_iter().next() {
        let _ = window.eval(js.to_string());
    }
}
