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
