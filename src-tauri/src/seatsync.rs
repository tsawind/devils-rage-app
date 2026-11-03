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
    let uri = request.uri().to_string();
    let path = Url::parse(&uri).map(|u| u.path().to_string()).unwrap_or(uri);
    path.starts_with("/launch-")
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
        return reply(StatusCode::FORBIDDEN, b"{\"error\":\"not for this page\"}".to_vec(), "");
    }
    let method = request.method().clone();
    if method == tauri::http::Method::OPTIONS {
        return reply(StatusCode::NO_CONTENT, Vec::new(), &origin);
    }
    let uri = request.uri().to_string();
    let path = Url::parse(&uri).map(|u| u.path().to_string()).unwrap_or(uri);
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
        _ => reply(StatusCode::NOT_FOUND, b"{}".to_vec(), &origin),
    }
}
