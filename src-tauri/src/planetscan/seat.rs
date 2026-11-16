//! The planet scan's talks with SeAT (the app's own sign-in): the corp's scans and each planet's
//! shared sky and bare map. Maps go up in pieces (a sky is a few MB).

use tauri::AppHandle;

fn client() -> reqwest::Client {
    crate::http_client()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut map = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        map[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in text.as_bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = map[c as usize];
        if v == 255 {
            return None;
        }
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

async fn post_json(app: &AppHandle, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let (cookies, xsrf) = crate::launch::seat_write_session(app).await?;
    let response = client()
        .post(format!("{}/devilsrage/{path}", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .header("X-XSRF-TOKEN", xsrf)
        .header("X-Requested-With", "XMLHttpRequest")
        .json(body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(body["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("SeAT answered {status}")));
    }
    Ok(body)
}

/// One planet's shared map from SeAT (None when nobody has saved one).
pub async fn get_map(app: &AppHandle, planet: u64, kind: &str) -> Result<Option<Vec<u8>>, String> {
    let (cookies, _) = crate::launch::seat_session(app)?;
    let response = client()
        .get(format!("{}/devilsrage/pi/maps/{planet}/{kind}", crate::SEAT_URL))
        .header("Accept", "application/json")
        .header("Cookie", cookies)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("SeAT answered {} for the {kind} map", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|_| "SeAT sent something else (signed out?)".to_string())?;
    Ok(body["data"].as_str().and_then(b64_decode))
}

/// Hand one planet's sky or bare map to SeAT, in pieces.
pub async fn save_map(app: &AppHandle, planet: u64, kind: &str, bytes: &[u8], coverage: f64, meta: &str, character_id: Option<u64>) -> Result<String, String> {
    const PIECE: usize = 600_000;
    let text = b64_encode(bytes);
    let pieces: Vec<&str> = text.as_bytes().chunks(PIECE).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    let upload = format!("{planet}-{kind}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let mut message = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        let body = serde_json::json!({
            "planet_id": planet, "kind": kind, "upload": upload, "part": i, "parts": pieces.len(), "data": piece,
            "coverage": (coverage.clamp(0.0, 1.0) * 1000.0).round() / 1000.0, "meta": meta, "character_id": character_id,
        });
        let answer = post_json(app, "pi/maps", &body).await?;
        message = answer["message"].as_str().unwrap_or("").to_string();
    }
    Ok(message)
}

pub async fn delete_map(app: &AppHandle, planet: u64, kind: &str) -> Result<(), String> {
    post_json(app, &format!("pi/maps/{planet}/{kind}/delete"), &serde_json::json!({})).await.map(|_| ())
}

/// Hand a resource scan to SeAT (it merges into what the corp has, and keeps this one dated).
pub async fn save_scan(app: &AppHandle, planet: u64, resource: &str, character_id: Option<u64>, cells: &[u8]) -> Result<String, String> {
    let body = serde_json::json!({ "planet_id": planet, "resource": resource, "character_id": character_id, "cells": b64_encode(cells) });
    let answer = post_json(app, "pi/scans", &body).await?;
    Ok(answer["message"].as_str().unwrap_or("Saved.").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_both_ways() {
        for n in 0..20 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&bytes)).unwrap(), bytes);
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert!(b64_decode("@@@@").is_none());
    }
}
