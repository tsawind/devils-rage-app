//! Developer tools in the top bar (shown on the Dev channel only): record everything, and take a
//! snapshot. So a problem can be looked at afterwards without screenshots going back and forth.
//!
//! Record: the whole app window (what the pilot sees: bar, page, maps, Do now) twice a second,
//! with the page's address and the planet scan's whole status, in `app-<stamp>-NNN.bin` files of
//! 30 s; and when a planet scan is running, EVE's own pictures as before (`scan2-<stamp>-NNN.bin`,
//! 5 a second, with the scan's status). Same stamp, so the two line up by their times.
//! Files: "REC2", then for each picture a u32 (LE) length and a JSON note, a u32 length and a JPEG.
//!
//! Snapshot: `snap-<stamp>.jpg` (the app window), `snap-<stamp>-eve.jpg` (EVE's picture, when a
//! scan is running) and `snap-<stamp>.json` (version, page, the scan's status, the end of the log).
//!
//! All in the app's recordings folder; nothing leaves the PC.

use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

static RECORDING: AtomicBool = AtomicBool::new(false);
/// The last snapshot's name and when (for the bar to say it was taken).
static LAST_SNAP: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);
/// When the recording started (seconds since 1970), 0 when not recording.
static STARTED: AtomicU64 = AtomicU64::new(0);
/// The app window is at most this wide in recordings and snapshots (full detail to read text).
const APP_WIDTH: u32 = 1920;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn folder(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// The main window's handle (its whole inside: the bar and the page under it).
fn main_handle(app: &AppHandle) -> Option<isize> {
    #[cfg(windows)]
    {
        let window = app.get_window(crate::MAIN_WINDOW)?;
        window.hwnd().ok().map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        None
    }
}

/// The app window as a JPEG (None when minimised or it can't be read).
fn app_jpeg(app: &AppHandle, quality: u8) -> Option<Vec<u8>> {
    let pic = crate::capture::window_picture(main_handle(app)?, APP_WIDTH).ok()?;
    let mut jpeg = Vec::with_capacity(512 * 1024);
    jpeg_encoder::Encoder::new(&mut jpeg, quality)
        .encode(&pic.bgra, pic.width as u16, pic.height as u16, jpeg_encoder::ColorType::Bgra)
        .ok()?;
    Some(jpeg)
}

/// Where the main window is (its address), for the notes.
fn page_url(app: &AppHandle) -> String {
    crate::shown_view(app).and_then(|v| v.url().ok()).map(|u| u.to_string()).unwrap_or_default()
}

/// The planet scan's status as JSON (null when it isn't running).
fn scan_status(app: &AppHandle) -> serde_json::Value {
    let text = app.state::<crate::planetscan::Engine>().status_text();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) if v.as_object().is_some_and(|o| !o.is_empty()) => v,
        _ => serde_json::Value::Null,
    }
}

/// The snapshot just taken (for 4 s), for the bar.
pub fn last_snapshot() -> Option<String> {
    LAST_SNAP.lock().unwrap().as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(n, _)| n.clone())
}

/// Recording, and for how many seconds (for the bar).
pub fn recording_secs() -> Option<u64> {
    RECORDING.load(Ordering::SeqCst).then(|| now_secs().saturating_sub(STARTED.load(Ordering::SeqCst)))
}

/// Start or stop recording everything. Returns the folder it records into.
pub fn set_recording(app: &AppHandle, on: bool) -> Result<String, String> {
    let dir = folder(app)?;
    let engine = app.state::<crate::planetscan::Engine>();
    if !on {
        RECORDING.store(false, Ordering::SeqCst);
        engine.set_recording(app, false, 0)?;
        return Ok(dir.display().to_string());
    }
    if RECORDING.swap(true, Ordering::SeqCst) {
        return Ok(dir.display().to_string());
    }
    let stamp = now_secs();
    STARTED.store(stamp, Ordering::SeqCst);
    // EVE's pictures, when a planet scan is watching a client (it records them as they come).
    engine.set_recording(app, true, stamp)?;
    let app = app.clone();
    let out = dir.clone();
    std::thread::Builder::new()
        .name("dev-record".into())
        .spawn(move || record_loop(app, out, stamp))
        .map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// The app window twice a second, in 30 s files, until stopped (or after an hour).
fn record_loop(app: AppHandle, dir: PathBuf, stamp: u64) {
    crate::planetscan::sky::background_thread();
    let started = Instant::now();
    let (mut part, mut n) = (0u32, 0u64);
    let mut out: Option<(std::io::BufWriter<std::fs::File>, Instant)> = None;
    while RECORDING.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(3600) {
        let at = Instant::now();
        if out.as_ref().is_none_or(|(_, t)| t.elapsed() >= Duration::from_secs(30)) {
            if let Some((mut w, _)) = out.take() {
                let _ = w.flush();
            }
            part += 1;
            match std::fs::File::create(dir.join(format!("app-{stamp}-{part:03}.bin"))) {
                Ok(file) => {
                    let mut w = std::io::BufWriter::new(file);
                    let _ = w.write_all(b"REC2");
                    out = Some((w, Instant::now()));
                }
                Err(e) => {
                    log::warn!("dev recording: couldn't write ({e})");
                    break;
                }
            }
        }
        if let (Some(jpeg), Some((w, _))) = (app_jpeg(&app, 80), out.as_mut()) {
            let ms = started.elapsed().as_millis() as u64;
            let note = json!({ "ms": ms, "at": now_secs(), "page": page_url(&app), "scan": scan_status(&app) }).to_string();
            let ok = w.write_all(&(note.len() as u32).to_le_bytes()).is_ok()
                && w.write_all(note.as_bytes()).is_ok()
                && w.write_all(&(jpeg.len() as u32).to_le_bytes()).is_ok()
                && w.write_all(&jpeg).is_ok();
            if !ok {
                break;
            }
            n += 1;
        }
        std::thread::sleep(Duration::from_millis(500).saturating_sub(at.elapsed()));
    }
    if let Some((mut w, _)) = out.take() {
        let _ = w.flush();
    }
    RECORDING.store(false, Ordering::SeqCst);
    let _ = app.state::<crate::planetscan::Engine>().set_recording(&app, false, 0);
    log::info!("dev recording saved: {n} pictures of the app in {part} files (app-{stamp}-*.bin)");
}

/// One snapshot of everything: the app window, EVE's picture (when scanning) and the details.
/// Returns the files' common name.
pub fn snapshot(app: &AppHandle) -> Result<String, String> {
    let dir = folder(app)?;
    let stamp = now_secs();
    let stem = dir.join(format!("snap-{stamp}"));
    let mut files = Vec::new();
    if let Some(jpeg) = app_jpeg(app, 92) {
        std::fs::write(stem.with_extension("jpg"), jpeg).map_err(|e| e.to_string())?;
        files.push("app window");
    }
    if let Some(f) = app.state::<crate::planetscan::Engine>().latest_frame() {
        let mut jpeg = Vec::with_capacity(1 << 20);
        if jpeg_encoder::Encoder::new(&mut jpeg, 92).encode(&f.bgra, f.w as u16, f.h as u16, jpeg_encoder::ColorType::Bgra).is_ok() {
            std::fs::write(dir.join(format!("snap-{stamp}-eve.jpg")), jpeg).map_err(|e| e.to_string())?;
            files.push("EVE");
        }
    }
    let s = crate::settings::load(app);
    let details = json!({
        "at": stamp,
        "version": app.package_info().version.to_string(),
        "channel": s.channel(app).as_str(),
        "page": page_url(app),
        "scan": scan_status(app),
        "log": crate::commands::read_log(app.clone()),
    });
    std::fs::write(stem.with_extension("json"), serde_json::to_vec_pretty(&details).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    files.push("details");
    log::info!("snapshot snap-{stamp}: {}", files.join(", "));
    *LAST_SNAP.lock().unwrap() = Some((format!("snap-{stamp}"), Instant::now()));
    Ok(format!("snap-{stamp}"))
}

// ------------------------------------------------------------------ snapshot of one thing

/// What a snapshot can be of: the app window, a whole monitor, or one EVE client.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    App,
    /// Monitor's place and size on the desktop (physical pixels).
    Monitor { name: String, x: i32, y: i32, w: u32, h: u32 },
    Client { name: String },
}

/// Everything there is to take a snapshot of, for the picker window.
#[tauri::command]
pub fn dev_snapshot_targets(app: AppHandle) -> Vec<serde_json::Value> {
    let mut out = vec![json!({ "label": "This app's window", "target": Target::App })];
    if let Ok(monitors) = app.available_monitors() {
        for (i, m) in monitors.iter().enumerate() {
            let (pos, size) = (m.position(), m.size());
            let name = format!("Monitor {}", i + 1);
            out.push(json!({ "label": format!("{name} · {} × {}", size.width, size.height), "target": Target::Monitor { name, x: pos.x, y: pos.y, w: size.width, h: size.height } }));
        }
    }
    let mut clients = crate::clipboard::open_clients();
    clients.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, _) in clients {
        out.push(json!({ "label": format!("EVE: {name}"), "target": Target::Client { name } }));
    }
    out
}

/// Snapshot of one thing at full size (no smaller than the screen shows it), with the details file.
/// Returns the picture's file name.
#[tauri::command]
pub async fn dev_snapshot_take(app: AppHandle, target: Target) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = folder(&app)?;
        let stamp = now_secs();
        let (pic, what) = match &target {
            Target::App => (crate::capture::window_picture(main_handle(&app).ok_or("the app window isn't there")?, 8192)?, "app".to_string()),
            Target::Client { name } => {
                let handle = crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h).ok_or("that EVE client is closed")?;
                (crate::capture::window_picture(handle, 8192)?, format!("eve-{}", safe(name)))
            }
            Target::Monitor { name, x, y, w, h } => (screen_picture(*x, *y, *w, *h)?, safe(name).to_lowercase()),
        };
        let mut jpeg = Vec::with_capacity(2 << 20);
        jpeg_encoder::Encoder::new(&mut jpeg, 92)
            .encode(&pic.bgra, pic.width as u16, pic.height as u16, jpeg_encoder::ColorType::Bgra)
            .map_err(|e| e.to_string())?;
        let file = format!("snap-{stamp}-{what}.jpg");
        std::fs::write(dir.join(&file), jpeg).map_err(|e| e.to_string())?;
        let s = crate::settings::load(&app);
        let details = json!({
            "at": stamp,
            "of": target,
            "size": [pic.width, pic.height],
            "version": app.package_info().version.to_string(),
            "channel": s.channel(&app).as_str(),
            "page": page_url(&app),
            "scan": scan_status(&app),
            "log": crate::commands::read_log(app.clone()),
        });
        std::fs::write(dir.join(format!("snap-{stamp}-{what}.json")), serde_json::to_vec_pretty(&details).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        log::info!("snapshot {file}");
        *LAST_SNAP.lock().unwrap() = Some((file.clone(), Instant::now()));
        Ok(file)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A name safe in a file name.
fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Open the snapshot picker (a small window over the app).
pub fn open_picker(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PICKER) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = tauri::WebviewWindowBuilder::new(app, PICKER, tauri::WebviewUrl::App("snapshot.html".into()))
        .title("Snapshot of…")
        .inner_size(380.0, 460.0)
        .resizable(true)
        .always_on_top(true)
        .center()
        .build();
    if let Err(e) = built {
        log::warn!("couldn't open the snapshot picker: {e}");
    }
}
const PICKER: &str = "snapshot";

/// A part of the desktop (a whole monitor) as it's shown now, BGRA.
#[cfg(windows)]
fn screen_picture(x: i32, y: i32, w: u32, h: u32) -> Result<crate::capture::Picture, String> {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    };
    let (w, h) = (w as i32, h as i32);
    if w < 16 || h < 16 {
        return Err("that monitor has no size".into());
    }
    // SAFETY: GDI calls on handles created here and released before returning.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(dc, bitmap);
        let copied = BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT) != 0;
        SelectObject(dc, old);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let rows = GetDIBits(screen, bitmap, 0, h as u32, bgra.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);
        if !copied || rows == 0 {
            return Err("couldn't read the screen".into());
        }
        Ok(crate::capture::Picture { width: w as u32, height: h as u32, bgra })
    }
}

#[cfg(not(windows))]
fn screen_picture(_x: i32, _y: i32, _w: u32, _h: u32) -> Result<crate::capture::Picture, String> {
    Err("only on Windows".into())
}
