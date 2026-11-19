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
    recordings(app)
}

/// Where videos, snapshots and scan recordings go (Tsa, 19 Nov: where the app lives, e.g.
/// C:\Devils Rage\Recordings, so they're easy to find and an uninstall's "delete app data" box
/// can't take them). If the app's folder can't be written (Program Files), Documents\Devil's
/// Rage\Recordings; failing that, the old spot in the app's data folder.
pub fn recordings(app: &AppHandle) -> Result<PathBuf, String> {
    static CHOSEN: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let chosen = CHOSEN.get_or_init(|| {
        let writable = |dir: PathBuf| -> Option<PathBuf> {
            std::fs::create_dir_all(&dir).ok()?;
            let probe = dir.join(".write-test");
            std::fs::write(&probe, b"ok").ok()?;
            let _ = std::fs::remove_file(&probe);
            Some(dir)
        };
        let beside_app = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("Recordings")));
        let documents = app.path().document_dir().ok().map(|d| d.join("Devil's Rage").join("Recordings"));
        let found = [beside_app, documents].into_iter().flatten().find_map(writable);
        if let Some(dir) = &found {
            log::info!("recordings go to {}", dir.display());
        }
        found
    });
    if let Some(dir) = chosen {
        return Ok(dir.clone());
    }
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Move recordings from the old spot (the app's data folder) to the new one, once, in the
/// background. Anything that can't move stays where it was.
pub fn move_old_recordings(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("move-recordings".into()).spawn(move || {
        let Ok(old) = app.path().app_local_data_dir().map(|d| d.join("recordings")) else { return };
        let Ok(new) = recordings(&app) else { return };
        if new == old || !old.is_dir() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&old) else { return };
        let (mut moved, mut left) = (0, 0);
        for entry in entries.flatten() {
            let to = new.join(entry.file_name());
            if to.exists() {
                left += 1;
                continue;
            }
            if std::fs::rename(entry.path(), &to).is_ok() || copy_then_remove(&entry.path(), &to) {
                moved += 1;
            } else {
                left += 1;
            }
        }
        if left == 0 {
            let _ = std::fs::remove_dir(&old);
        }
        log::info!("moved {moved} old recordings to {} ({left} left behind)", new.display());
    });
}

/// Another drive: copy it over, then remove the original (a folder is copied file by file).
fn copy_then_remove(from: &std::path::Path, to: &std::path::Path) -> bool {
    fn copy(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        if from.is_dir() {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                copy(&entry.path(), &to.join(entry.file_name()))?;
            }
            Ok(())
        } else {
            std::fs::copy(from, to).map(|_| ())
        }
    }
    if copy(from, to).is_err() {
        return false;
    }
    if from.is_dir() { std::fs::remove_dir_all(from).is_ok() } else { std::fs::remove_file(from).is_ok() }
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
    /// Every monitor together (the whole desktop): boxes anywhere on any screen.
    Desktop { x: i32, y: i32, w: u32, h: u32 },
}

/// Everything there is to take a snapshot of, for the picker window.
#[tauri::command]
pub fn dev_snapshot_targets(app: AppHandle) -> Vec<serde_json::Value> {
    let mut out = vec![json!({ "label": "This app's window", "target": Target::App })];
    if let Ok(monitors) = app.available_monitors() {
        if monitors.len() > 1 {
            let x0 = monitors.iter().map(|m| m.position().x).min().unwrap_or(0);
            let y0 = monitors.iter().map(|m| m.position().y).min().unwrap_or(0);
            let x1 = monitors.iter().map(|m| m.position().x + m.size().width as i32).max().unwrap_or(0);
            let y1 = monitors.iter().map(|m| m.position().y + m.size().height as i32).max().unwrap_or(0);
            let (w, h) = ((x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
            out.push(json!({ "label": format!("All screens · {w} × {h}"), "target": Target::Desktop { x: x0, y: y0, w, h } }));
        }
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
            Target::Desktop { x, y, w, h } => (screen_picture(*x, *y, *w, *h)?, "screens".to_string()),
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

// ------------------------------------------------------------------ video: boxes on a source

/// A video recording: one source (monitor, EVE client, the app), only the boxes switched on
/// (none on: the whole source), at a chosen rate, quality and size.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct VideoSpec {
    pub target: Target,
    /// Boxes in the source's own pixels: x, y, width, height.
    #[serde(default)]
    pub boxes: Vec<[u32; 4]>,
    pub fps: f32,
    pub quality: u8,
    /// Half size (each 2 × 2 pixels averaged) instead of full.
    #[serde(default)]
    pub half: bool,
}

static VIDEO_ON: AtomicBool = AtomicBool::new(false);
/// Seconds since 1970 it started, pictures, bytes and files so far.
static VIDEO_STATS: std::sync::Mutex<(u64, u64, u64, u32)> = std::sync::Mutex::new((0, 0, 0, 0));
/// A file is closed at 30 s or 25 MB, whichever first: small enough to fetch through the bridge.
const VIDEO_PART_SECS: u64 = 30;
const VIDEO_PART_BYTES: u64 = 25 << 20;

/// The source's whole picture now.
fn grab(app: &AppHandle, target: &Target) -> Result<crate::capture::Picture, String> {
    match target {
        Target::App => crate::capture::window_picture(main_handle(app).ok_or("the app window isn't there")?, 8192),
        Target::Client { name } => {
            let handle = crate::clipboard::open_clients().into_iter().find(|(n, _)| n == name).map(|(_, h)| h).ok_or("that EVE client is closed")?;
            crate::capture::window_picture(handle, 8192)
        }
        Target::Monitor { x, y, w, h, .. } | Target::Desktop { x, y, w, h } => screen_picture(*x, *y, *w, *h),
    }
}

/// One piece of a capture: which box, where in the source, its size and pixels (BGRA).
struct Piece {
    k: usize,
    rect: [u32; 4],
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// The boxes (none: all of it) out of a source, at full or half size, and the source's size.
/// On screens each box is read straight off the screen, so small boxes on three 4K monitors cost
/// only their own pixels.
fn pieces(app: &AppHandle, target: &Target, boxes: &[[u32; 4]], half: bool) -> Result<((u32, u32), Vec<Piece>), String> {
    let screen = match target {
        Target::Monitor { x, y, w, h, .. } | Target::Desktop { x, y, w, h } => Some((*x, *y, *w, *h)),
        _ => None,
    };
    if let (Some((sx, sy, sw, sh)), false) = (screen, boxes.is_empty()) {
        let mut out = Vec::new();
        for (k, b) in boxes.iter().enumerate() {
            let (bx, by) = (b[0].min(sw), b[1].min(sh));
            let (bw, bh) = (b[2].min(sw - bx), b[3].min(sh - by));
            if bw < 8 || bh < 8 {
                continue;
            }
            let pic = screen_picture(sx + bx as i32, sy + by as i32, bw, bh)?;
            if let Some((w, h, px)) = cut(&pic, [0, 0, bw, bh], half) {
                out.push(Piece { k, rect: [bx, by, bw, bh], w, h, px });
            }
        }
        return Ok(((sw, sh), out));
    }
    let pic = grab(app, target)?;
    let all = if boxes.is_empty() { vec![[0, 0, pic.width, pic.height]] } else { boxes.to_vec() };
    let out = all
        .iter()
        .enumerate()
        .filter_map(|(k, b)| cut(&pic, *b, half).map(|(w, h, px)| Piece { k, rect: *b, w, h, px }))
        .collect();
    Ok(((pic.width, pic.height), out))
}

/// Open the app's recordings folder (snapshots and videos) in Explorer.
#[tauri::command]
pub fn dev_open_recordings(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = folder(&app)?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// One box out of a picture (clipped to it), at full or half size, BGRA.
fn cut(pic: &crate::capture::Picture, b: [u32; 4], half: bool) -> Option<(u32, u32, Vec<u8>)> {
    let (x0, y0) = (b[0].min(pic.width), b[1].min(pic.height));
    let (x1, y1) = ((b[0] + b[2]).min(pic.width), (b[1] + b[3]).min(pic.height));
    if x1 < x0 + 8 || y1 < y0 + 8 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let at = |x: u32, y: u32| ((y * pic.width + x) * 4) as usize;
    if !half {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y1 {
            out.extend_from_slice(&pic.bgra[at(x0, y)..at(x1, y)]);
        }
        return Some((w, h, out));
    }
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0u8; (hw * hh * 4) as usize];
    for y in 0..hh {
        for x in 0..hw {
            let (sx, sy) = (x0 + x * 2, y0 + y * 2);
            for c in 0..4 {
                let sum = pic.bgra[at(sx, sy) + c] as u32 + pic.bgra[at(sx + 1, sy) + c] as u32 + pic.bgra[at(sx, sy + 1) + c] as u32 + pic.bgra[at(sx + 1, sy + 1) + c] as u32;
                out[((y * hw + x) * 4) as usize + c] = (sum / 4) as u8;
            }
        }
    }
    Some((hw, hh, out))
}

/// The source as a still, at most 1100 wide, for drawing the boxes on: its full size, then a JPEG.
#[tauri::command]
pub async fn dev_video_still(app: AppHandle, target: Target) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let pic = grab(&app, &target)?;
        let k = (pic.width / 1100 + 1).max(1);
        let (w, h) = (pic.width / k, pic.height / k);
        let mut small = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let s = (((y * k) * pic.width + x * k) * 4) as usize;
                small[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].copy_from_slice(&pic.bgra[s..s + 4]);
            }
        }
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 85).encode(&small, w as u16, h as u16, jpeg_encoder::ColorType::Bgra).map_err(|e| e.to_string())?;
        Ok(json!({ "w": pic.width, "h": pic.height, "jpeg": crate::previews::base64_encode(&jpeg) }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start a video recording (stops one already running first). Returns the folder.
#[tauri::command]
pub fn dev_video_start(app: AppHandle, spec: VideoSpec) -> Result<String, String> {
    let dir = folder(&app)?;
    VIDEO_ON.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(150));
    let stamp = now_secs();
    *VIDEO_STATS.lock().unwrap() = (stamp, 0, 0, 0);
    VIDEO_ON.store(true, Ordering::SeqCst);
    let out = dir.clone();
    std::thread::Builder::new()
        .name("dev-video".into())
        .spawn(move || video_loop(app, out, stamp, spec))
        .map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

#[tauri::command]
pub fn dev_video_stop() {
    VIDEO_ON.store(false, Ordering::SeqCst);
}

/// Recording a video: seconds, pictures, MB, files.
#[tauri::command]
pub fn dev_video_status() -> serde_json::Value {
    let (started, frames, bytes, files) = *VIDEO_STATS.lock().unwrap();
    if !VIDEO_ON.load(Ordering::SeqCst) {
        return json!({ "on": false, "frames": frames, "mb": bytes as f64 / 1048576.0, "files": files });
    }
    json!({ "on": true, "secs": now_secs().saturating_sub(started), "frames": frames, "mb": bytes as f64 / 1048576.0, "files": files })
}

/// Recording a video, for how many seconds (for the bar).
pub fn video_secs() -> Option<u64> {
    VIDEO_ON.load(Ordering::SeqCst).then(|| now_secs().saturating_sub(VIDEO_STATS.lock().unwrap().0))
}

/// The video: `vid-<stamp>-NNN.bin`, REC2 like the others: per box per picture a note (ms, which
/// box, where in the source, its size; once a second also the page and the scan's status) and a
/// JPEG. Closed at 30 s or 25 MB; an hour at most.
fn video_loop(app: AppHandle, dir: PathBuf, stamp: u64, spec: VideoSpec) {
    crate::planetscan::sky::background_thread();
    let started = Instant::now();
    let every = Duration::from_secs_f32(1.0 / spec.fps.clamp(0.5, 30.0));
    let quality = spec.quality.clamp(30, 95);
    let (mut part, mut part_bytes) = (0u32, 0u64);
    let mut out: Option<(std::io::BufWriter<std::fs::File>, Instant)> = None;
    let mut details_at: Option<Instant> = None;
    while VIDEO_ON.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(3600) {
        let at = Instant::now();
        if out.as_ref().is_none_or(|(_, t)| t.elapsed() >= Duration::from_secs(VIDEO_PART_SECS) || part_bytes >= VIDEO_PART_BYTES) {
            if let Some((mut w, _)) = out.take() {
                let _ = w.flush();
            }
            part += 1;
            part_bytes = 0;
            match std::fs::File::create(dir.join(format!("vid-{stamp}-{part:03}.bin"))) {
                Ok(file) => {
                    let mut w = std::io::BufWriter::new(file);
                    let _ = w.write_all(b"REC2");
                    out = Some((w, Instant::now()));
                    VIDEO_STATS.lock().unwrap().3 = part;
                }
                Err(e) => {
                    log::warn!("video: couldn't write ({e})");
                    break;
                }
            }
        }
        match pieces(&app, &spec.target, &spec.boxes, spec.half) {
            Ok((source, got)) => {
                let details = details_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
                if details {
                    details_at = Some(Instant::now());
                }
                for (n, p) in got.iter().enumerate() {
                    let mut jpeg = Vec::with_capacity(256 * 1024);
                    if jpeg_encoder::Encoder::new(&mut jpeg, quality).encode(&p.px, p.w as u16, p.h as u16, jpeg_encoder::ColorType::Bgra).is_err() {
                        continue;
                    }
                    let mut note = json!({ "ms": started.elapsed().as_millis() as u64, "at": now_secs(), "box": p.k, "rect": p.rect, "w": p.w, "h": p.h, "source": [source.0, source.1] });
                    if details && n == 0 {
                        note["page"] = json!(page_url(&app));
                        note["scan"] = scan_status(&app);
                    }
                    let note = note.to_string();
                    let Some((wr, _)) = out.as_mut() else { break };
                    let ok = wr.write_all(&(note.len() as u32).to_le_bytes()).is_ok()
                        && wr.write_all(note.as_bytes()).is_ok()
                        && wr.write_all(&(jpeg.len() as u32).to_le_bytes()).is_ok()
                        && wr.write_all(&jpeg).is_ok();
                    if !ok {
                        VIDEO_ON.store(false, Ordering::SeqCst);
                        break;
                    }
                    let n = 8 + note.len() as u64 + jpeg.len() as u64;
                    part_bytes += n;
                    let mut st = VIDEO_STATS.lock().unwrap();
                    st.1 += 1;
                    st.2 += n;
                }
            }
            Err(e) => log::debug!("video: no picture ({e})"),
        }
        std::thread::sleep(every.saturating_sub(at.elapsed()));
    }
    if let Some((mut w, _)) = out.take() {
        let _ = w.flush();
    }
    VIDEO_ON.store(false, Ordering::SeqCst);
    let (_, frames, bytes, files) = *VIDEO_STATS.lock().unwrap();
    log::info!("video saved: vid-{stamp}-*.bin, {frames} pictures, {} MB in {files} files", bytes >> 20);
}

/// Open the capture window (snapshots and video of a source, with boxes).
pub fn open_video(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(VIDEO_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = tauri::WebviewWindowBuilder::new(app, VIDEO_WINDOW, tauri::WebviewUrl::App("video.html".into()))
        .title("Capture (dev)")
        .inner_size(760.0, 560.0)
        .min_inner_size(380.0, 260.0)
        .resizable(true)
        .center()
        .build();
    if let Err(e) = built {
        log::warn!("couldn't open the video recorder: {e}");
    }
}
const VIDEO_WINDOW: &str = "video";

/// A snapshot of the boxes switched on (none: all of the source), each at full size, with the
/// details file. Returns the files' common name. For pressing again and again while showing a bug.
#[tauri::command]
pub async fn dev_snapshot_boxes(app: AppHandle, target: Target, boxes: Vec<[u32; 4]>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = folder(&app)?;
        // Unique even when pressed several times a second.
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let (source, got) = pieces(&app, &target, &boxes, false)?;
        let mut files = Vec::new();
        let mut rects = Vec::new();
        for p in &got {
            let mut jpeg = Vec::with_capacity(1 << 20);
            jpeg_encoder::Encoder::new(&mut jpeg, 92).encode(&p.px, p.w as u16, p.h as u16, jpeg_encoder::ColorType::Bgra).map_err(|e| e.to_string())?;
            let name = format!("snap-{stamp}-{}.jpg", p.k + 1);
            std::fs::write(dir.join(&name), jpeg).map_err(|e| e.to_string())?;
            files.push(name);
            rects.push(p.rect);
        }
        let s = crate::settings::load(&app);
        let details = json!({
            "at": stamp / 1000, "of": target, "boxes": rects, "files": files, "source": [source.0, source.1],
            "version": app.package_info().version.to_string(), "channel": s.channel(&app).as_str(),
            "page": page_url(&app), "scan": scan_status(&app), "log": crate::commands::read_log(app.clone()),
        });
        std::fs::write(dir.join(format!("snap-{stamp}.json")), serde_json::to_vec_pretty(&details).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        log::info!("snapshot snap-{stamp}: {} box(es)", files.len());
        *LAST_SNAP.lock().unwrap() = Some((format!("snap-{stamp}"), Instant::now()));
        Ok(format!("snap-{stamp} ({} picture{})", files.len(), if files.len() == 1 { "" } else { "s" }))
    })
    .await
    .map_err(|e| e.to_string())?
}
