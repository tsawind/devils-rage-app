//! Self-update. Checks a few seconds after start, then every few hours.
//! Each channel has its own manifest on the repo's `updates` branch, written by
//! the release workflows. Builds are signed; the updater refuses anything not
//! signed with our key (public half in tauri.conf.json).

use std::time::Duration;

use tauri::{AppHandle, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use crate::settings;

const MANIFEST_BASE: &str = "https://raw.githubusercontent.com/tsawind/devils-rage-app/updates";
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if let Err(err) = check(&app).await {
                // Offline, GitHub down, etc. Try again next round.
                // (Phase 0.3 sends this to the log file / developer panel.)
                eprintln!("update check failed: {err}");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

async fn check(app: &AppHandle) -> Result<(), String> {
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
        return Ok(()); // already up to date
    };

    let question = format!(
        "Devil's Rage {} is ready ({} channel). You have {}.\n\nInstall it and restart now?",
        update.version,
        channel.as_str(),
        update.current_version
    );
    if !ask(app, question).await {
        return Ok(()); // "Later": ask again at the next check
    }

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
