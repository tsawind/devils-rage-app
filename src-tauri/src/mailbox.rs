//! 22 Nov (Devil's Peepshow split): the two programs ask each other for things on this PC only.
//!
//! A message is a small JSON file dropped in the other program's `mailbox` folder (in its
//! local data folder); each program looks in its own every 300 ms, does what's asked and
//! deletes the file. Nothing goes over the network, and a message nobody picked up within
//! 30 s (the other program wasn't running) is dropped rather than done late.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tauri::AppHandle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum To {
    App,
    Peepshow,
}

fn dir(to: To) -> Option<PathBuf> {
    match to {
        To::App => crate::shared::app_local("mailbox"),
        To::Peepshow => crate::shared::peepshow_local("mailbox"),
    }
}

/// Whether the other program is running right now (its one-copy lock is held).
pub fn running(to: To) -> bool {
    crate::clienttools::held(match to {
        To::App => crate::clienttools::APP_LOCK,
        To::Peepshow => crate::clienttools::PEEPSHOW_LOCK,
    })
}

/// Drop a message for the other program. False when it couldn't be written.
pub fn send(to: To, message: Value) -> bool {
    let Some(dir) = dir(to) else { return false };
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!("{stamp}-{}.tmp", std::process::id()));
    let done = dir.join(format!("{stamp}-{}.json", std::process::id()));
    let ok = std::fs::write(&tmp, message.to_string()).is_ok() && std::fs::rename(&tmp, &done).is_ok();
    log::info!("message to {to:?}: {} ({})", message.get("do").and_then(Value::as_str).unwrap_or("?"), if ok { "sent" } else { "couldn't write it" });
    ok
}

/// Look in this program's own mailbox every 300 ms and hand each message to `handle`
/// (on this thread: `handle` uses `mainthread::run` for any window work it does).
pub fn start(me: To, handle: fn(&AppHandle, Value)) {
    let Some(dir) = dir(me) else { return };
    let _ = std::fs::create_dir_all(&dir);
    // Leftovers from before this start are old news.
    clear_old(&dir, Duration::ZERO);
    let _ = std::thread::Builder::new().name("mailbox".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for file in files {
            let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(30));
            let text = std::fs::read_to_string(&file).ok();
            let _ = std::fs::remove_file(&file);
            let Some(message) = text.and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            if !fresh {
                log::info!("old message dropped: {}", message.get("do").and_then(Value::as_str).unwrap_or("?"));
                continue;
            }
            if let Some(app) = crate::mainthread::app() {
                handle(app, message);
            }
        }
        clear_old(&dir, Duration::from_secs(60));
    });
}

/// Remove half-written or stale files older than `age`.
fn clear_old(dir: &std::path::Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|a| a >= age);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
