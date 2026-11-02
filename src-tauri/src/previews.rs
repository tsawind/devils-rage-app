//! Patch 62t: our own EVE-O Preview. One small borderless, always-on-top window per EVE
//! client, each showing that client's live picture (from capture.rs). Click a preview to
//! bring that client to the front; drag to place it (its spot is remembered per character).
//! Nothing is sent anywhere and nothing touches the game.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Label every preview window shares a prefix, so we can find and close them together.
const PREFIX: &str = "preview-";

fn label_for(name: &str) -> String {
    // Keep it to a safe window label (letters and digits); the page gets the real name.
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{PREFIX}{slug}")
}

/// The EVE clients open right now (character name + whether it's minimized), newest-free order.
#[tauri::command]
pub fn preview_clients() -> Vec<serde_json::Value> {
    crate::clipboard::open_eve_windows()
        .into_iter()
        .map(|(name, handle)| serde_json::json!({ "name": name, "minimized": crate::capture::minimized_handle(handle) }))
        .collect()
}

/// One client's picture as a data URL, for its preview window to draw.
#[tauri::command]
pub fn preview_frame(name: String, width: Option<u32>, quality: Option<u8>) -> Result<String, String> {
    let jpeg = crate::capture::client_jpeg(&name, width.unwrap_or(480).clamp(120, 1280), quality.unwrap_or(70).clamp(30, 90))?;
    Ok(format!("data:image/jpeg;base64,{}", base64_encode(&jpeg)))
}

/// Bring a client to the front (its preview was clicked).
#[tauri::command]
pub fn preview_focus(name: String) -> bool {
    crate::capture::focus_client(&name)
}

/// Open (or refocus) a preview window for every open EVE client. Idempotent.
#[tauri::command]
pub fn open_previews(app: AppHandle) -> Result<usize, String> {
    let clients = crate::clipboard::open_eve_windows();
    if clients.is_empty() {
        return Err("No EVE clients are open.".to_string());
    }
    let mut count = 0usize;
    for (name, _) in clients {
        let label = label_for(&name);
        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.show();
            let _ = existing.set_focus();
            count += 1;
            continue;
        }
        let url = format!("preview.html?name={}", urlencode(&name));
        let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
            .title(&format!("{name} · preview"))
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .inner_size(320.0, 200.0)
            .min_inner_size(160.0, 100.0)
            .build();
        match built {
            Ok(_) => count += 1,
            Err(err) => log::warn!("couldn't open preview for {name}: {err}"),
        }
    }
    log::info!("previews open: {count}");
    Ok(count)
}

/// Close one preview (its own ✕).
#[tauri::command]
pub fn close_preview(window: tauri::WebviewWindow) {
    let _ = window.destroy();
}

/// Close every preview window.
#[tauri::command]
pub fn close_previews(app: AppHandle) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(PREFIX) {
            let _ = window.destroy();
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(CHARS[(n >> 18 & 63) as usize] as char);
        out.push(CHARS[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_safe_and_stable() {
        assert_eq!(label_for("Luigi Squirrel"), "preview-LuigiSquirrel");
        assert_eq!(label_for("Déjà'Vu 2"), "preview-DjVu2");
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"EVE"), "RVZF");
    }

    #[test]
    fn urlencodes_names() {
        assert_eq!(urlencode("Luigi Squirrel"), "Luigi%20Squirrel");
        assert_eq!(urlencode("A-b_c.d~e"), "A-b_c.d~e");
    }
}
