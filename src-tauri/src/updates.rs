//! Self-update. Checks a few seconds after start, then every few hours, and
//! when someone picks "Check for updates" (menu or start screen).
//! Each channel has its own manifest on the repo's `updates` branch, written by
//! the release workflows. Builds are signed; the updater refuses anything not
//! signed with our key (public half in tauri.conf.json).

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use crate::settings;

const MANIFEST_BASE: &str = "https://raw.githubusercontent.com/tsawind/devils-rage-app/updates";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// When the last check ran and what it found (shown in Settings).
static LAST: Mutex<Option<(SystemTime, String)>> = Mutex::new(None);
static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn record(outcome: impl Into<String>) {
    let outcome = outcome.into();
    log::info!("update check: {outcome}");
    *LAST.lock().unwrap() = Some((SystemTime::now(), outcome));
}

/// (seconds ago, what it found), or None before the first check.
pub fn last_check() -> Option<(u64, String)> {
    LAST.lock().unwrap().as_ref().map(|(when, text)| {
        let ago = when.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        (ago, text.clone())
    })
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if let Err(err) = check(&app, false).await {
                // Offline, GitHub down, etc. Try again next round.
                // (Phase 0.3 sends this to the log file / developer panel.)
                record(format!("Couldn't check: {err}"));
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// "Check for updates" clicked: same check, but always says what it found.
pub fn check_now(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = check(&app, true).await {
            record(format!("Couldn't check: {err}"));
            tell(
                &app,
                "Couldn't check for updates",
                format!("{err}\n\nCheck your internet connection and try again."),
                MessageDialogKind::Warning,
            );
        }
    });
}

async fn check(app: &AppHandle, manual: bool) -> Result<(), String> {
    let channel = settings::load(app).channel(app);
    // 22 Nov: Devil's Peepshow has its own channel files (peepshow-dev.json, …).
    let program = if crate::clienttools::on() { "peepshow-" } else { "" };
    let manifest: Url = format!("{MANIFEST_BASE}/{program}{}.json", channel.as_str())
        .parse()
        .map_err(|e| format!("bad manifest url: {e}"))?;

    let updater = app
        .updater_builder()
        .endpoints(vec![manifest])
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        record("You have the newest version");
        if manual {
            tell(
                app,
                "No update",
                format!(
                    "You have the newest version: {} ({} channel).",
                    app.package_info().version,
                    channel.as_str()
                ),
                MessageDialogKind::Info,
            );
        }
        return Ok(());
    };

    record(format!("Version {} is ready", update.version));
    let question = format!(
        "{} {} is ready ({} channel). You have {}.\n\nInstall it and restart now?",
        if crate::clienttools::on() { "Devil's Peepshow" } else { "Devil's Rage" },
        update.version,
        channel.as_str(),
        update.current_version
    );
    if !ask(app, question).await {
        return Ok(()); // "Later": ask again at the next check
    }

    // One install at a time (the automatic check and the button could both ask).
    if INSTALLING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        log::info!("an update is already being installed");
        return Ok(());
    }
    log::info!("installing {}", update.version);
    // Patch 74: say how the download goes, and give up after a while instead of hanging
    // (Tsa, 18 Nov: dev.221 never finished downloading and nothing said so).
    let got = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let last_said = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (got2, said2) = (got.clone(), last_said.clone());
    let download = update.download_and_install(
        move |chunk, total| {
            let now = got2.fetch_add(chunk as u64, std::sync::atomic::Ordering::Relaxed) + chunk as u64;
            if now - said2.load(std::sync::atomic::Ordering::Relaxed) >= 1_000_000 || Some(now) == total {
                said2.store(now, std::sync::atomic::Ordering::Relaxed);
                log::info!("update download: {} of {} KB", now / 1024, total.map_or("?".to_string(), |t| (t / 1024).to_string()));
            }
        },
        || log::info!("update downloaded: starting the installer"),
    );
    let result = tokio::time::timeout(Duration::from_secs(180), download).await;
    INSTALLING.store(false, std::sync::atomic::Ordering::SeqCst);
    match result {
        Err(_) => {
            let got = got.load(std::sync::atomic::Ordering::Relaxed) / 1024;
            log::warn!("update download stalled after {got} KB");
            let text = format!("The download stopped after {got} KB. Try again in a minute, or get the installer from the releases page.");
            tell(app, "Update didn't download", text.clone(), MessageDialogKind::Warning);
            return Err(text);
        }
        Ok(Err(e)) => {
            log::warn!("update install failed: {e}");
            tell(app, "Update didn't install", format!("{e}\n\nTry again, or get the installer from the releases page."), MessageDialogKind::Warning);
            return Err(e.to_string());
        }
        Ok(Ok(())) => {}
    }
    app.restart();
}

async fn ask(app: &AppHandle, question: String) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(question)
        .title("Update available")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and restart".into(),
            "Later".into(),
        ))
        .show(move |yes| {
            let _ = tx.send(yes);
        });
    rx.await.unwrap_or(false)
}

fn tell(app: &AppHandle, title: &str, text: String, kind: MessageDialogKind) {
    app.dialog()
        .message(text)
        .title(title)
        .kind(kind)
        .show(|_| {});
}
