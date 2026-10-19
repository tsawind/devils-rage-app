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
    let manifest: Url = format!("{MANIFEST_BASE}/{}.json", channel.as_str())
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
        "Devil's Rage {} is ready ({} channel). You have {}.\n\nInstall it and restart now?",
        update.version,
        channel.as_str(),
        update.current_version
    );
    if !ask(app, question).await {
        return Ok(()); // "Later": ask again at the next check
    }

    log::info!("installing {}", update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
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
