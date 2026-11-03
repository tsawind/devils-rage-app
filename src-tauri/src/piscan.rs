//! PI video scan: the scan page (ui/piscan.html) watches one EVE client's planet view while the
//! pilot rotates the planet, and paints the resource colours onto a whole-planet map. The
//! picture comes from capture.rs (the same picture Windows shows; nothing touches the game).
//! These commands talk to SeAT with the app's own sign-in: the pilot's planets with every
//! building's position (the anchors), and the corp's scans to read and add to.

use tauri::AppHandle;

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

#[cfg(test)]
mod tests {
    use super::segment;

    #[test]
    fn segments_encode_spaces() {
        assert_eq!(segment("Base Metals"), "Base%20Metals");
        assert_eq!(segment("Micro-Organisms"), "Micro-Organisms");
    }
}
