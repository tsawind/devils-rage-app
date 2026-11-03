//! PI video scan: the scan page (ui/piscan.html) watches one EVE client's planet view while the
//! pilot rotates the planet, and paints the resource colours onto a whole-planet map. The
//! picture comes from capture.rs (the same picture Windows shows; nothing touches the game).
//! These commands talk to SeAT with the app's own sign-in: the pilot's planets with every
//! building's position (the anchors), and the corp's scans to read and add to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Encode one path segment (resource names have spaces).
fn segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

async fn get_json(app: &AppHandle, path: &str) -> Result<serde_json::Value, String> {
    let (cookies, _) = crate::launch::seat_session(app)?;
    let response = client()
        .get(format!("{}/devilsrage/{path}", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(serde_json::Value::Null);
    }
    if !status.is_success() {
        return Err(format!("SeAT answered {status} (sign in to SeAT in the app again?)"));
    }
    response.json().await.map_err(|_| "SeAT sent something else (signed out? open the PI tab once)".to_string())
}

/// Your planets, their buildings' positions, and the scans so far.
#[tauri::command]
pub async fn piscan_planets(app: AppHandle) -> Result<serde_json::Value, String> {
    get_json(&app, "pi/scan-planets").await
}

/// The corp's scan of one planet and resource so far (null when none).
#[tauri::command]
pub async fn piscan_cells(app: AppHandle, planet_id: u64, resource: String) -> Result<serde_json::Value, String> {
    get_json(&app, &format!("pi/scans/{planet_id}/{}", segment(&resource))).await
}

/// Hand a scan to SeAT (it merges into what the corp has).
#[tauri::command]
pub async fn piscan_save(app: AppHandle, planet_id: u64, resource: String, character_id: Option<u64>, cells: String) -> Result<String, String> {
    let (cookies, xsrf) = crate::launch::seat_session(&app)?;
    let xsrf = xsrf.ok_or("SeAT's session is missing: open the PI tab once, then try again")?;
    let response = client()
        .post(format!("{}/devilsrage/pi/scans", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&serde_json::json!({ "planet_id": planet_id, "resource": resource, "character_id": character_id, "cells": cells }))
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

/// The client's picture for the scan, raw: 8 bytes (width, height as little-endian u32) then
/// RGBA pixels. Skips the JPEG encode/decode the previews use, so the scan sees more pictures a
/// second (smaller turns between them track more surely). Nothing leaves the PC.
#[tauri::command]
pub async fn piscan_frame(name: String, width: Option<u32>) -> Result<tauri::ipc::Response, String> {
    let w = width.unwrap_or(1280).clamp(320, 1600);
    tauri::async_runtime::spawn_blocking(move || {
        let (width, height, rgba) = crate::capture::client_rgba(&name, w)?;
        Ok(tauri::ipc::Response::new(frame_bytes(width, height, &rgba)))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn frame_bytes(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + rgba.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(rgba);
    out
}

/// Delete the corp's scan of one planet and resource (yours, or any for a leader).
#[tauri::command]
pub async fn piscan_delete(app: AppHandle, planet_id: u64, resource: String) -> Result<(), String> {
    let (cookies, xsrf) = crate::launch::seat_session(&app)?;
    let xsrf = xsrf.ok_or("SeAT's session is missing: open the PI tab once, then try again")?;
    let response = client()
        .post(format!("{}/devilsrage/pi/scans/{planet_id}/{}/delete", crate::SEAT_URL, segment(&resource)))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!("SeAT answered {status}"))
    }
}

// ---- Recording (for tuning the scan on real pictures) ----------------------------------------
// The scan page's Record button: the client's picture 3 times a second, saved as JPEGs with a
// notes file (what the page had selected), next to the app's logs. Nothing leaves the PC.

static RECORDING: AtomicBool = AtomicBool::new(false);
static NOTES: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());

fn recordings_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_local_data_dir().map(|d| d.join("recordings")).map_err(|e| e.to_string())
}

/// Start recording a client (stops by itself after `seconds`, at most 3 minutes).
#[tauri::command]
pub fn piscan_record_start(app: AppHandle, name: String, seconds: Option<u64>) -> Result<String, String> {
    if RECORDING.swap(true, Ordering::SeqCst) {
        return Err("Already recording.".into());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dir = recordings_dir(&app)?.join(format!("scan-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    NOTES.lock().unwrap().clear();
    let limit = seconds.unwrap_or(120).clamp(5, 180);
    let out = dir.clone();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut n = 0u32;
        while RECORDING.load(Ordering::SeqCst) && started.elapsed().as_secs() < limit {
            let t0 = std::time::Instant::now();
            if let Ok(jpeg) = crate::capture::client_jpeg(&name, 1280, 85) {
                n += 1;
                let _ = std::fs::write(out.join(format!("frame-{n:04}-{}ms.jpg", started.elapsed().as_millis())), jpeg);
            }
            let spent = t0.elapsed();
            if spent < std::time::Duration::from_millis(333) {
                std::thread::sleep(std::time::Duration::from_millis(333) - spent);
            }
        }
        RECORDING.store(false, Ordering::SeqCst);
        let notes = std::mem::take(&mut *NOTES.lock().unwrap());
        let _ = std::fs::write(out.join("notes.json"), serde_json::to_string_pretty(&serde_json::json!({ "client": name, "frames": n, "notes": notes })).unwrap_or_default());
        log::info!("scan recording: {n} frames in {}", out.display());
    });
    Ok(dir.display().to_string())
}

/// Stop recording.
#[tauri::command]
pub fn piscan_record_stop() {
    RECORDING.store(false, Ordering::SeqCst);
}

#[tauri::command]
pub fn piscan_recording() -> bool {
    RECORDING.load(Ordering::SeqCst)
}

/// What the page has selected (planet, resource, edge, lock), kept with the recording.
#[tauri::command]
pub fn piscan_record_note(note: serde_json::Value) {
    if RECORDING.load(Ordering::SeqCst) {
        let mut notes = NOTES.lock().unwrap();
        if notes.len() < 2000 {
            notes.push(note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{frame_bytes, segment};

    #[test]
    fn frame_has_size_then_pixels() {
        let out = frame_bytes(2, 1, &[1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(&out[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn segments_encode_spaces() {
        assert_eq!(segment("Base Metals"), "Base%20Metals");
        assert_eq!(segment("Micro-Organisms"), "Micro-Organisms");
    }
}
