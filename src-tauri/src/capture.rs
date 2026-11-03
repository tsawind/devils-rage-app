//! Cameras: the app takes each EVE client's picture itself, so the mapper's share page doesn't
//! need the browser's window picker (which always asks, every time). The page fetches frames
//! from this app-only address, a few a second, and draws them into its own video:
//!
//! - http://drcap.localhost/windows             : [{name, minimized}] for every EVE client
//! - http://drcap.localhost/frame?name=X&w=1920&q=75 : that client's picture now (JPEG)
//!
//! Only the mapper's own pages may ask (checked by the request's Origin). Nothing is sent
//! anywhere and nothing touches the game: it is the same picture Windows shows on screen.

use tauri::http::{header, Request, Response, StatusCode};
use tauri::Url;

pub const SCHEME: &str = "drcap";

/// The widest picture handed out, and the JPEG quality, unless the page asks for less.
const MAX_WIDTH: u32 = 1920;
const QUALITY: u8 = 75;

/// Only the mapper's own pages (the share window, the map).
pub fn allowed_origin(origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(|host| host == crate::host_of(crate::MAPPER_URL)))
        .unwrap_or(false)
}

/// "/frame" and "name=X&w=1" from any spelling of the address.
pub fn path_and_query(uri: &str) -> (String, String) {
    let rest = match uri.find("://") {
        Some(i) => {
            let after = &uri[i + 3..];
            after.find('/').map(|j| &after[j..]).unwrap_or("/")
        }
        None => uri,
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.split('#').next().unwrap_or("").trim_end_matches('/');
    (if path.is_empty() { "/".to_string() } else { path.to_string() }, query.split('#').next().unwrap_or("").to_string())
}

/// Width and quality the page asked for, within limits.
pub fn wanted(width: Option<&str>, quality: Option<&str>) -> (u32, u8) {
    let width = width.and_then(|w| w.parse::<u32>().ok()).unwrap_or(MAX_WIDTH).clamp(160, MAX_WIDTH);
    let quality = quality.and_then(|q| q.parse::<u8>().ok()).unwrap_or(QUALITY).clamp(30, 92);
    (width, quality)
}

fn reply(status: StatusCode, kind: &str, body: Vec<u8>, origin: &str) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        .header(header::CACHE_CONTROL, "no-store");
    if !origin.is_empty() {
        response = response
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header("Access-Control-Allow-Private-Network", "true")
            .header(header::VARY, "Origin");
    }
    response.body(body).unwrap_or_else(|_| Response::new(Vec::new()))
}

pub fn handle(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !allowed_origin(&origin) {
        return reply(StatusCode::FORBIDDEN, "text/plain", b"not for this page".to_vec(), "");
    }
    if request.method() == tauri::http::Method::OPTIONS {
        return reply(StatusCode::NO_CONTENT, "text/plain", Vec::new(), &origin);
    }
    // Path and query straight from the request (however the webview spells the address:
    // "http://drcap.localhost/frame?…", "drcap://localhost/frame?…" or just "/frame?…").
    let (path, query) = path_and_query(&request.uri().to_string());
    let url = Url::parse(&format!("http://{SCHEME}.localhost/?{query}")).ok();
    let param = |key: &str| url.as_ref().and_then(|url| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()));

    match path.as_str() {
        "/windows" => {
            let list: Vec<serde_json::Value> = crate::clipboard::open_eve_windows()
                .into_iter()
                .map(|(name, handle)| serde_json::json!({ "name": name, "minimized": grab::minimized(handle) }))
                .collect();
            reply(StatusCode::OK, "application/json", serde_json::to_vec(&list).unwrap_or_default(), &origin)
        }
        "/frame" => {
            let Some(name) = param("name") else {
                return reply(StatusCode::BAD_REQUEST, "text/plain", b"which client?".to_vec(), &origin);
            };
            let Some((_, handle)) = crate::clipboard::open_eve_windows().into_iter().find(|(open, _)| *open == name) else {
                log::info!("camera picture: no EVE window called {name:?} (open: {:?})", crate::clipboard::open_eve_windows().into_iter().map(|(n, _)| n).collect::<Vec<_>>());
                return reply(StatusCode::NOT_FOUND, "text/plain", b"no such EVE window".to_vec(), &origin);
            };
            let (width, quality) = wanted(param("w").as_deref(), param("q").as_deref());
            match grab::picture(handle, width).and_then(|picture| encode(&picture, quality)) {
                Ok(jpeg) => reply(StatusCode::OK, "image/jpeg", jpeg, &origin),
                Err(why) => reply(StatusCode::CONFLICT, "text/plain", why.as_bytes().to_vec(), &origin),
            }
        }
        other => {
            log::warn!("camera picture: unknown address {other:?} ({})", request.uri());
            reply(StatusCode::BAD_REQUEST, "text/plain", b"unknown".to_vec(), &origin)
        }
    }
}

/// A client's picture: width × height, 4 bytes a pixel (blue, green, red, unused), top row first.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

fn encode(picture: &Picture, quality: u8) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(256 * 1024);
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&picture.bgra, picture.width as u16, picture.height as u16, jpeg_encoder::ColorType::Bgra)
        .map_err(|_| "couldn't encode the picture")?;
    Ok(out)
}

/// Patch 62t: one EVE client's picture as JPEG bytes, for the previews (previews.rs). The name
/// is the character ("EVE - <name>"); the client must be open and not minimized.
pub fn client_jpeg(name: &str, width: u32, quality: u8) -> Result<Vec<u8>, String> {
    let (_, handle) = crate::clipboard::open_clients()
        .into_iter()
        .find(|(open, _)| open == name)
        .ok_or_else(|| "that EVE client is closed".to_string())?;
    let picture = grab::picture(handle, width).map_err(str::to_string)?;
    encode(&picture, quality).map_err(str::to_string)
}

/// Whether a window handle is minimized (for the previews).
pub fn minimized_handle(handle: isize) -> bool {
    grab::minimized(handle)
}

/// Bring one EVE client to the front (clicking its preview). True if it was found.
/// Bring a window forward by handle (a client still at the login screen has no character name).
pub fn focus_handle(handle: isize) -> bool {
    grab::focus(handle)
}

pub fn focus_client(name: &str) -> bool {
    match crate::clipboard::open_clients().into_iter().find(|(open, _)| open == name) {
        Some((_, handle)) => grab::focus(handle),
        None => false,
    }
}

#[cfg(windows)]
mod grab {
    use super::Picture;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, SetBrushOrgEx,
        SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
    };
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic};

    /// PW_CLIENTONLY | PW_RENDERFULLCONTENT: the inside of the window, DirectX content too.
    const PRINT_FLAGS: u32 = 1 | 2;

    pub fn minimized(handle: isize) -> bool {
        // SAFETY: a plain query on a window handle (a stale one just answers no).
        unsafe { IsIconic(handle as _) != 0 }
    }

    pub fn focus(handle: isize) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};
        let hwnd = handle as _;
        // SAFETY: plain window calls on a handle EnumWindows gave us; a stale one just fails.
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd) != 0
        }
    }

    pub fn picture(handle: isize, max_width: u32) -> Result<Picture, &'static str> {
        let hwnd = handle as _;
        // SAFETY: GDI calls on handles created here and released before returning; the
        // window handle comes from EnumWindows (a closed window makes the calls fail, not crash).
        unsafe {
            if IsIconic(hwnd) != 0 {
                return Err("minimized");
            }
            let mut rect: RECT = std::mem::zeroed();
            if GetClientRect(hwnd, &mut rect) == 0 {
                return Err("gone");
            }
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if width < 16 || height < 16 {
                return Err("minimized");
            }
            let scale = (max_width as f64 / width as f64).min(1.0);
            let out_width = ((width as f64 * scale).round() as i32).max(2);
            let out_height = ((height as f64 * scale).round() as i32).max(2);

            let screen = GetDC(std::ptr::null_mut());
            let full_dc = CreateCompatibleDC(screen);
            let full_bitmap = CreateCompatibleBitmap(screen, width, height);
            let full_old = SelectObject(full_dc, full_bitmap);
            let printed = PrintWindow(hwnd, full_dc, PRINT_FLAGS) != 0;

            let out_dc = CreateCompatibleDC(screen);
            let out_bitmap = CreateCompatibleBitmap(screen, out_width, out_height);
            let out_old = SelectObject(out_dc, out_bitmap);
            SetStretchBltMode(out_dc, HALFTONE);
            SetBrushOrgEx(out_dc, 0, 0, std::ptr::null_mut());
            StretchBlt(out_dc, 0, 0, out_width, out_height, full_dc, 0, 0, width, height, SRCCOPY);
            SelectObject(out_dc, out_old);

            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: out_width,
                // Negative: top row first.
                biHeight: -out_height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bgra = vec![0u8; (out_width * out_height * 4) as usize];
            let rows = GetDIBits(screen, out_bitmap, 0, out_height as u32, bgra.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);

            SelectObject(full_dc, full_old);
            DeleteObject(full_bitmap);
            DeleteObject(out_bitmap);
            DeleteDC(full_dc);
            DeleteDC(out_dc);
            ReleaseDC(std::ptr::null_mut(), screen);

            if !printed || rows == 0 {
                return Err("couldn't read the window");
            }
            Ok(Picture { width: out_width as u32, height: out_height as u32, bgra })
        }
    }
}

#[cfg(not(windows))]
mod grab {
    use super::Picture;

    pub fn minimized(_handle: isize) -> bool {
        false
    }

    pub fn focus(_handle: isize) -> bool {
        false
    }

    pub fn picture(_handle: isize, _max_width: u32) -> Result<Picture, &'static str> {
        Err("only on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_of_the_address() {
        for uri in ["http://drcap.localhost/frame?name=A%20B&w=1", "drcap://localhost/frame?name=A%20B&w=1", "/frame?name=A%20B&w=1", "https://drcap.localhost/frame/?name=A%20B&w=1"] {
            assert_eq!(path_and_query(uri), ("/frame".to_string(), "name=A%20B&w=1".to_string()), "{uri}");
        }
        assert_eq!(path_and_query("http://drcap.localhost/windows").0, "/windows");
    }

    #[test]
    fn only_the_mapper_may_ask() {
        assert!(allowed_origin(crate::MAPPER_URL));
        assert!(!allowed_origin("https://example.com"));
        assert!(!allowed_origin(""));
        assert!(!allowed_origin("null"));
    }

    #[test]
    fn keeps_size_and_quality_in_bounds() {
        assert_eq!(wanted(None, None), (MAX_WIDTH, QUALITY));
        assert_eq!(wanted(Some("640"), Some("60")), (640, 60));
        assert_eq!(wanted(Some("99999"), Some("5")), (MAX_WIDTH, 30));
        assert_eq!(wanted(Some("x"), Some("y")), (MAX_WIDTH, QUALITY));
    }

    #[test]
    fn encodes_a_picture() {
        let picture = Picture { width: 4, height: 2, bgra: vec![128; 4 * 2 * 4] };
        let jpeg = encode(&picture, 75).expect("jpeg");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
