//! 27 Nov: the client bar's camera records an EVE client to a video file on this PC, in the
//! app's Videos folder: what the client shows, with its own sound when Windows can give it
//! (process loopback, as the hole watch's listening). Nothing leaves the PC.
//!
//! Files are `cam <character> <date> <time>.avi`: Motion-JPEG pictures and 16-bit stereo sound,
//! which Windows' Media Player, Films & TV and VLC play. A new file every 10 minutes (or when the
//! client's size changes), so a crash loses at most the last one's index (VLC still plays it).

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

const PART: Duration = Duration::from_secs(600);
const PART_BYTES: u64 = 900 << 20;
/// Pictures this wide at most (EVE's text stays readable).
const WIDTH: u32 = 1280;
const QUALITY: u8 = 70;
pub const AUDIO_RATE: u32 = 48_000;

struct Recording {
    stop: Arc<AtomicBool>,
    started: Instant,
    fps: u32,
    /// The file being written, bytes so far (all parts), parts, whether sound is in it.
    state: Arc<Mutex<(PathBuf, u64, u32, bool)>>,
}

static RECORDINGS: LazyLock<Mutex<HashMap<String, Recording>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// An AVI file being written: Motion-JPEG video, and PCM sound if `audio`.
pub struct Avi {
    file: std::io::BufWriter<std::fs::File>,
    w: u32,
    h: u32,
    fps: u32,
    audio: bool,
    frames: u32,
    audio_bytes: u64,
    /// Offset of the movi list's 'movi' word (idx1 offsets count from it), and the index.
    movi_at: u64,
    index: Vec<([u8; 4], u32, u32)>,
    written: u64,
}

fn u32le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

impl Avi {
    fn header(&self) -> Vec<u8> {
        let mut strl_v = Vec::new();
        // strh (video)
        strl_v.extend(b"strh");
        strl_v.extend(u32le(56));
        strl_v.extend(b"vids");
        strl_v.extend(b"MJPG");
        strl_v.extend(u32le(0)); // flags
        strl_v.extend([0u8; 4]); // priority, language
        strl_v.extend(u32le(0)); // initial frames
        strl_v.extend(u32le(1)); // scale
        strl_v.extend(u32le(self.fps)); // rate
        strl_v.extend(u32le(0)); // start
        strl_v.extend(u32le(self.frames)); // length
        strl_v.extend(u32le(1 << 20)); // suggested buffer
        strl_v.extend(u32le(u32::MAX)); // quality
        strl_v.extend(u32le(0)); // sample size
        strl_v.extend([0u8; 4]);
        strl_v.extend((self.w as u16).to_le_bytes());
        strl_v.extend((self.h as u16).to_le_bytes());
        // strf: BITMAPINFOHEADER
        strl_v.extend(b"strf");
        strl_v.extend(u32le(40));
        strl_v.extend(u32le(40));
        strl_v.extend(u32le(self.w));
        strl_v.extend(u32le(self.h));
        strl_v.extend(1u16.to_le_bytes());
        strl_v.extend(24u16.to_le_bytes());
        strl_v.extend(b"MJPG");
        strl_v.extend(u32le(self.w * self.h * 3));
        strl_v.extend([0u8; 16]);

        let mut strl_a = Vec::new();
        if self.audio {
            let block = 4u32;
            strl_a.extend(b"strh");
            strl_a.extend(u32le(56));
            strl_a.extend(b"auds");
            strl_a.extend(u32le(0));
            strl_a.extend(u32le(0));
            strl_a.extend([0u8; 4]);
            strl_a.extend(u32le(0));
            strl_a.extend(u32le(block)); // scale
            strl_a.extend(u32le(AUDIO_RATE * block)); // rate (bytes a second)
            strl_a.extend(u32le(0));
            strl_a.extend(u32le((self.audio_bytes / block as u64) as u32)); // length in blocks
            strl_a.extend(u32le(AUDIO_RATE * block));
            strl_a.extend(u32le(u32::MAX));
            strl_a.extend(u32le(block)); // sample size
            strl_a.extend([0u8; 8]);
            strl_a.extend(b"strf");
            strl_a.extend(u32le(18));
            strl_a.extend(1u16.to_le_bytes()); // PCM
            strl_a.extend(2u16.to_le_bytes());
            strl_a.extend(u32le(AUDIO_RATE));
            strl_a.extend(u32le(AUDIO_RATE * block));
            strl_a.extend((block as u16).to_le_bytes());
            strl_a.extend(16u16.to_le_bytes());
            strl_a.extend(0u16.to_le_bytes());
        }

        let list = |kind: &[u8; 4], body: &[u8]| -> Vec<u8> {
            let mut out = Vec::new();
            out.extend(b"LIST");
            out.extend(u32le(body.len() as u32 + 4));
            out.extend(kind);
            out.extend(body);
            out
        };
        let mut hdrl = Vec::new();
        hdrl.extend(b"avih");
        hdrl.extend(u32le(56));
        hdrl.extend(u32le(1_000_000 / self.fps.max(1)));
        hdrl.extend(u32le(0));
        hdrl.extend(u32le(0));
        hdrl.extend(u32le(0x10)); // has an index
        hdrl.extend(u32le(self.frames));
        hdrl.extend(u32le(0));
        hdrl.extend(u32le(if self.audio { 2 } else { 1 }));
        hdrl.extend(u32le(1 << 20));
        hdrl.extend(u32le(self.w));
        hdrl.extend(u32le(self.h));
        hdrl.extend([0u8; 16]);
        hdrl.extend(list(b"strl", &strl_v));
        if self.audio {
            hdrl.extend(list(b"strl", &strl_a));
        }
        let hdrl = list(b"hdrl", &hdrl);

        let mut out = Vec::new();
        out.extend(b"RIFF");
        // RIFF size: everything after these 8 bytes (header, movi list, idx1).
        let movi_size = 4 + self.written;
        let idx = if self.index.is_empty() { 0 } else { 8 + 16 * self.index.len() as u64 };
        out.extend(u32le((4 + hdrl.len() as u64 + 8 + movi_size + idx) as u32));
        out.extend(b"AVI ");
        out.extend(hdrl);
        out.extend(b"LIST");
        out.extend(u32le(movi_size as u32));
        out.extend(b"movi");
        out
    }

    pub fn create(path: &Path, w: u32, h: u32, fps: u32, audio: bool) -> std::io::Result<Avi> {
        let file = std::fs::File::create(path)?;
        let mut avi = Avi { file: std::io::BufWriter::new(file), w, h, fps, audio, frames: 0, audio_bytes: 0, movi_at: 0, index: Vec::new(), written: 0 };
        let head = avi.header();
        avi.movi_at = head.len() as u64 - 4;
        avi.file.write_all(&head)?;
        Ok(avi)
    }

    fn chunk(&mut self, id: [u8; 4], data: &[u8]) -> std::io::Result<()> {
        let offset = (4 + self.written) as u32;
        self.file.write_all(&id)?;
        self.file.write_all(&u32le(data.len() as u32))?;
        self.file.write_all(data)?;
        let mut n = 8 + data.len() as u64;
        if data.len() % 2 == 1 {
            self.file.write_all(&[0])?;
            n += 1;
        }
        self.index.push((id, offset, data.len() as u32));
        self.written += n;
        Ok(())
    }

    pub fn picture(&mut self, jpeg: &[u8]) -> std::io::Result<()> {
        self.frames += 1;
        self.chunk(*b"00dc", jpeg)
    }

    pub fn sound(&mut self, pcm: &[u8]) -> std::io::Result<()> {
        if !self.audio || pcm.is_empty() {
            return Ok(());
        }
        self.audio_bytes += pcm.len() as u64;
        self.chunk(*b"01wb", pcm)
    }

    pub fn bytes(&self) -> u64 {
        self.written
    }

    /// Write the index and the final header.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.file.write_all(b"idx1")?;
        self.file.write_all(&u32le(16 * self.index.len() as u32))?;
        for (id, offset, size) in &self.index {
            self.file.write_all(id)?;
            self.file.write_all(&u32le(if id == b"00dc" { 0x10 } else { 0 }))?;
            self.file.write_all(&u32le(*offset))?;
            self.file.write_all(&u32le(*size))?;
        }
        let head = self.header();
        debug_assert_eq!(head.len() as u64 - 4, self.movi_at);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&head)?;
        self.file.flush()
    }
}

/// "2026-11-27 22-41-05" in this PC's time.
fn local_stamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
        // SAFETY: fills a local struct.
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("{secs}")
    }
}

fn file_name(character: &str) -> String {
    let safe: String = character.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '\'' { c } else { '_' }).collect();
    format!("cam {} {}.avi", safe.trim(), local_stamp())
}

fn handle_of(character: &str) -> Option<isize> {
    crate::clipboard::open_clients().into_iter().find(|(name, _)| name == character).map(|(_, h)| h)
}

fn jpeg_of(handle: isize) -> Option<(u32, u32, Vec<u8>)> {
    let pic = crate::capture::window_picture(handle, WIDTH).ok()?;
    if pic.width < 16 || pic.height < 16 {
        return None;
    }
    let mut jpeg = Vec::with_capacity(200 * 1024);
    jpeg_encoder::Encoder::new(&mut jpeg, QUALITY).encode(&pic.bgra, pic.width as u16, pic.height as u16, jpeg_encoder::ColorType::Bgra).ok()?;
    Some((pic.width, pic.height, jpeg))
}

/// Start recording `character`'s client (already recording: nothing changes).
#[tauri::command]
pub fn camrec_start(app: AppHandle, name: String, fps: Option<u32>) -> Result<String, String> {
    let fps = fps.unwrap_or(5).clamp(1, 30);
    let mut all = RECORDINGS.lock().unwrap();
    if all.contains_key(&name) {
        return Ok(format!("Already recording {name}."));
    }
    let handle = handle_of(&name).ok_or_else(|| format!("{name}'s client isn't open."))?;
    let dir = crate::devrec::kind_dir(&app, crate::devrec::Kind::Videos)?;
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new((PathBuf::new(), 0u64, 0u32, false)));
    all.insert(name.clone(), Recording { stop: stop.clone(), started: Instant::now(), fps, state: state.clone() });
    drop(all);

    // Its sound, into a shared buffer the picture loop empties.
    let sound: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let hearing = Arc::new(AtomicBool::new(false));
    sound::start(&name, sound.clone(), hearing.clone(), stop.clone());

    let who = name.clone();
    std::thread::Builder::new()
        .name("camrec".into())
        .spawn(move || {
            record(&who, handle, &dir, fps, &stop, &state, &sound, &hearing);
            RECORDINGS.lock().unwrap().remove(&who);
            stop.store(true, Ordering::SeqCst);
        })
        .map_err(|e| e.to_string())?;
    log::info!("camera: recording {name} at {fps}/s");
    Ok(format!("Recording {name} to {}.", crate::devrec::kind_dir(&app, crate::devrec::Kind::Videos).map(|d| d.display().to_string()).unwrap_or_default()))
}

#[allow(clippy::too_many_arguments)]
fn record(name: &str, mut handle: isize, dir: &Path, fps: u32, stop: &AtomicBool, state: &Mutex<(PathBuf, u64, u32, bool)>, sound: &Mutex<Vec<u8>>, hearing: &AtomicBool) {
    let every = Duration::from_secs_f64(1.0 / fps as f64);
    // Give the sound a moment to start, so the first file knows whether it has sound.
    std::thread::sleep(Duration::from_millis(400));
    let mut avi: Option<(Avi, Instant, u64)> = None; // file, its start, pictures due so far
    let mut last: Option<(u32, u32, Vec<u8>)> = None;
    let mut gone_since: Option<Instant> = None;
    let mut total_before = 0u64;
    while !stop.load(Ordering::SeqCst) {
        let tick = Instant::now();
        let got = jpeg_of(handle);
        match &got {
            Some(_) => gone_since = None,
            None => {
                // Closed or changed window (logged in: same window; restarted: a new one).
                if let Some(h) = handle_of(name) {
                    handle = h;
                }
                let since = *gone_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(20) && handle_of(name).is_none() {
                    log::info!("camera: {name}'s client closed, recording stopped");
                    break;
                }
            }
        }
        if let Some(pic) = got {
            last = Some(pic);
        }
        let Some((w, h, jpeg)) = last.as_ref() else {
            std::thread::sleep(every);
            continue;
        };
        // A new file every 10 minutes, past 900 MB, or when the size changes.
        let new_file = avi.as_ref().is_none_or(|(a, t, _)| t.elapsed() >= PART || a.bytes() >= PART_BYTES || a.w != *w || a.h != *h);
        if new_file {
            if let Some((old, _, _)) = avi.take() {
                total_before += old.bytes();
                if let Err(e) = old.finish() {
                    log::warn!("camera: couldn't finish a file ({e})");
                }
            }
            let path = dir.join(file_name(name));
            let with_sound = hearing.load(Ordering::SeqCst);
            match Avi::create(&path, *w, *h, fps, with_sound) {
                Ok(a) => {
                    let mut s = state.lock().unwrap();
                    s.0 = path;
                    s.2 += 1;
                    s.3 = with_sound;
                    drop(s);
                    sound.lock().unwrap().clear();
                    avi = Some((a, Instant::now(), 0));
                }
                Err(e) => {
                    log::warn!("camera: couldn't write {} ({e})", path.display());
                    break;
                }
            }
        }
        let Some((file, started, due)) = avi.as_mut() else { break };
        // Pictures by the clock (a slow capture repeats the last one, so sound and picture agree).
        let want = (started.elapsed().as_secs_f64() * fps as f64).floor() as u64 + 1;
        let mut ok = true;
        while *due < want && ok {
            ok = file.picture(jpeg).is_ok();
            *due += 1;
        }
        let pcm: Vec<u8> = std::mem::take(&mut *sound.lock().unwrap());
        ok = ok && file.sound(&pcm).is_ok();
        if !ok {
            log::warn!("camera: writing {name}'s video failed (disk full?)");
            break;
        }
        state.lock().unwrap().1 = total_before + file.bytes();
        std::thread::sleep(every.saturating_sub(tick.elapsed()));
    }
    if let Some((file, _, _)) = avi.take() {
        let _ = file.finish();
    }
    log::info!("camera: {name}'s recording saved");
}

#[tauri::command]
pub fn camrec_stop(name: String) -> String {
    match RECORDINGS.lock().unwrap().get(&name) {
        Some(r) => {
            r.stop.store(true, Ordering::SeqCst);
            format!("Stopped recording {name}; the video is in the Videos folder.")
        }
        None => format!("{name} wasn't being recorded."),
    }
}

/// Every recording now: name, seconds, MB, file, parts, whether it has sound.
#[tauri::command]
pub fn camrec_status() -> Vec<serde_json::Value> {
    RECORDINGS
        .lock()
        .unwrap()
        .iter()
        .map(|(name, r)| {
            let s = r.state.lock().unwrap();
            serde_json::json!({
                "name": name, "secs": r.started.elapsed().as_secs(), "fps": r.fps,
                "mb": (s.1 as f64 / 1048576.0 * 10.0).round() / 10.0,
                "file": s.0.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
                "parts": s.2, "sound": s.3,
            })
        })
        .collect()
}

pub fn is_recording(name: &str) -> bool {
    RECORDINGS.lock().unwrap().contains_key(name)
}

/// Stop every recording (the app is closing): their files are finished.
pub fn stop_all() {
    let all: Vec<Arc<AtomicBool>> = RECORDINGS.lock().unwrap().values().map(|r| r.stop.clone()).collect();
    for s in &all {
        s.store(true, Ordering::SeqCst);
    }
    if !all.is_empty() {
        std::thread::sleep(Duration::from_millis(600));
    }
}

/// The client's own sound (Windows 10 2004 or newer), as 16-bit stereo.
mod sound {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[cfg(windows)]
    pub fn start(character: &str, out: Arc<Mutex<Vec<u8>>>, hearing: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
        use std::sync::atomic::Ordering;
        let Some(pid) = crate::clientaudio::eve_pid(character) else { return };
        let who = character.to_string();
        std::thread::spawn(move || {
            let run = || -> Result<(), Box<dyn std::error::Error>> {
                let _ = wasapi::initialize_mta();
                let format = wasapi::WaveFormat::new(16, 16, wasapi::SampleType::Int, super::AUDIO_RATE as usize, 2, None);
                let block = format.get_blockalign() as usize;
                let mut client = wasapi::AudioClient::new_application_loopback_client(pid, true)?;
                client.initialize_client(&format, wasapi::Direction::Capture, wasapi::StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 }, None)?;
                let event = client.set_get_eventhandle()?;
                let capture = client.get_audiocaptureclient()?;
                client.start_stream()?;
                hearing.store(true, Ordering::SeqCst);
                let mut queue = std::collections::VecDeque::new();
                while !stop.load(Ordering::SeqCst) {
                    if capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                        capture.read_from_device_to_deque(&mut queue)?;
                    }
                    let n = queue.len() / block * block;
                    if n > 0 {
                        let mut buf = out.lock().unwrap();
                        buf.extend(queue.drain(..n));
                        // Nobody's taking it (no file yet): keep at most 10 s.
                        let max = super::AUDIO_RATE as usize * block * 10;
                        if buf.len() > max {
                            let cut = (buf.len() - max) / block * block;
                            buf.drain(..cut);
                        }
                    }
                    let _ = event.wait_for_event(200);
                }
                let _ = client.stop_stream();
                Ok(())
            };
            if let Err(e) = run() {
                log::warn!("camera: no sound from {who}'s client ({e}); recording pictures only");
            }
            hearing.store(false, Ordering::SeqCst);
        });
    }

    #[cfg(not(windows))]
    pub fn start(_character: &str, _out: Arc<Mutex<Vec<u8>>>, _hearing: Arc<AtomicBool>, _stop: Arc<AtomicBool>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_playable_avi_layout() {
        let path = std::env::temp_dir().join(format!("dr-camrec-{}.avi", std::process::id()));
        let mut avi = Avi::create(&path, 64, 36, 5, true).unwrap();
        avi.picture(&[0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]).unwrap();
        avi.sound(&[0u8; 400]).unwrap();
        avi.picture(&[0xFF, 0xD8, 4, 0xFF, 0xD9, 0]).unwrap();
        avi.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        assert_eq!(&bytes[8..12], b"AVI ");
        // Two pictures, both streams.
        let avih = bytes.windows(4).position(|w| w == b"avih").unwrap();
        assert_eq!(u32::from_le_bytes(bytes[avih + 24..avih + 28].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[avih + 32..avih + 36].try_into().unwrap()), 2);
        // The movi list's size matches its contents, and the index follows it.
        let movi = bytes.windows(4).position(|w| w == b"movi").unwrap();
        let size = u32::from_le_bytes(bytes[movi - 4..movi].try_into().unwrap()) as usize;
        assert_eq!(&bytes[movi + size..movi + size + 4], b"idx1");
        // The first index entry points at the first picture (offset from 'movi').
        let idx = movi + size;
        let off = u32::from_le_bytes(bytes[idx + 16..idx + 20].try_into().unwrap()) as usize;
        assert_eq!(&bytes[movi + off..movi + off + 4], b"00dc");
    }
}
