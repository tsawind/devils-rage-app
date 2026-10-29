//! Cameras: one EVE client's own sound, for the mapper's hole watch.
//!
//! A browser can only share the whole computer's sound (every client and Discord mixed), so
//! it can't tell which client heard a wormhole activation. Windows can record one program's
//! sound by itself ("process loopback", Windows 10 2004 or newer); the app does that for the
//! clients the camera page asks for and listens with audiodetect.rs. Nothing is recorded or
//! sent anywhere: the sound is only listened to, here, and the page is told "heard".
//!
//! The camera page (a mapper page in the app's cameras window) asks by opening
//! https://dr-app.invalid/... (see lib.rs):
//! - /audio-clients                      : which EVE clients are open
//! - /audio-listen?character=X&on=1|0    : start / stop listening to X's client
//! - /audio-learn?character=X            : the next loud moment in X's client is the sound
//!
//! and hears back through window events on that page:
//! - devils-rage-app:audio-clients {clients: [names]}
//! - devils-rage-app:audio-level   {character, level (0-100)}  (4 a second while listening)
//! - devils-rage-app:heard         {character, likeness}
//! - devils-rage-app:learned       {character}
//! - devils-rage-app:audio-error   {character, message}

use tauri::{AppHandle, Manager, Url};

/// Tells the page `me` something (a window event).
fn tell(app: &AppHandle, me: &str, event: &str, detail: serde_json::Value) {
    if let Some(view) = app.get_webview(me) {
        let _ = view.eval(format!(
            "window.dispatchEvent(new CustomEvent({}, {{ detail: {} }}));",
            serde_json::Value::String(event.to_string()),
            detail
        ));
    }
}

/// Where the learned activation sound is kept (the same sound for every client).
fn print_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|dir| dir.join("activation-sound.json"))
}

fn load_print(app: &AppHandle) -> Option<Vec<f32>> {
    let text = std::fs::read_to_string(print_file(app)?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_print(app: &AppHandle, print: &[f32]) {
    if let Some(path) = print_file(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(print) {
            let _ = std::fs::write(path, text);
        }
    }
}

pub fn signal(app: &AppHandle, me: &str, url: &Url) {
    let param = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    let character = param("character").unwrap_or_default();
    match url.path() {
        "/audio-clients" => {
            let clients = crate::clipboard::open_eve_characters();
            tell(app, me, "devils-rage-app:audio-clients", serde_json::json!({ "clients": clients, "supported": cfg!(windows) }));
        }
        "/audio-listen" if !character.is_empty() => {
            if param("on").as_deref() == Some("1") {
                listen::start(app, me, &character, load_print(app));
            } else {
                listen::stop(&character);
            }
        }
        "/audio-learn" if !character.is_empty() => listen::learn(&character),
        other => log::warn!("unknown audio signal {other}"),
    }
}

#[cfg(windows)]
mod listen {
    use super::{save_print, tell};
    use crate::audiodetect::{to_mono, Ear, Heard, RATE};
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::{Duration, Instant};
    use tauri::AppHandle;

    struct Listener {
        stop: Arc<AtomicBool>,
        learn: Arc<AtomicBool>,
    }

    static LISTENERS: LazyLock<Mutex<HashMap<String, Listener>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

    /// The process id of "EVE - <character>"'s window.
    fn eve_pid(character: &str) -> Option<u32> {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible};

        struct Search {
            title: String,
            pid: u32,
        }
        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the &mut Search passed to EnumWindows below, alive for the call.
            let search = unsafe { &mut *(data as *mut Search) };
            let mut buf = [0u16; 256];
            // SAFETY: plain Win32 calls on a window handle EnumWindows gave us and a stack buffer.
            let len = unsafe {
                if IsWindowVisible(hwnd) == 0 {
                    return 1;
                }
                GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
            };
            if len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == search.title {
                let mut pid = 0u32;
                // SAFETY: as above; `pid` is a local the call writes.
                unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
                search.pid = pid;
                return 0;
            }
            1
        }
        let mut search = Search { title: format!("EVE - {character}"), pid: 0 };
        // SAFETY: the callback only touches `search` through the pointer, during this call.
        unsafe {
            EnumWindows(Some(each), &mut search as *mut Search as LPARAM);
        }
        (search.pid != 0).then_some(search.pid)
    }

    pub fn start(app: &AppHandle, me: &str, character: &str, learned: Option<Vec<f32>>) {
        let mut listeners = LISTENERS.lock().unwrap();
        if listeners.contains_key(character) {
            return;
        }
        let Some(pid) = eve_pid(character) else {
            tell(app, me, "devils-rage-app:audio-error", serde_json::json!({ "character": character, "message": format!("No EVE window for {character} is open") }));
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let learn = Arc::new(AtomicBool::new(false));
        listeners.insert(character.to_string(), Listener { stop: stop.clone(), learn: learn.clone() });
        let (app, me, character) = (app.clone(), me.to_string(), character.to_string());
        log::info!("listening to {character}'s client (pid {pid})");
        std::thread::spawn(move || {
            if let Err(err) = capture(&app, &me, &character, pid, learned, &stop, &learn) {
                log::warn!("listening to {character}'s client stopped: {err}");
                tell(&app, &me, "devils-rage-app:audio-error", serde_json::json!({ "character": character, "message": format!("Couldn't hear {character}'s client: {err}") }));
            }
            LISTENERS.lock().unwrap().remove(&character);
        });
    }

    pub fn stop(character: &str) {
        if let Some(listener) = LISTENERS.lock().unwrap().remove(character) {
            listener.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn learn(character: &str) {
        if let Some(listener) = LISTENERS.lock().unwrap().get(character) {
            listener.learn.store(true, Ordering::Relaxed);
        }
    }

    fn capture(
        app: &AppHandle,
        me: &str,
        character: &str,
        pid: u32,
        learned: Option<Vec<f32>>,
        stop: &AtomicBool,
        learn: &AtomicBool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _ = wasapi::initialize_mta();
        let format = wasapi::WaveFormat::new(32, 32, wasapi::SampleType::Float, RATE, 2, None);
        let block_align = format.get_blockalign() as usize;
        let mut client = wasapi::AudioClient::new_application_loopback_client(pid, true)?;
        client.initialize_client(&format, wasapi::Direction::Capture, wasapi::StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }, None)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;

        let mut ear = Ear::new(learned);
        let mut queue: VecDeque<u8> = VecDeque::new();
        let mut last_level = Instant::now();
        let mut learn_until: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            if learn.swap(false, Ordering::Relaxed) {
                ear.learning = true;
                learn_until = Some(Instant::now() + Duration::from_secs(30));
            }
            if learn_until.is_some_and(|until| Instant::now() > until) {
                ear.learning = false;
                learn_until = None;
            }
            if capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                capture.read_from_device_to_deque(&mut queue)?;
            }
            let frames = queue.len() / block_align;
            if frames > 0 {
                let bytes: Vec<u8> = queue.drain(..frames * block_align).collect();
                let samples: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                for heard in ear.feed(&to_mono(&samples, 2)) {
                    match heard {
                        Heard::Activation(alike) => {
                            log::info!("activation sound in {character}'s client ({alike:.2})");
                            tell(app, me, "devils-rage-app:heard", serde_json::json!({ "character": character, "likeness": alike }));
                        }
                        Heard::Learned(print) => {
                            save_print(app, &print);
                            learn_until = None;
                            tell(app, me, "devils-rage-app:learned", serde_json::json!({ "character": character }));
                        }
                        Heard::Other(_) => {}
                    }
                }
            }
            if last_level.elapsed() >= Duration::from_millis(250) {
                last_level = Instant::now();
                let level = ((ear.level * 400.0).min(100.0)).round();
                tell(app, me, "devils-rage-app:audio-level", serde_json::json!({ "character": character, "level": level }));
            }
            // No sound for a while is fine (a quiet client); the loop keeps checking for stop.
            let _ = event.wait_for_event(500);
        }
        let _ = client.stop_stream();
        Ok(())
    }
}

#[cfg(not(windows))]
mod listen {
    use tauri::AppHandle;

    pub fn start(app: &AppHandle, me: &str, character: &str, _learned: Option<Vec<f32>>) {
        super::tell(app, me, "devils-rage-app:audio-error", serde_json::json!({ "character": character, "message": "Listening to one client needs Windows" }));
    }

    pub fn stop(_character: &str) {}

    pub fn learn(_character: &str) {}
}
