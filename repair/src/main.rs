//! Devil's Rage Repair (Tsa, 19 Nov): a small program next to the app (and in the Start menu)
//! for when the app won't start, won't update, or got itself in a twist. It never needs the
//! app itself to run:
//!   1. Repair: close the app and install the newest dev build over it (settings kept)
//!   2. Install the newest stable build (settings kept)
//!   3. Clean install: put the app's settings aside (moved to a backup folder, not deleted),
//!      then install the newest dev build
//!   4. Collect the logs for a bug report (a zip on the Desktop, sign-in tokens masked)
//!   5. Close stuck Devil's Rage programs
//! It copies itself to a temporary folder first, so the installer can replace the copy next to
//! the app. Downloads come from the app's own GitHub releases, over HTTPS.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFESTS: &str = "https://raw.githubusercontent.com/tsawind/devils-rage-app/updates";
/// The app's folders (Tauri's: Roaming for settings, Local for logs, recordings and the browser data).
const APP_ID: &str = "com.tsawind.devilsrage";
/// The app's program names (the installer's and the build's), and Client Tools (20 Nov).
const APP_EXES: [&str; 3] = ["Devils Rage.exe", "devils-rage-app.exe", "Devils Rage Client Tools.exe"];

fn main() {
    if let Some(code) = run_from_temp() {
        std::process::exit(code);
    }
    println!("Devil's Rage Repair\n===================\n");
    println!("Nothing here signs in to EVE or sends your settings anywhere.\n");
    loop {
        println!("  1  Repair: install the newest dev build over the app (keeps your settings)");
        println!("  2  Install the newest stable build (keeps your settings)");
        println!("  3  Clean install: put the app's settings aside (backed up), then install the newest dev build");
        println!("  4  Collect logs for a bug report (zip on your Desktop, sign-in tokens masked)");
        println!("  5  Close stuck Devil's Rage programs");
        println!("  Q  Quit\n");
        let choice = ask("Pick one: ");
        let result = match choice.trim().to_ascii_lowercase().as_str() {
            "1" => install("dev", false),
            "2" => install("stable", false),
            "3" => clean_install(),
            "4" => collect_logs(),
            "5" => close_app().map(|n| format!("Closed {n} program(s).")),
            "q" | "" => return,
            _ => continue,
        };
        match result {
            Ok(said) => println!("\n{said}\n"),
            Err(e) => println!("\nThat didn't work: {e}\n"),
        }
    }
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

/// Run a copy from the temp folder (so the installer can overwrite the one next to the app).
/// Some(exit code) when this was the original and the copy ran.
fn run_from_temp() -> Option<i32> {
    if !cfg!(windows) {
        return None;
    }
    let me = std::env::current_exe().ok()?;
    let temp = std::env::temp_dir().join("devils-rage-repair");
    if me.starts_with(&temp) {
        return None;
    }
    std::fs::create_dir_all(&temp).ok()?;
    let copy = temp.join("Devils Rage Repair.exe");
    std::fs::copy(&me, &copy).ok()?;
    let status = Command::new(&copy).env("DEVILS_RAGE_APP_DIR", me.parent()?).status().ok()?;
    Some(status.code().unwrap_or(0))
}

/// Where the app is installed (the folder this program came from).
fn app_dir() -> Option<PathBuf> {
    std::env::var_os("DEVILS_RAGE_APP_DIR").map(PathBuf::from).or_else(|| std::env::current_exe().ok()?.parent().map(Path::to_path_buf))
}

/// Close every Devil's Rage program (not this one). How many were running.
fn close_app() -> Result<usize, String> {
    let mut closed = 0;
    for exe in APP_EXES {
        let out = Command::new("taskkill").args(["/F", "/T", "/IM", exe]).output().map_err(|e| format!("couldn't run taskkill: {e}"))?;
        if out.status.success() {
            closed += String::from_utf8_lossy(&out.stdout).matches("SUCCESS").count().max(1);
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(closed)
}

/// The installer for a channel (dev / beta / stable): its version and download address.
fn newest(channel: &str) -> Result<(String, String), String> {
    let text = download_text(&format!("{MANIFESTS}/{channel}.json"))?;
    installer_in(&text).ok_or_else(|| format!("no Windows installer in the {channel} channel yet"))
}

/// The version and Windows installer address in an update manifest.
fn installer_in(manifest: &str) -> Option<(String, String)> {
    let json: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let version = json["version"].as_str()?.to_string();
    let platforms = &json["platforms"];
    let url = ["windows-x86_64-nsis", "windows-x86_64"].iter().find_map(|k| platforms[*k]["url"].as_str())?;
    (url.starts_with("https://github.com/tsawind/devils-rage-app/releases/download/") && url.ends_with(".exe")).then(|| (version, url.to_string()))
}

fn download_text(url: &str) -> Result<String, String> {
    let out = Command::new("curl.exe").args(["-fsSL", "--max-time", "30", url]).output().map_err(|e| format!("couldn't run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("couldn't download {url} (is the internet up?)"));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

fn download_file(url: &str, to: &Path) -> Result<(), String> {
    println!("Downloading {url}");
    let status = Command::new("curl.exe").args(["-fL", "--retry", "3", "--max-time", "600", "-o"]).arg(to).arg(url).status().map_err(|e| format!("couldn't run curl: {e}"))?;
    if !status.success() || std::fs::metadata(to).map(|m| m.len() < 1_000_000).unwrap_or(true) {
        return Err("the download failed or came back too small".into());
    }
    Ok(())
}

/// Close the app, download a channel's newest installer and run it.
fn install(channel: &str, quiet_check: bool) -> Result<String, String> {
    let (version, url) = newest(channel)?;
    if !quiet_check && ask(&format!("Install Devil's Rage {version} ({channel})? The app closes first. [y/N] ")).to_ascii_lowercase() != "y" {
        return Ok("Nothing changed.".into());
    }
    let setup = std::env::temp_dir().join("devils-rage-repair").join(format!("Devils Rage {version} setup.exe"));
    let _ = std::fs::create_dir_all(setup.parent().unwrap_or(Path::new(".")));
    download_file(&url, &setup)?;
    let closed = close_app()?;
    println!("Closed {closed} program(s). Starting the installer: keep the folder it suggests.");
    let status = Command::new(&setup).status().map_err(|e| format!("couldn't start the installer: {e}"))?;
    if !status.success() {
        return Err(format!("the installer stopped (code {:?})", status.code()));
    }
    let _ = std::fs::remove_file(&setup);
    Ok(format!("Devil's Rage {version} is installed. Start it from the Start menu."))
}

/// Move the app's settings folders aside (never deleted), then install the newest dev build.
fn clean_install() -> Result<String, String> {
    println!("This signs you out of SeAT and the mapper in the app and resets its settings, client spots and layouts.");
    println!("Nothing is deleted: the old folders move to a backup folder you can copy back.");
    if ask("Type CLEAN to go ahead: ") != "CLEAN" {
        return Ok("Nothing changed.".into());
    }
    close_app()?;
    let backup = backup_root().ok_or("couldn't find your Documents folder")?;
    let stamp = stamp();
    let mut moved = Vec::new();
    for (name, base) in [("Roaming", env_dir("APPDATA")), ("Local", env_dir("LOCALAPPDATA"))] {
        let Some(base) = base else { continue };
        let from = base.join(APP_ID);
        if !from.is_dir() {
            continue;
        }
        let to = backup.join(format!("{stamp} {name}"));
        std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
        move_dir(&from, &to)?;
        moved.push(to.display().to_string());
    }
    let said = install("dev", true)?;
    Ok(format!("Settings put aside in:\n  {}\n{said}", if moved.is_empty() { "(there were none)".into() } else { moved.join("\n  ") }))
}

fn backup_root() -> Option<PathBuf> {
    env_dir("USERPROFILE").map(|h| h.join("Documents").join("Devil's Rage").join("Backups"))
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// "2026-11-19 0812" (local time, from Windows' own clock tool; else seconds).
fn stamp() -> String {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-Date -Format 'yyyy-MM-dd HHmm'"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_default())
}

/// Rename, or copy then remove (another drive).
fn move_dir(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_dir(from, to).map_err(|e| format!("couldn't back up {}: {e}", from.display()))?;
    std::fs::remove_dir_all(from).map_err(|e| format!("backed up, but couldn't clear {}: {e}", from.display()))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Mask anything that looks like a sign-in token (32+ token characters in a row).
fn mask(text: &str) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || "_+/=-".contains(c);
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 32 {
            out.push_str("[masked]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if token(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// The app's logs (masked) and its install folder's listing, zipped onto the Desktop.
fn collect_logs() -> Result<String, String> {
    let logs = env_dir("LOCALAPPDATA").map(|d| d.join(APP_ID).join("logs")).ok_or("no LOCALAPPDATA")?;
    let work = std::env::temp_dir().join("devils-rage-repair").join("logs");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&logs) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read(entry.path()) {
                let _ = std::fs::write(work.join(entry.file_name()), mask(&String::from_utf8_lossy(&text)));
                count += 1;
            }
        }
    }
    let mut about = format!("Devil's Rage Repair report\napp folder: {}\n", app_dir().map(|d| d.display().to_string()).unwrap_or_default());
    if let Some(dir) = app_dir() {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            about.push_str(&format!("  {} ({size} bytes)\n", entry.file_name().to_string_lossy()));
        }
    }
    let _ = std::fs::write(work.join("about.txt"), about);
    let desktop = env_dir("USERPROFILE").map(|h| h.join("Desktop")).ok_or("no Desktop")?;
    let zip = desktop.join(format!("Devils Rage logs {}.zip", stamp()));
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", "Compress-Archive -Force -Path $env:DR_FROM -DestinationPath $env:DR_TO"])
        .env("DR_FROM", work.join("*"))
        .env("DR_TO", &zip)
        .status()
        .map_err(|e| format!("couldn't run PowerShell: {e}"))?;
    if !status.success() {
        return Err("couldn't make the zip".into());
    }
    Ok(format!("{count} log file(s) zipped to {}: send that in Discord.", zip.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_masked_and_words_kept() {
        let line = "ssoToken: eyJhbGciOiJSUzI1NiIsImtpZCI6IkpXVC1TaWduYXR1cmUtS2V5In0.abc done";
        assert_eq!(mask(line), "ssoToken: [masked].abc done");
        assert_eq!(mask("short words stay"), "short words stay");
    }

    #[test]
    fn only_our_own_releases_are_installed() {
        let ok = r#"{"version":"0.1.8-dev.224","platforms":{"windows-x86_64":{"url":"https://github.com/tsawind/devils-rage-app/releases/download/v0.1.8-dev.224/Devils.Rage_0.1.8-dev.224_x64-setup.exe"}}}"#;
        assert_eq!(installer_in(ok).map(|v| v.0), Some("0.1.8-dev.224".to_string()));
        let elsewhere = r#"{"version":"9","platforms":{"windows-x86_64":{"url":"https://example.com/x.exe"}}}"#;
        assert_eq!(installer_in(elsewhere), None);
        assert_eq!(installer_in("not json"), None);
    }
}
