//! Patch 66: SeAT's Your accounts page hands the app your accounts and launch groups.
//!
//! SeAT is where launch groups are made (tied to your accounts and characters). When its Your
//! accounts page runs inside the app it posts them here, over the same local address the camera
//! pages use (http://drcap.localhost), and only that site may: the request's Origin must be our
//! SeAT. The app keeps a copy (seat-groups.json) for its Launch groups page, pedal cycling and
//! Client settings. The page can also take the groups made on this PC (e.g. imported from EVE-O
//! Preview) to bring them into SeAT.

use tauri::http::{header, Request, Response, StatusCode};
use tauri::AppHandle;
use tauri::Url;

const MAX_BODY: usize = 2 * 1024 * 1024;

/// Whether a drcap request is one of ours (the rest is the camera capture).
pub fn wants(request: &Request<Vec<u8>>) -> bool {
    let path = crate::capture::path_and_query(&request.uri().to_string()).0;
    path.starts_with("/launch-") || path == "/pc-accounts" || path == "/incognito-hide"
}

fn allowed(origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(|host| host == crate::host_of(crate::SEAT_URL)))
        .unwrap_or(false)
}

fn reply(status: StatusCode, body: Vec<u8>, origin: &str) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store");
    if !origin.is_empty() {
        response = response
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, DELETE, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type")
            .header("Access-Control-Allow-Private-Network", "true")
            .header(header::VARY, "Origin");
    }
    response.body(body).unwrap_or_else(|_| Response::new(Vec::new()))
}

pub fn handle(app: &AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let origin = request.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !allowed(&origin) {
        log::warn!("launch sync from {origin:?} refused (only our SeAT may)");
        return reply(StatusCode::FORBIDDEN, b"{\"error\":\"not for this page\"}".to_vec(), "");
    }
    let method = request.method().clone();
    if method == tauri::http::Method::OPTIONS {
        return reply(StatusCode::NO_CONTENT, Vec::new(), &origin);
    }
    let (path, _) = crate::capture::path_and_query(&request.uri().to_string());
    log::info!("SeAT page → app: {} {path} ({} bytes)", method.as_str(), request.body().len());
    match (method.as_str(), path.as_str()) {
        ("POST", "/launch-sync") => {
            let body = request.body();
            if body.len() > MAX_BODY {
                return reply(StatusCode::PAYLOAD_TOO_LARGE, b"{}".to_vec(), &origin);
            }
            match serde_json::from_slice::<crate::launch::SeatData>(body) {
                Ok(data) => match crate::launch::store_seat(app, &data) {
                    Ok(()) => reply(StatusCode::OK, b"{\"saved\":true}".to_vec(), &origin),
                    Err(e) => reply(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e }).to_string().into_bytes(), &origin),
                },
                Err(e) => reply(StatusCode::BAD_REQUEST, serde_json::json!({ "error": e.to_string() }).to_string().into_bytes(), &origin),
            }
        }
        ("GET", "/launch-local") => {
            let groups = crate::launch::local_for_import(app);
            reply(StatusCode::OK, serde_json::to_vec(&groups).unwrap_or_default(), &origin)
        }
        // POST (a "simple" request, no CORS preflight) or DELETE: SeAT took the PC's groups.
        ("POST", "/launch-local-done") | ("DELETE", "/launch-local") => match crate::launch::retire_local(app) {
            Ok(()) => reply(StatusCode::OK, b"{\"done\":true}".to_vec(), &origin),
            Err(e) => reply(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e }).to_string().into_bytes(), &origin),
        },
        // Patch 74: which characters the EVE launcher loaded for each EVE account on this PC, and
        // which of them are incognito, so Your accounts can show them where they belong (greyed).
        // Only for our SeAT's page (checked above); the page shows them and never sends the
        // incognito ones on.
        ("GET", "/pc-accounts") => {
            let accounts: Vec<serde_json::Value> = crate::launch::launcher_accounts()
                .into_iter()
                .map(|(user, ids)| {
                    let characters: Vec<serde_json::Value> = ids.into_iter().map(|id| serde_json::json!({ "id": id, "incognito": !crate::incognito::is_public_id(id) })).collect();
                    serde_json::json!({ "user_id": user, "characters": characters })
                })
                .collect();
            let hidden = crate::incognito::incognito_state()["hidden"].clone();
            reply(StatusCode::OK, serde_json::json!({ "accounts": accounts, "hidden": hidden }).to_string().into_bytes(), &origin)
        }
        // Patch 74: hide a character known to SeAT from the mapper / SeAT (or share it again).
        ("POST", "/incognito-hide") => {
            #[derive(serde::Deserialize)]
            struct Hide {
                name: String,
                hidden: bool,
            }
            match serde_json::from_slice::<Hide>(request.body()) {
                Ok(h) if !h.name.trim().is_empty() && h.name.len() <= 40 => {
                    crate::incognito::incognito_hide(app.clone(), h.name, h.hidden);
                    reply(StatusCode::OK, b"{\"saved\":true}".to_vec(), &origin)
                }
                _ => reply(StatusCode::BAD_REQUEST, b"{}".to_vec(), &origin),
            }
        }
        _ => reply(StatusCode::NOT_FOUND, b"{}".to_vec(), &origin),
    }
}
