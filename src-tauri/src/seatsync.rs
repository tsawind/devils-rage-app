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
        ("GET", "/pc-accounts") => reply(StatusCode::OK, pc_accounts().to_string().into_bytes(), &origin),
        _ => reply(StatusCode::NOT_FOUND, b"{}".to_vec(), &origin),
    }
}

/// Patch 74: which characters the EVE launcher loaded for each EVE account on this PC, and which
/// of them are incognito, so Your accounts can show them where they belong (greyed). Only for our
/// SeAT's page; the page shows them and never sends the incognito ones on.
pub fn pc_accounts() -> serde_json::Value {
    // Only the EVE launcher's own lists (from CCP: this account has these characters). Profile
    // folders say which profile a character was played with, not whose it is (Tsa, 18 Nov:
    // Tsawind started once with settings_11 is just a stray file there).
    let accounts: Vec<serde_json::Value> = crate::launch::launcher_accounts()
        .into_iter()
        .map(|(user, ids)| {
            let characters: Vec<serde_json::Value> = ids.into_iter().map(|id| serde_json::json!({ "id": id, "incognito": !crate::incognito::is_public_id(id), "how": "launcher" })).collect();
            serde_json::json!({ "user_id": user, "characters": characters })
        })
        .collect();
    let state = crate::incognito::incognito_state();
    let (hidden, spies) = (state["hidden"].clone(), state["spies"].clone());
    // Logged-in clients and the EVE account each was signed in to (from its own start-up line):
    // certain, so Your accounts can flag a character sitting on another EVE account than its own.
    let running: Vec<serde_json::Value> = crate::clipboard::open_eve_windows()
        .into_iter()
        .filter_map(|(name, handle)| Some(serde_json::json!({ "name": name, "user_id": crate::startline::account_of_window(handle)? })))
        .collect();
    serde_json::json!({ "accounts": accounts, "hidden": hidden, "spies": spies, "running": running })
}

/// Character ids for names (EVE's public lookup), for characters seen logged in here.
async fn esi_ids(names: Vec<String>) -> std::collections::HashMap<String, u64> {
    if names.is_empty() {
        return Default::default();
    }
    let response = crate::http_client()
        .post("https://esi.evetech.net/latest/universe/ids/?datasource=tranquility")
        .json(&names)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;
    let Ok(response) = response else { return Default::default() };
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    body["characters"].as_array().into_iter().flatten().filter_map(|c| Some((c["name"].as_str()?.to_lowercase(), c["id"].as_u64()?))).collect()
}

/// Hand `pc_accounts` to a SeAT page in the app (its Your accounts asked), with each character's
/// name (public ESI lookup from this PC, remembered; the page's own lookup can fail), plus every
/// character seen logged in here on its EVE account (certain: from the client's sign-in token).
pub fn send_pc_accounts(app: &AppHandle, me: &str) {
    let (app, me) = (app.clone(), me.to_string());
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let mut detail = pc_accounts();
        // Seen logged in: add each to its EVE account (the launcher's lists may not have it).
        let seen = crate::pcaccounts::seen(&app);
        let ids = esi_ids(seen.keys().cloned().collect()).await;
        if let Some(accounts) = detail["accounts"].as_array_mut() {
            for (name, user) in &seen {
                let Some(id) = ids.get(&name.to_lowercase()).copied() else { continue };
                // Seen on this account now: off any other account the older lists put it on.
                for account in accounts.iter_mut() {
                    if account["user_id"].as_u64() != Some(*user) {
                        if let Some(list) = account["characters"].as_array_mut() {
                            list.retain(|c| c["id"].as_u64() != Some(id));
                        }
                    }
                }
                let entry = match accounts.iter_mut().position(|a| a["user_id"].as_u64() == Some(*user)) {
                    Some(i) => &mut accounts[i],
                    None => {
                        accounts.push(serde_json::json!({ "user_id": user, "characters": [] }));
                        accounts.last_mut().expect("just pushed")
                    }
                };
                if let Some(list) = entry["characters"].as_array_mut() {
                    if !list.iter().any(|c| c["id"].as_u64() == Some(id)) {
                        list.push(serde_json::json!({ "id": id, "incognito": !crate::incognito::is_public_id(id), "how": "client", "name": name }));
                    }
                }
            }
        }
        // Each EVE account's login name and tier, as its sign-in token said (this PC only).
        let info: serde_json::Map<String, serde_json::Value> = crate::pcaccounts::infos(&app)
            .into_iter()
            .map(|(user, i)| {
                // Its own profile folder (settings_<login>), when it's there.
                let own = i.login.as_ref().map(|l| l.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>()).filter(|n| !n.is_empty() && crate::profiles::root().is_ok_and(|r| r.join(format!("settings_{n}")).is_dir()));
                (user.to_string(), serde_json::json!({ "login": i.login, "tier": i.tier, "profile": i.profile, "own": own, "at": i.at }))
            })
            .collect();
        detail["info"] = serde_json::Value::Object(info);
        detail["seen"] = serde_json::Value::Array(seen.iter().map(|(name, user)| serde_json::json!({ "name": name, "user_id": user })).collect());
        // Every other character with settings on this PC that no account list has.
        let placed: std::collections::BTreeSet<u64> = detail["accounts"].as_array().into_iter().flatten().flat_map(|a| a["characters"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_u64())).collect();
        let others: Vec<serde_json::Value> = crate::pcaccounts::others(&app, &placed)
            .into_iter()
            .map(|(id, last, profiles)| serde_json::json!({ "id": id, "last": last, "profiles": profiles, "incognito": !crate::incognito::is_public_id(id) }))
            .collect();
        detail["others"] = serde_json::Value::Array(others);
        // Every character's last-played date and profile folders on this PC (shown on its row).
        let chars: serde_json::Map<String, serde_json::Value> = crate::pcaccounts::all_chars()
            .into_iter()
            .map(|(id, profiles)| (id.to_string(), serde_json::json!({ "last": profiles.first().map(|p| p.1).unwrap_or(0), "profiles": profiles.iter().map(|(p, t)| serde_json::json!({ "name": p, "at": t })).collect::<Vec<_>>() })))
            .collect();
        detail["chars"] = serde_json::Value::Object(chars);
        let mut ids: Vec<u64> = placed.into_iter().collect();
        ids.extend(detail["others"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_u64()));
        let names = crate::profiles::profiles_names(ids).await;
        if let Some(accounts) = detail["accounts"].as_array_mut() {
            for character in accounts.iter_mut().filter_map(|a| a["characters"].as_array_mut()).flatten() {
                if let Some(name) = character["id"].as_u64().and_then(|id| names.get(&id)) {
                    character["name"] = serde_json::Value::String(name.clone());
                }
            }
        }
        if let Some(others) = detail["others"].as_array_mut() {
            for character in others.iter_mut() {
                if let Some(name) = character["id"].as_u64().and_then(|id| names.get(&id)) {
                    character["name"] = serde_json::Value::String(name.clone());
                }
            }
        }
        if let Some(view) = app.get_webview(&me) {
            let _ = view.eval(format!("window.dispatchEvent(new CustomEvent('devils-rage-app:pc-accounts', {{ detail: {detail} }}));"));
        }
    });
}
