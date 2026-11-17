//! The planet scan, remade (ui/piscan.html): the app watches one EVE client's planet view at
//! full size, as fast as EVE draws it, and does all the work natively:
//!
//! - **The sky** (sky.rs): the stars and nebula behind the planet stay put while the camera
//!   turns, so each planet's recorded sky tells which way the camera faces. Every planet gets its
//!   own sky; until it has one, it borrows another planet's in the same system and records its own
//!   as you turn, and the two are compared.
//! - **Scanning** (scan.rs): with the facing known, EVE's scan colours are read onto a
//!   whole-planet map; at 99 % it's done and (with autosave) saved for the corp.
//! - **Shared maps** (seat.rs): skies and bare maps (the planet's own colours) go to SeAT, so
//!   nobody records them twice.
//!
//! Three threads per client: capture (wgc.rs, or the slower PrintWindow picture when Windows'
//! capture can't start), processing (one picture after another, skipping any that pile up), and
//! the preview (a JPEG for the page, about 30 a second). The page polls for the preview and a
//! status (one call) and sends commands. Nothing clicks or types in EVE.

pub mod base;
pub mod geom;
pub mod rings;
pub mod scan;
pub mod seat;
pub mod sky;
#[cfg(windows)]
mod wgc;

use base::Base;
use geom::{apart, pose_q, Disc};
use scan::{ListSpot, Map};
use serde_json::json;
use rings::{PoleRings, RingLearner, Rings};
use sky::{BootFrame, Pic, Sky, SkyMask, UiMask, ViewBuffers, DEG};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// EVE's lens: focal length 750 px at 1280 wide (measured from the stars' own motion).
pub fn lens_for(width: usize) -> f64 {
    750.0 * width as f64 / 1280.0
}

/// When a resource scan counts as done (Tsa: 99 % of the planet).
const DONE_AT: f64 = 0.99;

/// A gas giant: under this share of the picture left as sky beside its rings, the sky gives no
/// facing (with so little, a wrong one is likelier than none).
const MIN_SKY: f64 = 0.25;

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub bgra: Vec<u8>,
    pub n: u64,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct PlanetInfo {
    pub id: u64,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    /// When SeAT's copy of this planet's sky / bare map was saved (seconds), if there is one.
    /// Any number (the page may note a save itself, to the millisecond).
    #[serde(default)]
    pub sky_at: Option<f64>,
    #[serde(default)]
    pub bare_at: Option<f64>,
    #[serde(default)]
    pub sky_coverage: Option<f64>,
}

struct Pose {
    yaw: f64,
    pitch: f64,
    mirrored: bool,
    /// Which planet's sky placed it (None: the buildings, until the sky takes over).
    sky_of: Option<u64>,
    /// Placed on this picture (false: carried over from an earlier one).
    placed: bool,
    coarse: f64,
    stars: f64,
    level: usize,
    /// The facing one picture earlier (for predicting the next).
    before: Option<(f64, f64)>,
    /// Placed on the planet map itself (too little sky past a gas giant's rings), not the sky.
    by_planet: bool,
}

struct Run {
    /// "" = the bare planet (EVE on No filter).
    resource: String,
    /// EVE's list row this resource sits in (1 … 5; 0 = No filter), to check against EVE.
    row: Option<usize>,
    map: Map,
    started: Instant,
    done: bool,
    paused: Option<&'static str>,
    painted_frames: u64,
}

/// Starting a sky from nothing: the pictures watched (about 4 a second while the planet turns
/// sideways), then the search in its own thread, then catching up with the pictures since.
struct BootRun {
    frames: Vec<Arc<Frame>>,
    points: Vec<BootFrame>,
    at: Instant,
    job: Option<std::thread::JoinHandle<Option<sky::Boot>>>,
    catch: Vec<(Arc<Frame>, BootFrame)>,
}

impl BootRun {
    fn new() -> BootRun {
        BootRun { frames: Vec::new(), points: Vec::new(), at: Instant::now(), job: None, catch: Vec::new() }
    }
}

/// Pictures a start from nothing watches before it works it out (plus the first).
const BOOT_PICTURES: usize = 12;

/// Looks at EVE's windows before a sky is started (so they aren't recorded as sky).
const BOOT_WINDOW_LOOKS: u32 = 8;

#[derive(Default)]
struct Rate {
    count: u32,
    since: Option<Instant>,
    per_second: f64,
}

impl Rate {
    fn tick(&mut self) {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        self.count += 1;
        let secs = now.duration_since(since).as_secs_f64();
        if secs >= 1.0 {
            self.per_second = self.count as f64 / secs;
            self.count = 0;
            self.since = Some(now);
        }
    }
}

pub enum Job {
    SaveScan { planet: u64, planet_name: String, resource: String, character_id: Option<u64>, cells: Vec<u8>, coverage: f64 },
    SaveMap { planet: u64, kind: &'static str, bytes: Vec<u8>, coverage: f64, meta: String, character_id: Option<u64> },
    DeleteMap { planet: u64, kind: &'static str },
    WriteFile { path: PathBuf, bytes: Vec<u8> },
}

struct Session {
    dir: Option<PathBuf>,
    client: Option<String>,
    character_id: Option<u64>,
    size: (usize, usize),
    frames: u64,
    planet: Option<PlanetInfo>,
    system: Vec<PlanetInfo>,
    edge_r: Option<f64>,
    edge_auto: bool,
    /// Edges found lately while looking for it (it's taken once a few agree), and since when
    /// none was found at the zoomed-out size.
    edge_seen: Vec<f64>,
    edge_missing: Option<Instant>,
    auto_zoom: bool,
    pose: Option<Pose>,
    skies: HashMap<u64, Sky>,
    /// Own sky changed since saved: (on this PC, for the corp, coverage when last sent).
    sky_dirty: HashMap<u64, (bool, bool, f64, Instant)>,
    borrowed: Option<u64>,
    bares: HashMap<u64, Map>,
    ui: Option<UiMask>,
    ui_version: u64,
    ui_saved: Option<Instant>,
    mask: Option<((i64, i64, i64, u64, i64), SkyMask)>,
    list: Option<ListSpot>,
    list_guessed: bool,
    picked: Option<usize>,
    run: Option<Run>,
    autosave: bool,
    auto_planet: bool,
    events: Vec<serde_json::Value>,
    event_id: u64,
    proc: Rate,
    step_ms: f64,
    method: &'static str,
    trouble: Option<String>,
    last_find: Option<Instant>,
    last_find_note: Option<serde_json::Value>,
    last_paint: Option<(f64, f64, Instant)>,
    rim_ok: (bool, Option<Instant>),
    compare: Option<(f64, u64, Instant)>,
    compare_at: Option<Instant>,
    lost_since: Option<Instant>,
    fast_turn: bool,
    identify_at: Option<Instant>,
    preview_width: usize,
    /// The picture the pilot is pointing out buildings on (held still on the page).
    held: Option<Arc<Frame>>,
    /// Record the sky from the held picture at the next step (set by Lock on from buildings).
    paint_held: bool,
    /// This planet's base map (its own colours in detail) and how its recording goes.
    base: Option<Base>,
    base_on: bool,
    base_last: Option<(f64, f64)>,
    base_cov: (f64, Option<Instant>),
    /// Changed since saved: (on this PC, for the corp, coverage when last sent, when last kept).
    base_dirty: (bool, bool, f64, Instant),
    base_done: bool,
    base_paused: Option<&'static str>,
    /// A sky being started from nothing, and what the last try said.
    boot: Option<BootRun>,
    boot_note: Option<String>,
    /// The pilot asked for this planet's own sky from nothing (not the borrowed one).
    new_sky: bool,
    /// Every planet starts its own sky from nothing (another planet's sky is only compared with,
    /// never followed: each planet has its own longitude 0).
    own_sky_each: bool,
    /// This planet's sky, how much of each level is recorded (worked out once a second).
    sky_cov: ([f64; 3], Option<Instant>),
    /// Own sky against another planet's: the longitude shift between their frames once found,
    /// then each look's tilt (pitch difference; 0 when the skies are the same) and shift.
    compare_shift: Option<f64>,
    compare_last: Option<(f64, f64, u64)>,
    base_full: bool,
    /// Watching only after the pilot pressed Start with EVE zoomed all the way out: before that
    /// nothing is learnt or painted, so EVE's opening zoom can't get into anything.
    started: bool,
    /// Since when the planet's edge has been missing (after 3 s: Start again).
    rim_lost: Option<Instant>,
    /// A gas giant's rings: the votes, the cut in use (and a number that changes with it),
    /// whether this planet's sky was started with the cut (else it starts again once they're
    /// measured), when the cut was last looked at, and the heights of sky that can be seen past it.
    ring_learner: Option<RingLearner>,
    /// A gas giant's rings not measured yet: from over its top, then its bottom (before its sky).
    ring_pole: Option<RingPole>,
    ring_pole_at: Option<Instant>,
    ring_cut: Rings,
    ring_version: u64,
    ring_clean: bool,
    ring_at: Option<Instant>,
    ring_saved: Option<Instant>,
    reach: Option<[bool; 181]>,
    /// EVE's windows as loaded (learnt on planets before): what the ring measuring leaves out
    /// (on a gas giant the live mask also takes in rings while the planet turns sideways).
    ui_trusted: Option<UiMask>,
    /// For the whole-sky search on a gas giant (a picture without the ring cut).
    find_buf: ViewBuffers,
    /// Recording the pictures for testing.
    recorder: Option<Recorder>,
}

/// Measuring a gas giant's rings from over its poles: the top first, then the bottom (with what
/// the top found).
enum RingPole {
    Top(PoleRings),
    Bottom(Rings, PoleRings),
}

/// Recording for testing: a picture every 200 ms (full size, JPEG) with a note of what the app
/// made of it, to one file in the recordings folder; at most 3 minutes. Written by its own thread.
struct Recorder {
    tx: std::sync::mpsc::Sender<(Arc<Frame>, String)>,
    started: Instant,
    last: Option<Instant>,
    frames: u32,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            dir: None,
            client: None,
            character_id: None,
            size: (0, 0),
            frames: 0,
            planet: None,
            system: Vec::new(),
            edge_r: None,
            edge_auto: true,
            edge_seen: Vec::new(),
            edge_missing: None,
            auto_zoom: false,
            pose: None,
            skies: HashMap::new(),
            sky_dirty: HashMap::new(),
            borrowed: None,
            bares: HashMap::new(),
            ui: None,
            ui_version: 0,
            ui_saved: None,
            mask: None,
            list: None,
            list_guessed: true,
            picked: None,
            run: None,
            autosave: true,
            auto_planet: false,
            events: Vec::new(),
            event_id: 0,
            proc: Rate::default(),
            step_ms: 0.0,
            method: "",
            trouble: None,
            last_find: None,
            last_find_note: None,
            last_paint: None,
            rim_ok: (false, None),
            compare: None,
            compare_at: None,
            lost_since: None,
            fast_turn: false,
            identify_at: None,
            preview_width: 1280,
            held: None,
            paint_held: false,
            base: None,
            base_on: true,
            base_last: None,
            base_cov: (0.0, None),
            base_dirty: (false, false, 0.0, Instant::now()),
            base_done: false,
            base_paused: None,
            boot: None,
            boot_note: None,
            new_sky: false,
            own_sky_each: true,
            sky_cov: ([0.0; 3], None),
            compare_shift: None,
            compare_last: None,
            base_full: false,
            started: false,
            rim_lost: None,
            ring_learner: None,
            ring_pole: None,
            ring_pole_at: None,
            ring_cut: Rings::none(),
            ring_version: 0,
            ring_clean: false,
            ring_at: None,
            ring_saved: None,
            reach: None,
            ui_trusted: None,
            find_buf: ViewBuffers::default(),
            recorder: None,
        }
    }
}

pub struct Inner {
    state: Mutex<Session>,
    frame: Mutex<Option<Arc<Frame>>>,
    frame_cv: Condvar,
    preview: Mutex<(u64, Arc<Vec<u8>>)>,
    preview_cv: Condvar,
    /// The last few pictures sent to the page, by preview number (to hold the one the pilot sees).
    shown: Mutex<std::collections::VecDeque<(u64, Arc<Frame>)>>,
    status: Mutex<Arc<String>>,
    gen: AtomicU64,
    cap_fps: AtomicU64,
    preview_width: AtomicUsize,
    jobs: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Job>>>,
}

#[derive(Clone)]
pub struct Engine(Arc<Inner>);

impl Default for Engine {
    fn default() -> Self {
        Engine(Arc::new(Inner {
            state: Mutex::new(Session::default()),
            frame: Mutex::new(None),
            frame_cv: Condvar::new(),
            preview: Mutex::new((0, Arc::new(Vec::new()))),
            preview_cv: Condvar::new(),
            shown: Mutex::new(std::collections::VecDeque::new()),
            status: Mutex::new(Arc::new("{}".into())),
            gen: AtomicU64::new(0),
            cap_fps: AtomicU64::new(0),
            preview_width: AtomicUsize::new(1280),
            jobs: Mutex::new(None),
        }))
    }
}

/// This scanner's own files for a planet's maps (older scanners' files are left alone).
fn local_name(planet: u64, kind: &str) -> String {
    match kind {
        "sky" => format!("{planet}-sky3.bin"),
        "bare" => format!("{planet}-base1.bin"),
        other => format!("{planet}-{other}.bin"),
    }
}

fn safe_name(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

impl Session {
    fn lens(&self) -> f64 {
        lens_for(self.size.0)
    }
    fn disc(&self) -> Option<Disc> {
        let r = self.edge_r?;
        Some(Disc::with_lens(self.size.0 as f64 / 2.0, self.size.1 as f64 / 2.0, r, self.lens()))
    }
    fn event(&mut self, value: serde_json::Value) {
        self.event_id += 1;
        let mut value = value;
        value["id"] = json!(self.event_id);
        self.events.push(value);
        if self.events.len() > 20 {
            self.events.remove(0);
        }
    }
    fn planet_id(&self) -> Option<u64> {
        self.planet.as_ref().map(|p| p.id)
    }
    fn planet_name(&self, id: u64) -> String {
        self.system.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| format!("planet {id}"))
    }
    fn is_gas(&self) -> bool {
        self.planet.as_ref().is_some_and(|p| p.kind == "gas")
    }
    /// A gas giant's measured rings, to leave out of the sky (none until measured).
    fn ring_cut_now(&self) -> Option<Rings> {
        (self.is_gas() && self.ring_cut.any()).then_some(self.ring_cut)
    }
    /// A new cut: the masks are made again and what sky can be seen past it worked out.
    fn set_ring_cut(&mut self, cut: Rings) {
        self.ring_cut = cut;
        self.ring_version += 1;
        self.mask = None;
        self.reach = match (cut.any(), self.disc()) {
            (true, Some(disc)) => Some(rings::reachable(&cut, &disc, self.size.0, self.size.1, self.lens())),
            _ => None,
        };
        self.sky_cov.1 = None;
    }
    /// This planet's rings from the PC (kept between sessions): the cut measured over its poles,
    /// and what turning it added since. A gas giant without one is measured first.
    fn load_rings(&mut self) {
        let read = |kind: &str| self.planet_id().and_then(|pid| self.map_path(pid, kind)).and_then(|p| std::fs::read(p).ok());
        let measured = read("rings3").and_then(|b| rings::cut_from_bytes(&b));
        let learner = read("rings2").and_then(|b| RingLearner::from_bytes(&b));
        let gas = self.is_gas();
        let mut cut = measured.unwrap_or_default();
        if let (Some(_), Some(l)) = (measured, learner.as_ref().filter(|l| l.measured())) {
            cut = cut.union(&l.rings());
        }
        self.ring_learner = learner;
        self.ring_clean = !gas || measured.is_some();
        self.ring_pole = (gas && measured.is_none()).then(|| RingPole::Top(PoleRings::new()));
        self.ring_pole_at = None;
        self.ring_at = None;
        self.set_ring_cut(cut);
    }
    /// A gas giant's rings measured (from over both poles): kept on the PC and cut out from now
    /// on. Its sky so far was recorded without knowing them, so it starts again; EVE's windows go
    /// back to the kept ones (over a pole, turning, the rings looked like windows).
    fn finish_ring_measure(&mut self, cut: Rings, jobs: &mut Vec<Job>) {
        self.ring_pole = None;
        self.ring_clean = true;
        self.ring_learner = Some(RingLearner::new());
        self.set_ring_cut(cut);
        if let Some(pid) = self.planet_id() {
            if let Some(path) = self.map_path(pid, "rings3") {
                jobs.push(Job::WriteFile { path, bytes: rings::cut_to_bytes(&cut) });
            }
            if self.skies.remove(&pid).is_some() {
                self.sky_dirty.remove(&pid);
                if let Some(path) = self.map_path(pid, "sky") {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        self.reload_windows();
        self.pose = None;
        self.boot = None;
        self.boot_note = None;
        self.lost_since = None;
    }
    /// What the recording keeps with each picture.
    fn record(&mut self, f: &Arc<Frame>) {
        let (expired, due, ms) = match self.recorder.as_ref() {
            None => return,
            Some(r) => (r.started.elapsed() > Duration::from_secs(180), r.last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)), r.started.elapsed().as_millis() as u64),
        };
        if expired {
            self.recorder = None;
            self.event(json!({ "kind": "warn", "text": "Recording stopped after 3 minutes." }));
            return;
        }
        if !due {
            return;
        }
        let note = json!({
            "ms": ms, "w": f.w, "h": f.h, "planet": self.planet.as_ref().map(|p| json!({ "id": p.id, "name": p.name, "type": p.kind })),
            "started": self.started, "edge": self.edge_r, "rim_ok": self.rim_ok.0, "lens": self.lens(),
            "pose": self.pose.as_ref().map(|p| json!({ "yaw": p.yaw / DEG, "pitch": p.pitch / DEG, "placed": p.placed, "by_planet": p.by_planet, "sky_of": p.sky_of })),
            "rings": self.ring_cut.bands(), "ring_clean": self.ring_clean, "picked": self.picked,
        })
        .to_string();
        let Some(rec) = self.recorder.as_mut() else { return };
        rec.last = Some(Instant::now());
        rec.frames += 1;
        if rec.tx.send((f.clone(), note)).is_err() {
            self.recorder = None;
        }
    }
    fn ui_path(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("windows-{}-{}x{}.bin", safe_name(self.client.as_deref()?), self.size.0, self.size.1)))
    }
    fn list_path(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("list-{}-{}x{}.txt", safe_name(self.client.as_deref()?), self.size.0, self.size.1)))
    }
    fn map_path(&self, planet: u64, kind: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(local_name(planet, kind)))
    }

    /// A new picture size (or client): the edge scales along, the windows mask and the list spot
    /// are this client's at this size.
    fn resized(&mut self, w: usize, h: usize) {
        if let (Some(r), true) = (self.edge_r, self.size.0 > 0) {
            self.edge_r = Some(r * w as f64 / self.size.0 as f64);
        }
        self.size = (w, h);
        self.mask = None;
        // EVE's windows rarely cover more than a third of the picture: a saved mask that does is wrong.
        self.reload_windows();
        let saved = self.list_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| {
            let v: Vec<f64> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| ListSpot { x: v[0], y0: v[1], y5: v[2] })
        });
        self.list_guessed = saved.is_none();
        // EVE's planet panel at 90 % UI scale on a 1918 × 1024 window (Tsa's): No filter at y 216,
        // the fifth resource at 342, names round x 140. Scaled by the window's height.
        self.list = saved.or(Some(ListSpot { x: 140.0 * h as f64 / 1024.0, y0: 216.0 * h as f64 / 1024.0, y5: 342.0 * h as f64 / 1024.0 }));
    }

    /// EVE's windows from the PC (learnt on planets before; a gas giant's are never kept, as
    /// the rings would be in them), as the live mask and as the trusted copy.
    fn reload_windows(&mut self) {
        let (w, h) = self.size;
        if w == 0 {
            return;
        }
        let saved = self.ui_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| UiMask::from_bytes(&b, w, h)).filter(|m| m.share() < 0.35);
        self.ui_trusted = saved.clone().filter(|m| m.ready());
        self.ui = saved.or_else(|| Some(UiMask::new(w, h)));
        self.ui_version += 1;
        self.mask = None;
    }

    /// The sky to place this picture with: the planet's own (if it has any), else the borrowed one.
    fn skies_to_try(&self) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(id) = self.planet_id() {
            if self.skies.get(&id).is_some_and(|s| s.coverage() > 0.01) {
                out.push(id);
            }
        }
        if self.own_sky_each {
            return out;
        }
        if let Some(b) = self.borrowed {
            if self.skies.contains_key(&b) && !out.contains(&b) {
                out.push(b);
            }
        }
        out
    }

    /// The least-recorded level of this planet's sky (cached, once a second).
    fn sky_done(&self) -> f64 {
        self.sky_cov.0.iter().cloned().fold(1.0, f64::min)
    }

    fn step(&mut self, f: &Arc<Frame>, buf: &mut ViewBuffers, jobs: &mut Vec<Job>) {
        let start = Instant::now();
        if (f.w, f.h) != self.size {
            self.resized(f.w, f.h);
        }
        self.frames += 1;
        self.proc.tick();
        self.record(f);
        // Nothing until Start (pressed with EVE zoomed all the way out on this planet).
        if !self.started || self.planet.is_none() {
            self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
            return;
        }
        let pic = Pic { w: f.w, h: f.h, bgra: &f.bgra };
        let disc = self.disc();
        let gas = self.is_gas();

        // EVE's windows (every sixth picture) and which row of EVE's resource list is lit. On a gas
        // giant they're not kept: turning sideways, its rings stay put on screen like a window.
        if self.frames % 6 == 0 {
            let path = self.ui_path();
            if let Some(ui) = self.ui.as_mut() {
                if ui.learn(&pic, disc.as_ref()) {
                    self.ui_version += 1;
                    if !gas && ui.ready() && self.ui_saved.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                        self.ui_saved = Some(Instant::now());
                        if let Some(path) = path {
                            jobs.push(Job::WriteFile { path, bytes: ui.to_bytes() });
                        }
                    }
                }
            }
        }
        if self.frames % 3 == 0 {
            self.picked = self.list.as_ref().and_then(|s| scan::picked_row(&pic, s));
        }

        // The edge: zoomed all the way out every planet is the same size (0.26 lens lengths), so
        // only an edge near that is taken, and only once four looks in a row agree (EVE's zoom
        // when a planet opens passes through other sizes). Then it's kept; Follow zoom follows it.
        if self.edge_r.is_none() && self.edge_auto && self.frames % 8 == 1 {
            let expect = sky::ZOOMED_OUT_RIM * self.lens();
            match sky::rim_radius(&pic, Some(expect), self.lens(), None) {
                Some(r) => {
                    self.edge_missing = None;
                    self.edge_seen.push(r);
                    if self.edge_seen.len() > 4 {
                        self.edge_seen.remove(0);
                    }
                    let (lo, hi) = self.edge_seen.iter().fold((f64::MAX, f64::MIN), |(a, b), &r| (a.min(r), b.max(r)));
                    if self.edge_seen.len() == 4 && hi - lo < expect * 0.012 {
                        let mut v = self.edge_seen.clone();
                        v.sort_by(f64::total_cmp);
                        self.edge_r = Some((v[1] + v[2]) / 2.0);
                        self.edge_seen.clear();
                        self.mask = None;
                    }
                }
                None => {
                    self.edge_seen.clear();
                    self.edge_missing.get_or_insert_with(Instant::now);
                }
            }
        } else if self.auto_zoom && self.frames % 15 == 0 {
            if let Some(r0) = self.edge_r {
                if let Some(r) = sky::rim_radius(&pic, Some(r0), self.lens(), None) {
                    if (r - r0).abs() > r0 * 0.015 {
                        self.edge_r = Some(r);
                        self.mask = None;
                    }
                }
            }
        }
        let Some(disc) = self.disc() else {
            self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
            return;
        };
        let lens = self.lens();
        // Zoomed all the way out still (the edge where it was)? Gone for 3 s: Start again.
        if self.rim_ok.1.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            self.rim_ok = (sky::rim_still_there(&pic, &disc), Some(Instant::now()));
        }
        if self.rim_ok.0 {
            self.rim_lost = None;
        } else if self.rim_lost.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(3) {
            self.started = false;
            self.rim_lost = None;
            self.boot = None;
            self.event(json!({ "kind": "warn", "text": "The planet's edge isn't where it was (zoomed in, or another planet?): zoom all the way out in EVE, then press Start." }));
            self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
            return;
        }

        // ---- a gas giant's rings, from over its poles, before its sky (nothing else meanwhile) ----
        if gas && self.ring_pole.is_some() {
            if self.ring_pole_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(180)) {
                self.ring_pole_at = Some(Instant::now());
                match self.ring_pole.as_mut() {
                    Some(RingPole::Top(p)) | Some(RingPole::Bottom(_, p)) => {
                        p.learn(&pic, &disc, lens, self.ui_trusted.as_ref());
                    }
                    None => {}
                }
                let bands = |r: &Rings| r.bands().iter().map(|(a, b)| format!("{a:.2}–{b:.2}")).collect::<Vec<_>>().join(", ");
                match self.ring_pole.take() {
                    Some(RingPole::Top(p)) if p.measured() => {
                        let top = p.rings();
                        let seen = if top.any() { format!("rings at {} planet radii", bands(&top)) } else { "no rings".to_string() };
                        self.event(json!({ "kind": "locked", "text": format!("From the top: {seen}. Now tilt the camera all the way down (looking up at the planet's bottom) and turn the planet all the way round.") }));
                        self.ring_pole = Some(RingPole::Bottom(top, PoleRings::new()));
                    }
                    Some(RingPole::Bottom(top, p)) if p.measured() => {
                        let cut = top.union(&p.rings());
                        self.finish_ring_measure(cut, jobs);
                        let text = if cut.any() {
                            format!("Rings measured: {} planet radii, cut out of the sky. Now tilt back until the rings look nearly flat (a thin oval), then turn the planet slowly sideways: the sky starts.", bands(&cut))
                        } else {
                            "No rings on this planet. Tilt back to the middle and turn the planet slowly sideways: the sky starts.".to_string()
                        };
                        self.event(json!({ "kind": "locked", "text": text }));
                    }
                    other => self.ring_pole = other,
                }
            }
            self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
            return;
        }

        // Which pixels are sky (rebuilt when the edge, EVE's windows, a gas giant's ring cut or,
        // with one, the pitch by half a degree change).
        let rings = self.ring_cut_now();
        let ring_pitch = self.pose.as_ref().map(|p| p.pitch).unwrap_or(0.0);
        let key = ((disc.r * 4.0) as i64, f.w as i64, f.h as i64, self.ui_version, if rings.is_some() && self.pose.is_some() { ((ring_pitch / (0.5 * DEG)).round() as i64) << 24 | self.ring_version as i64 } else { i64::MIN });
        if self.mask.as_ref().is_none_or(|(k, _)| *k != key) {
            let ui = self.ui.as_ref().filter(|u| u.ready());
            let ring_now = rings.filter(|_| self.pose.is_some()).map(|rg| (rg, sky::axis_for_pitch(ring_pitch), disc.d));
            self.mask = Some((key, SkyMask::new(f.w, f.h, Some(&disc), lens, ui, ring_now)));
        }
        let mask = &self.mask.as_ref().unwrap().1;
        let sky_share = mask.share();

        // ---- Lock on from buildings: the sky is recorded from the very picture they were pointed out on ----
        let mut held_painted = false;
        if self.paint_held && self.ui.as_ref().is_some_and(|u| u.ready()) {
            if let (Some(hf), Some(pid), Some(pose)) = (self.held.clone(), self.planet.as_ref().map(|p| p.id), self.pose.as_ref()) {
                if (hf.w, hf.h) == (f.w, f.h) {
                    let hp = Pic { w: hf.w, h: hf.h, bgra: &hf.bgra };
                    let hv = sky::view(buf, &hp, mask, lens);
                    let q = pose_q(pose.yaw, pose.pitch);
                    let sky = self.skies.entry(pid).or_insert_with(|| Sky::new(pose.mirrored));
                    if sky.mirrored == pose.mirrored {
                        sky.paint(&hv, &q, &[sky::COARSE, sky::MEDIUM, sky::FINE]);
                        held_painted = true;
                    }
                }
            }
            self.paint_held = false;
            self.held = None;
        }
        if held_painted {
            let pid = self.planet.as_ref().map(|p| p.id).unwrap_or(0);
            if let Some(p) = self.pose.as_mut() {
                p.sky_of = Some(pid);
                p.placed = true;
                self.last_paint = Some((p.yaw, p.pitch, Instant::now()));
            }
            let d = self.sky_dirty.entry(pid).or_insert((false, false, 0.0, Instant::now()));
            d.0 = true;
            d.1 = true;
        }
        let view = sky::view(buf, &pic, mask, lens);

        // ---- the facing ----
        let mut locked_said: Option<String> = None;
        let tries = self.skies_to_try();
        // A gas giant with too little sky left beside its rings: none of it is trusted.
        let sky_usable = !gas || rings.is_none() || sky_share >= MIN_SKY;
        if let Some(pose) = self.pose.as_mut() {
            pose.placed = false;
            pose.by_planet = false;
            let predicted = match pose.before {
                Some(b) => (2.0 * pose.yaw - b.0, 2.0 * pose.pitch - b.1),
                None => (pose.yaw, pose.pitch),
            };
            let now = (pose.yaw, pose.pitch);
            for id in tries.iter().filter(|_| sky_usable) {
                let Some(sky) = self.skies.get_mut(id) else { continue };
                if sky.mirrored != pose.mirrored {
                    continue;
                }
                let got = sky::follow_fast(&view, sky, predicted, None).ok_or(0.0).or_else(|_| sky::follow(&view, sky, now, None));
                if let Ok(fo) = got {
                    let turned = apart(now, (fo.yaw, fo.pitch));
                    self.fast_turn = turned > 4.0 * DEG;
                    pose.before = Some(now);
                    pose.yaw = fo.yaw;
                    pose.pitch = fo.pitch;
                    pose.placed = true;
                    pose.coarse = if fo.coarse.is_nan() { pose.coarse } else { fo.coarse };
                    pose.stars = fo.stars;
                    pose.level = fo.level;
                    pose.sky_of = Some(*id);
                    break;
                }
            }
            // Past a gas giant's broad rings (too little sky): placed on its own planet map, from
            // the last facing, where the map is painted enough round it.
            if !pose.placed && gas && rings.is_some() && self.ring_clean && self.base_cov.0 >= 0.3 {
                if let Some(base) = self.base.as_mut() {
                    if let Some(pl) = base.place(&pic, &disc, predicted, 1.5 * DEG).filter(|p| p.score >= 0.5) {
                        pose.before = Some(now);
                        pose.yaw = pl.yaw;
                        pose.pitch = pl.pitch;
                        pose.placed = true;
                        pose.by_planet = true;
                        pose.coarse = pl.score;
                        pose.stars = 0.0;
                    }
                }
            }
            if pose.placed {
                self.lost_since = None;
            } else if self.lost_since.is_none() {
                self.lost_since = Some(Instant::now());
            }
        }
        let lost_long = self.lost_since.is_some_and(|t| t.elapsed() > Duration::from_millis(400));
        if (self.pose.is_none() || lost_long) && !tries.is_empty() && self.last_find.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            self.last_find = Some(Instant::now());
            // A gas giant with its rings measured: the whole picture, the rings left out at each
            // pitch tried, and a find kept only where enough sky is left beside them.
            let free = rings.map(|_| SkyMask::new(f.w, f.h, Some(&disc), lens, self.ui.as_ref().filter(|u| u.ready()), None));
            let free_view = free.as_ref().map(|m| sky::view(&mut self.find_buf, &pic, m, lens));
            let mut best: Option<(sky::Find, u64)> = None;
            for id in &tries {
                let Some(sky) = self.skies.get_mut(id) else { continue };
                let found = match (&free_view, rings) {
                    (Some(v), Some(rg)) => sky::find(v, sky, Some((rg, disc.d))),
                    _ => sky::find(&view, sky, None),
                };
                if let Some(found) = found {
                    if best.as_ref().is_none_or(|b| found.coarse + found.stars > b.0.coarse + b.0.stars) {
                        best = Some((found, *id));
                    }
                }
            }
            let enough = |f: &sky::Find| match rings {
                Some(rg) => SkyMask::new(pic.w, pic.h, Some(&disc), lens, self.ui.as_ref().filter(|u| u.ready()), Some((rg, sky::axis_for_pitch(f.pitch), disc.d))).share() >= MIN_SKY,
                None => true,
            };
            self.last_find_note = best.as_ref().map(|(f, id)| json!({ "coarse": f.coarse, "stars": f.stars, "margin": f.margin, "sure": f.sure(), "sky": id }));
            if let Some((found, id)) = best.filter(|b| b.0.sure() && enough(&b.0)) {
                let mirrored = self.skies.get(&id).map(|s| s.mirrored).unwrap_or(true);
                let was_none = self.pose.is_none();
                self.pose = Some(Pose { yaw: found.yaw, pitch: found.pitch, mirrored, sky_of: Some(id), placed: true, coarse: found.coarse, stars: found.stars, level: found.level, before: None, by_planet: false });
                self.lost_since = None;
                if was_none {
                    let whose = if Some(id) == self.planet_id() { "this planet's own sky".to_string() } else { format!("{}'s sky", self.planet_name(id)) };
                    locked_said = Some(format!("Locked on by {whose}."));
                }
            }
        }

        // ---- no sky in the system yet (or a fresh one asked for): start one from nothing ----
        let mut boot_said: Option<String> = None;
        let mut base_restarted = false;
        let want_boot = self.planet.is_some() && self.pose.is_none() && (tries.is_empty() || self.new_sky);
        if !want_boot {
            self.boot = None;
        } else if self.ui.as_ref().is_some_and(|u| u.views >= BOOT_WINDOW_LOOKS) && self.rim_ok.0 {
            let pid = self.planet.as_ref().map(|p| p.id).unwrap_or(0);
            let b = self.boot.get_or_insert_with(BootRun::new);
            if b.job.as_ref().is_some_and(|j| j.is_finished()) {
                let result = b.job.take().and_then(|j| j.join().ok()).flatten();
                let sure = result.as_ref().is_some_and(|r| r.sure());
                match result {
                    Some(r) if sure => {
                        // The sky from the full pictures: the first at yaw 0, the others where they fitted.
                        let mut fresh = Sky::new(false);
                        let mut tmp = ViewBuffers::default();
                        let mut at = (0.0, r.pitch);
                        let all = [sky::COARSE, sky::MEDIUM, sky::FINE];
                        // A gas giant's rings, once measured, are left out at each picture's pitch.
                        let ring_mask = |pitch: f64| rings.map(|rg| SkyMask::new(f.w, f.h, Some(&disc), lens, self.ui.as_ref().filter(|u| u.ready()), Some((rg, sky::axis_for_pitch(pitch), disc.d))));
                        let mut placed_frames: Vec<(usize, (f64, f64))> = Vec::new();
                        for (k, fr) in b.frames.iter().enumerate() {
                            let pose = if k == 0 { Some((0.0, r.pitch)) } else { r.later.get(k - 1).copied().flatten().filter(|x| x.2 >= 0.5).map(|x| (x.0, x.1)) };
                            let Some(p) = pose else { continue };
                            if (fr.w, fr.h) != (f.w, f.h) {
                                continue;
                            }
                            let rm = ring_mask(p.1);
                            let v = sky::view(&mut tmp, &Pic { w: fr.w, h: fr.h, bgra: &fr.bgra }, rm.as_ref().unwrap_or(mask), lens);
                            fresh.paint(&v, &pose_q(p.0, p.1), &all);
                            placed_frames.push((k, p));
                            at = p;
                        }
                        // The pictures that came in meanwhile: each fitted to the sky so far, then added.
                        for (fr, bf) in &b.catch {
                            if (fr.w, fr.h) != (f.w, f.h) {
                                continue;
                            }
                            let c = sky::align_box(&bf.match_coarse, &mut fresh, sky::COARSE, at, (25.0 * DEG, 10.0 * DEG), 2.5 * DEG, 0.25, None).filter(|c| c.score >= 0.35);
                            let m = c.and_then(|c| sky::align_box(&bf.match_medium, &mut fresh, sky::MEDIUM, (c.yaw, c.pitch), (1.5 * DEG, 1.5 * DEG), 0.5 * DEG, 0.25, None)).filter(|m| m.score >= 0.45);
                            if let Some(m) = m {
                                at = (m.yaw, m.pitch);
                                let rm = ring_mask(m.pitch);
                                let v = sky::view(&mut tmp, &Pic { w: fr.w, h: fr.h, bgra: &fr.bgra }, rm.as_ref().unwrap_or(mask), lens);
                                fresh.paint(&v, &pose_q(m.yaw, m.pitch), &all);
                            }
                        }
                        let cov = fresh.coverage();
                        self.skies.insert(pid, fresh);
                        // A gas giant (rings measured over its poles): the start's pictures, turning
                        // sideways, add to what's known of them.
                        if gas {
                            let learner = self.ring_learner.get_or_insert_with(RingLearner::new);
                            for (k, p) in &placed_frames {
                                let fr = &b.frames[*k];
                                learner.learn(&Pic { w: fr.w, h: fr.h, bgra: &fr.bgra }, &disc, lens, p.0, p.1, self.ui_trusted.as_ref());
                            }
                        }
                        // A fresh sky puts longitude 0 somewhere new: a map made on an older sky no longer fits.
                        if self.base.as_ref().is_some_and(|b| b.version > 0) {
                            self.base = None;
                            self.base_last = None;
                            self.base_cov = (0.0, None);
                            self.base_done = false;
                            self.base_dirty = (false, false, 0.0, Instant::now());
                            base_restarted = true;
                        }
                        self.pose = Some(Pose { yaw: at.0, pitch: at.1, mirrored: false, sky_of: Some(pid), placed: false, coarse: r.coarse, stars: 0.0, level: sky::MEDIUM, before: None, by_planet: false });
                        self.lost_since = None;
                        self.last_paint = Some((at.0, at.1, Instant::now()));
                        self.sky_dirty.insert(pid, (true, true, 0.0, Instant::now()));
                        self.new_sky = false;
                        self.boot = None;
                        self.boot_note = None;
                        boot_said = Some(format!("Sky started from {} pictures ({:.0} % of it so far). Keep turning, every way: it records as you go.", r.fitted() + 1, cov * 100.0));
                    }
                    other => {
                        self.boot = None;
                        self.boot_note = Some(match other {
                            Some(r) if r.turned < 20.0 * DEG => "It didn't turn far enough while I watched (it needs about 20° in 3 s): keep turning the planet sideways, slowly and steadily.".to_string(),
                            Some(r) if r.fitted() < 3 => "Too fast to follow: turn the planet sideways a bit slower.".to_string(),
                            Some(_) => "Couldn't tell which way is up yet: turn sideways (left or right), not up and down.".to_string(),
                            None => "That didn't work out: turn the planet sideways, slowly.".to_string(),
                        });
                    }
                }
            } else if b.job.is_some() {
                if b.at.elapsed() >= Duration::from_millis(250) && b.catch.len() < 40 {
                    b.catch.push((f.clone(), BootFrame::of(&view)));
                    b.at = Instant::now();
                }
            } else if b.frames.is_empty() || b.at.elapsed() >= Duration::from_millis(250) {
                b.frames.push(f.clone());
                b.points.push(BootFrame::of(&view));
                b.at = Instant::now();
                if b.frames.len() > BOOT_PICTURES {
                    let pts = std::mem::take(&mut b.points);
                    let boot_rings = rings.map(|rg| (rg, disc.d));
                    b.job = std::thread::Builder::new()
                        .name("planetscan-boot".into())
                        .spawn(move || -> Option<sky::Boot> {
                            let (first, later) = pts.split_first()?;
                            sky::boot(first, later, boot_rings)
                        })
                        .ok();
                }
            }
        }

        if self.sky_cov.1.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
            let reach = self.reach.filter(|_| gas);
            let levels = self.planet_id().and_then(|pid| self.skies.get(&pid)).map(|sk| [sk.levels[0].coverage_in(reach.as_ref()), sk.levels[1].coverage_in(reach.as_ref()), sk.levels[2].coverage_in(reach.as_ref())]).unwrap_or([0.0; 3]);
            self.sky_cov = (levels, Some(Instant::now()));
        }

        // ---- recording this planet's own sky ----
        let fresh_buildings = !self.paint_held && self.pose.as_ref().is_some_and(|p| p.sky_of.is_none());
        // A gas giant's sky is recorded only once its rings are measured and cut out, and only
        // from facings the sky gave.
        let sky_records = !gas || (self.ring_clean && sky_usable);
        if let (Some(pid), Some(pose), true) = (self.planet_id(), self.pose.as_ref(), sky_records) {
            if ((pose.placed && !pose.by_planet) || fresh_buildings) && self.ui.as_ref().is_some_and(|u| u.ready()) {
                let moved = self.last_paint.is_none_or(|(y, p, t)| apart((y, p), (pose.yaw, pose.pitch)) > 0.6 * DEG && t.elapsed() > Duration::from_millis(120));
                let own = self.skies.get(&pid);
                let want = own.is_none() || self.sky_done() < 0.999;
                if self.rim_ok.0 && moved && want && own.is_none_or(|s| s.mirrored == pose.mirrored) {
                    let q = pose_q(pose.yaw, pose.pitch);
                    let (yaw, pitch, mirrored) = (pose.yaw, pose.pitch, pose.mirrored);
                    let sky = self.skies.entry(pid).or_insert_with(|| Sky::new(mirrored));
                    sky.paint(&view, &q, &[sky::COARSE, sky::MEDIUM, sky::FINE]);
                    self.last_paint = Some((yaw, pitch, Instant::now()));
                    let d = self.sky_dirty.entry(pid).or_insert((false, false, 0.0, Instant::now()));
                    d.0 = true;
                    d.1 = true;
                    if fresh_buildings {
                        if let Some(p) = self.pose.as_mut() {
                            p.sky_of = Some(pid);
                            p.placed = true;
                        }
                    }
                }
            }
        }

        // ---- own sky vs another planet's (the per-planet test) ----
        // Each planet's sky has its own longitude 0, so the same view sits at a constant longitude
        // shift on the other sky; the pitch is the same when both planets share the sky (tilt 0).
        if let (Some(pid), Some(b), Some(pose)) = (self.planet_id(), self.borrowed, self.pose.as_ref()) {
            if pose.placed && b != pid && pose.sky_of == Some(pid) && self.compare_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) && self.skies.get(&pid).is_some_and(|s| s.coverage() > 0.1) {
                self.compare_at = Some(Instant::now());
                let (yaw, pitch) = (pose.yaw, pose.pitch);
                let theirs = match self.compare_shift {
                    Some(shift) => self.skies.get_mut(&b).and_then(|s| sky::follow(&view, s, (yaw + shift, pitch), None).ok()).map(|f| (f.yaw, f.pitch)),
                    None => self.skies.get_mut(&b).and_then(|s| sky::find(&view, s, None)).filter(|f| f.sure()).map(|f| (f.yaw, f.pitch)),
                };
                if let Some((ty, tp)) = theirs {
                    let d = ty - yaw;
                    let shift = d.sin().atan2(d.cos());
                    self.compare_shift = Some(shift);
                    self.compare_last = Some(((tp - pitch) / DEG, shift / DEG, b));
                    self.compare = Some((apart((yaw + shift, pitch), (ty, tp)) / DEG, b, Instant::now()));
                } else if self.compare_shift.is_some() {
                    // Lost it: look again from scratch next time.
                    self.compare_shift = None;
                }
            }
        }

        // ---- scanning ----
        let placed = self.pose.as_ref().is_some_and(|p| p.placed);
        let picked = self.picked;
        // Only a list the pilot pointed out stops a scan (a guessed spot could read the wrong row).
        let list_known = !self.list_guessed;
        if let (Some(run), Some(pose)) = (self.run.as_mut(), self.pose.as_ref()) {
            if !run.done {
                run.paused = None;
                if !placed {
                    run.paused = Some("not on the sky");
                } else if let (Some(want), true) = (run.row, list_known) {
                    if picked.is_some_and(|p| p != want) {
                        run.paused = Some("EVE shows another resource");
                    }
                }
                if run.paused.is_none() {
                    let q = pose_q(pose.yaw, pose.pitch);
                    let bare = self.planet.as_ref().and_then(|p| self.bares.get(&p.id));
                    let every = (disc.r / 180.0).round().max(1.0) as usize;
                    run.map.paint(&pic, &disc, &q, pose.mirrored, every, if run.resource.is_empty() { None } else { bare });
                    run.painted_frames += 1;
                }
            }
        }
        if self.run.as_ref().is_some_and(|r| !r.done && r.paused.is_none() && r.map.coverage() >= DONE_AT) {
            self.finish(jobs);
        }

        // ---- the planet's own colours (EVE on No filter): the base map ----
        self.base_paused = None;
        if self.base_on && self.base_cov.0 < 0.9995 && self.planet.is_some() {
            match self.pose.as_ref().filter(|p| p.placed).map(|p| (p.yaw, p.pitch, p.by_planet)) {
                None => self.base_paused = Some("not on the sky"),
                Some((yaw, pitch, by_planet)) => {
                    let at = (yaw, pitch);
                    self.base_paused = if self.picked.is_some_and(|r| r != 0) {
                        Some("EVE shows a resource: set it to No filter")
                    } else if !self.rim_ok.0 {
                        Some("zoomed in: zoom all the way out")
                    } else if gas && !self.ring_clean {
                        Some("measuring the rings first")
                    } else {
                        None
                    };
                    if self.base_paused.is_none() && self.base_last.is_none_or(|l| apart(l, at) > 0.15 * DEG) {
                        let cut = self.ring_cut;
                        let base = self.base.get_or_insert_with(Base::new);
                        // Placed on the planet itself: only cells not seen face-on yet.
                        base.paint(&pic, &disc, &pose_q(at.0, at.1), cut.any().then_some((&cut, pitch)), by_planet);
                        self.base_last = Some(at);
                        self.base_dirty.0 = true;
                        self.base_dirty.1 = true;
                    }
                }
            }
        }
        if self.base.is_some() && self.base_cov.1.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            let cov = self.base.as_ref().map(|b| b.coverage()).unwrap_or(0.0);
            self.base_cov = (cov, Some(Instant::now()));
            if cov >= DONE_AT && !self.base_done {
                self.base_done = true;
                let autosave = self.autosave;
                self.save_base(jobs, autosave);
                let name = self.planet.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                self.event(json!({ "kind": "base-done", "coverage": cov, "saving": autosave,
                    "text": format!("{name}'s planet map is at {:.1} %{}. Keep going to fill the last spots (purple on the map).", cov * 100.0, if autosave { " and saved for the corp" } else { "" }) }));
            }
            if cov >= 0.999 && !self.base_full {
                self.base_full = true;
                let autosave = self.autosave;
                self.save_base(jobs, autosave);
                let name = self.planet.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                self.event(json!({ "kind": "base-full", "coverage": cov, "saving": autosave, "text": format!("{name}'s planet map is complete ({:.1} %){}.", cov * 100.0, if autosave { ", saved for the corp" } else { "" }) }));
            }
        }
        if let (Some(pid), true) = (self.planet_id(), self.base_dirty.0 && self.base_dirty.3.elapsed() > Duration::from_secs(20)) {
            if let (Some(path), Some(base)) = (self.map_path(pid, "bare"), self.base.as_ref()) {
                jobs.push(Job::WriteFile { path, bytes: base.to_bytes() });
            }
            // To the corp at 99 %, on Save now, or when switching planets: not every few minutes (6 MB).
            self.base_dirty = (false, self.base_dirty.1, self.base_dirty.2, Instant::now());
        }
        // ---- a gas giant's rings: what turning sideways shows adds to the cut (never takes away:
        // what was recorded without a ring stays right). A few looks a second. ----
        if gas && self.ring_clean {
            if self.ring_pole_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
                if let Some((yaw, pitch)) = self.pose.as_ref().filter(|p| p.placed && !p.by_planet).map(|p| (p.yaw, p.pitch)) {
                    self.ring_pole_at = Some(Instant::now());
                    let learner = self.ring_learner.get_or_insert_with(RingLearner::new);
                    learner.learn(&pic, &disc, lens, yaw, pitch, self.ui_trusted.as_ref());
                }
            }
            if self.ring_at.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                self.ring_at = Some(Instant::now());
                if let Some(cut) = self.ring_learner.as_ref().filter(|l| l.measured()).map(|l| l.rings()) {
                    let more = self.ring_cut.union(&cut);
                    if more != self.ring_cut {
                        self.set_ring_cut(more);
                        if let Some(path) = self.planet_id().and_then(|pid| self.map_path(pid, "rings3")) {
                            jobs.push(Job::WriteFile { path, bytes: rings::cut_to_bytes(&more) });
                        }
                    }
                }
                if self.ring_saved.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                    if let (Some(path), Some(l)) = (self.planet_id().and_then(|pid| self.map_path(pid, "rings2")), self.ring_learner.as_ref()) {
                        self.ring_saved = Some(Instant::now());
                        jobs.push(Job::WriteFile { path, bytes: l.to_bytes() });
                    }
                }
            }
        }
        if let Some(text) = boot_said.or(locked_said) {
            self.event(json!({ "kind": "locked", "text": text }));
        }
        if base_restarted {
            if let Some(path) = self.planet_id().and_then(|pid| self.map_path(pid, "bare")) {
                let _ = std::fs::remove_file(&path);
            }
            self.event(json!({ "kind": "warn", "text": "This planet's map starts over: it was made on an older sky, and a fresh sky moves where longitude 0 is." }));
        }

        // Keep this planet's own sky on the PC every 20 s; to the corp once it's 90 % recorded (so
        // the system's other planets can find themselves on it), and with the planet map.
        if let Some(pid) = self.planet_id() {
            if let (Some(d), Some(sky)) = (self.sky_dirty.get(&pid).copied(), self.skies.get(&pid)) {
                if d.0 && d.3.elapsed() > Duration::from_secs(20) {
                    if let Some(path) = self.map_path(pid, "sky") {
                        jobs.push(Job::WriteFile { path, bytes: sky.to_bytes() });
                    }
                    let cov = self.sky_cov.0.iter().cloned().fold(1.0, f64::min);
                    let send = self.autosave && d.1 && [0.9, 0.99, 0.999].iter().any(|&m| d.2 < m && cov >= m);
                    if send {
                        jobs.push(Job::SaveMap { planet: pid, kind: "sky", bytes: sky.to_bytes(), coverage: cov, meta: sky_meta(sky), character_id: self.character_id });
                    }
                    self.sky_dirty.insert(pid, (false, d.1 && !send, if send { cov } else { d.2 }, Instant::now()));
                }
            }
        }

        if held_painted {
            self.event(json!({ "kind": "locked", "text": "Sky recorded from the picture you pointed out: now turn the planet slowly all the way round, over both poles." }));
        }

        // Which planet is open (from its look), when asked to switch by itself.
        if self.auto_planet && self.identify_at.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.identify_at = Some(Instant::now());
            self.identify(&pic, &disc);
        }
        self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    /// The base map (and the sky it was recorded with) to the PC, and to the corp when asked.
    fn save_base(&mut self, jobs: &mut Vec<Job>, corp: bool) {
        let Some(pid) = self.planet_id() else { return };
        let Some(base) = self.base.as_ref() else { return };
        let bytes = base.to_bytes();
        let cov = base.coverage();
        if let Some(path) = self.map_path(pid, "bare") {
            jobs.push(Job::WriteFile { path, bytes: bytes.clone() });
        }
        if corp {
            jobs.push(Job::SaveMap { planet: pid, kind: "bare", bytes, coverage: cov, meta: String::new(), character_id: self.character_id });
            if let Some(sky) = self.skies.get(&pid) {
                if let Some(path) = self.map_path(pid, "sky") {
                    jobs.push(Job::WriteFile { path, bytes: sky.to_bytes() });
                }
                jobs.push(Job::SaveMap { planet: pid, kind: "sky", bytes: sky.to_bytes(), coverage: sky.coverage(), meta: sky_meta(sky), character_id: self.character_id });
                self.sky_dirty.insert(pid, (false, false, sky.coverage(), Instant::now()));
            }
        }
        self.base_dirty = (false, !corp && self.base_dirty.1, if corp { cov } else { self.base_dirty.2 }, Instant::now());
    }

    /// The planet in the picture against every mapped planet in the system (at the facing the
    /// sky gives: the same sky for all of them so far): switch when another clearly fits better.
    fn identify(&mut self, pic: &Pic, disc: &Disc) {
        let Some(pose) = self.pose.as_ref().filter(|p| p.placed) else { return };
        let q = pose_q(pose.yaw, pose.pitch);
        let mut scores: Vec<(u64, f64)> = self.bares.iter().filter_map(|(id, bare)| scan::looks_like(pic, disc, &q, pose.mirrored, bare).map(|s| (*id, s))).collect();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        let Some(&(best, top)) = scores.first() else { return };
        let second = scores.get(1).map(|s| s.1).unwrap_or(0.0);
        if Some(best) != self.planet_id() && top > 0.5 && top - second > 0.15 {
            let name = self.planet_name(best);
            self.event(json!({ "kind": "planet", "planet_id": best, "text": format!("This looks like {name}: switched to it.") }));
        }
    }

    /// 99 % (or Save now): the bare map is kept for the planet; a resource map is cleaned up and
    /// (with autosave) saved for the corp.
    fn finish(&mut self, jobs: &mut Vec<Job>) {
        let Some(pid) = self.planet_id() else { return };
        let pname = self.planet.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let autosave = self.autosave;
        let character_id = self.character_id;
        let Some(run) = self.run.as_mut() else { return };
        run.done = true;
        let coverage = run.map.coverage();
        let secs = run.started.elapsed().as_secs();
        if run.resource.is_empty() {
            let bytes = run.map.bare_bytes();
            if let Some(path) = self.dir.as_ref().map(|d| d.join(format!("{pid}-bare.bin"))) {
                jobs.push(Job::WriteFile { path, bytes: bytes.clone() });
            }
            if autosave {
                jobs.push(Job::SaveMap { planet: pid, kind: "bare", bytes: bytes.clone(), coverage, meta: String::new(), character_id });
            }
            if let Some(m) = Map::from_bare_bytes(&bytes) {
                self.bares.insert(pid, m);
            }
            self.event(json!({ "kind": "done", "resource": "", "planet_id": pid, "coverage": coverage, "secs": secs, "saving": autosave,
                "text": format!("{pname}'s own colours are mapped ({:.0} %, {secs} s){}.", coverage * 100.0, if autosave { " and being saved for the corp" } else { "" }) }));
            // The sky recorded with it goes to the corp too.
            if let Some(sky) = self.skies.get(&pid) {
                if autosave {
                    jobs.push(Job::SaveMap { planet: pid, kind: "sky", bytes: sky.to_bytes(), coverage: sky.coverage(), meta: sky_meta(sky), character_id });
                    self.sky_dirty.insert(pid, (true, false, sky.coverage(), Instant::now()));
                }
            }
        } else {
            run.map.despeckle(0.2);
            let cells = run.map.cells();
            let resource = run.resource.clone();
            if autosave {
                jobs.push(Job::SaveScan { planet: pid, planet_name: pname.clone(), resource: resource.clone(), character_id, cells, coverage });
            }
            self.event(json!({ "kind": "done", "resource": resource, "planet_id": pid, "coverage": coverage, "secs": secs, "saving": autosave,
                "text": format!("{resource} on {pname}: {:.0} % in {secs} s{}.", coverage * 100.0, if autosave { ", saving for the corp" } else { " (autosave is off: press Save for the corp)" }) }));
        }
    }

    fn status(&self, engine: &Inner) -> serde_json::Value {
        let boot = if self.pose.is_none() && (self.boot.is_some() || self.boot_note.is_some()) {
            json!({
                "window_looks": self.ui.as_ref().map(|u| u.views).unwrap_or(0), "window_looks_needed": BOOT_WINDOW_LOOKS,
                "pictures": self.boot.as_ref().map(|b| b.frames.len()).unwrap_or(0), "of": BOOT_PICTURES + 1,
                "working": self.boot.as_ref().is_some_and(|b| b.job.is_some()), "note": self.boot_note,
            })
        } else {
            serde_json::Value::Null
        };
        let edge_seek = if self.edge_r.is_none() {
            json!({ "agree": self.edge_seen.len(), "missing_ms": self.edge_missing.map(|t| t.elapsed().as_millis() as u64) })
        } else {
            serde_json::Value::Null
        };
        let reach = self.reach.filter(|_| self.is_gas());
        let rings_status = if self.is_gas() {
            let (phase, progress) = match self.ring_pole.as_ref() {
                Some(RingPole::Top(p)) => (Some("top"), p.progress()),
                Some(RingPole::Bottom(_, p)) => (Some("bottom"), p.progress()),
                None => (None, 1.0),
            };
            json!({
                "phase": phase, "progress": progress, "measured": self.ring_clean,
                "bands": self.ring_cut.bands().iter().map(|(a, b)| [*a, *b]).collect::<Vec<_>>(),
                "sky_share": self.mask.as_ref().map(|m| m.1.share()),
            })
        } else {
            serde_json::Value::Null
        };
        let sky_missing = self.pose.as_ref().filter(|_| self.sky_done() < 0.999).and_then(|p| self.planet_id().and_then(|id| self.skies.get(&id)).and_then(|s| s.missing_hint(&pose_q(p.yaw, p.pitch), reach.as_ref())));
        let base_missing = self.pose.as_ref().filter(|_| !self.base_done).and_then(|p| self.base.as_ref().and_then(|b| b.missing_hint(&pose_q(p.yaw, p.pitch))));
        let pose = self.pose.as_ref().map(|p| {
            json!({
                "yaw": p.yaw / DEG, "pitch": p.pitch / DEG, "mirrored": p.mirrored, "placed": p.placed,
                "q": pose_q(p.yaw, p.pitch), "coarse": p.coarse, "stars": p.stars, "level": sky::SPECS[p.level].name,
                "sky_of": p.sky_of, "sky_name": p.sky_of.map(|id| self.planet_name(id)), "by_planet": p.by_planet,
            })
        });
        let own = self.planet_id().and_then(|id| self.skies.get(&id));
        let run = self.run.as_ref().map(|r| {
            let hint = self.pose.as_ref().filter(|_| !r.done).and_then(|p| r.map.missing_hint(&pose_q(p.yaw, p.pitch), p.mirrored));
            json!({ "resource": r.resource, "row": r.row, "coverage": r.map.coverage(), "done": r.done, "paused": r.paused, "secs": r.started.elapsed().as_secs_f64(), "frames": r.painted_frames, "version": r.map.version, "missing": hint })
        });
        json!({
            "client": self.client,
            "size": [self.size.0, self.size.1],
            "capture": { "fps": engine.cap_fps.load(Ordering::Relaxed) as f64 / 100.0, "method": self.method, "trouble": self.trouble },
            "proc": { "fps": self.proc.per_second, "ms": self.step_ms },
            "edge": self.edge_r.map(|r| json!({ "r": r, "auto_zoom": self.auto_zoom })),
            "disc": self.disc().map(|d| json!({ "cx": d.cx, "cy": d.cy, "r": d.r, "D": d.d })),
            "lens": self.lens(),
            "planet": self.planet.as_ref().map(|p| json!({ "id": p.id, "name": p.name, "type": p.kind })),
            "pose": pose,
            "lost_ms": self.lost_since.map(|t| t.elapsed().as_millis() as u64),
            "fast_turn": self.fast_turn,
            "find": self.last_find_note,
            "sky": {
                "own": own.map(|s| json!({ "coverage": s.coverage(), "levels": self.sky_cov.0, "done": self.sky_done(), "mirrored": s.mirrored, "frames": s.frames })),
                "borrowed": self.borrowed.map(|b| json!({ "id": b, "name": self.planet_name(b), "loaded": self.skies.contains_key(&b) })),
                "compare": self.compare_last.map(|(tilt, shift, b)| json!({ "tilt": tilt, "shift": shift, "with": self.planet_name(b) })),
                "own_each": self.own_sky_each,
                "missing": sky_missing,
                "reach": reach.map(|r| rings::reachable_share(&r)),
                "recording": self.last_paint.is_some_and(|(_, _, t)| t.elapsed() < Duration::from_millis(800)),
                "unsent": self.planet_id().and_then(|id| self.sky_dirty.get(&id)).is_some_and(|d| d.1),
            },
            "bare": self.planet_id().and_then(|id| self.bares.get(&id)).map(|b| json!({ "coverage": b.coverage() })),
            "ui": self.ui.as_ref().map(|u| json!({ "ready": u.ready(), "views": u.views })),
            "list": { "picked": self.picked, "guessed": self.list_guessed, "spot": self.list.map(|s| json!({ "x": s.x, "y0": s.y0, "y5": s.y5 })) },
            "run": run,
            "autosave": self.autosave,
            "auto_planet": self.auto_planet,
            "events": self.events,
            "preview_width": engine.preview_width.load(Ordering::Relaxed),
            "held": self.held.is_some(),
            "base_map": {
                "coverage": self.base_cov.0, "on": self.base_on, "done": self.base_done, "full": self.base_full, "paused": self.base_paused,
                "version": self.base.as_ref().map(|b| b.version).unwrap_or(0),
                "missing": base_missing,
            },
            "boot": boot,
            "new_sky": self.new_sky,
            "rim_ok": self.rim_ok.0,
            "edge_seek": edge_seek,
            "started": self.started,
            "rings": rings_status,
            "recording": self.recorder.as_ref().map(|r| json!({ "frames": r.frames, "secs": r.started.elapsed().as_secs() })),
        })
    }
}

fn sky_meta(sky: &Sky) -> String {
    json!({ "levels": sky.levels.iter().map(|l| l.coverage()).collect::<Vec<_>>(), "frames": sky.frames, "mirrored": sky.mirrored }).to_string()
}

// ---------------------------------------------------------------------------------------------
// Threads.

fn publish(inner: &Inner, rate: &mut Rate, w: usize, h: usize, bgra: Vec<u8>, n: u64) {
    *inner.frame.lock().unwrap() = Some(Arc::new(Frame { w, h, bgra, n }));
    inner.frame_cv.notify_all();
    rate.tick();
    inner.cap_fps.store((rate.per_second * 100.0) as u64, Ordering::Relaxed);
}

fn capture_loop(inner: Arc<Inner>, gen: u64, client: String) {
    let mut n = 0u64;
    let mut rate = Rate::default();
    let alive = |inner: &Inner| inner.gen.load(Ordering::SeqCst) == gen;
    while alive(&inner) {
        let Some((_, handle)) = crate::clipboard::open_clients().into_iter().find(|(name, _)| *name == client) else {
            inner.state.lock().unwrap().trouble = Some(format!("EVE client {client} isn't open"));
            std::thread::sleep(Duration::from_millis(800));
            continue;
        };
        #[cfg(windows)]
        {
            match wgc::Capture::start(handle) {
                Ok(mut cap) => {
                    {
                        let mut s = inner.state.lock().unwrap();
                        s.method = "Windows capture";
                        s.trouble = None;
                    }
                    let mut idle = Instant::now();
                    while alive(&inner) {
                        match cap.next() {
                            Ok(Some((w, h, bgra))) => {
                                n += 1;
                                publish(&inner, &mut rate, w, h, bgra, n);
                                idle = Instant::now();
                            }
                            Ok(None) => {
                                if idle.elapsed() > Duration::from_secs(3) && crate::capture::minimized_handle(handle) {
                                    inner.state.lock().unwrap().trouble = Some("the EVE window is minimized".into());
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(why) => {
                                inner.state.lock().unwrap().trouble = Some(format!("Windows capture stopped: {why}"));
                                break;
                            }
                        }
                    }
                    if !alive(&inner) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                Err(why) => {
                    log::warn!("planet scan: Windows capture couldn't start ({why}); using the slower picture");
                    inner.state.lock().unwrap().trouble = Some(format!("Windows capture couldn't start ({why}): using the slower picture"));
                }
            }
        }
        // The slower picture (PrintWindow): still the full size.
        inner.state.lock().unwrap().method = "window picture (slower)";
        while alive(&inner) {
            match crate::capture::window_picture(handle, 4096) {
                Ok(p) => {
                    n += 1;
                    publish(&inner, &mut rate, p.width as usize, p.height as usize, p.bgra, n);
                }
                Err(why) => {
                    inner.state.lock().unwrap().trouble = Some(format!("can't read the EVE window: {why}"));
                    std::thread::sleep(Duration::from_millis(500));
                    break;
                }
            }
        }
    }
}

fn process_loop(inner: Arc<Inner>, gen: u64) {
    let mut buf = ViewBuffers::default();
    let mut last = 0u64;
    let mut status_at = Instant::now() - Duration::from_secs(1);
    while inner.gen.load(Ordering::SeqCst) == gen {
        let frame = {
            let mut slot = inner.frame.lock().unwrap();
            loop {
                if let Some(f) = slot.as_ref() {
                    if f.n > last {
                        break Some(f.clone());
                    }
                }
                let (s, timeout) = inner.frame_cv.wait_timeout(slot, Duration::from_millis(200)).unwrap();
                slot = s;
                if timeout.timed_out() {
                    break None;
                }
            }
        };
        let mut jobs = Vec::new();
        {
            let mut s = inner.state.lock().unwrap();
            if let Some(f) = frame {
                last = f.n;
                s.step(&f, &mut buf, &mut jobs);
            }
            if status_at.elapsed() > Duration::from_millis(60) {
                status_at = Instant::now();
                let text = s.status(&inner).to_string();
                *inner.status.lock().unwrap() = Arc::new(text);
            }
        }
        if !jobs.is_empty() {
            if let Some(tx) = inner.jobs.lock().unwrap().as_ref() {
                for job in jobs {
                    let _ = tx.send(job);
                }
            }
        }
    }
}

/// The picture for the page: the newest one, at most `preview_width` wide, as JPEG, ~30 a second.
fn preview_loop(inner: Arc<Inner>, gen: u64) {
    let mut last = 0u64;
    let mut seq = 0u64;
    while inner.gen.load(Ordering::SeqCst) == gen {
        let t = Instant::now();
        let frame = inner.frame.lock().unwrap().clone();
        if let Some(f) = frame.filter(|f| f.n > last) {
            last = f.n;
            let want = inner.preview_width.load(Ordering::Relaxed).clamp(320, 3840);
            let (w, h, px) = shrink(&f, want);
            let mut out = Vec::with_capacity(256 * 1024);
            if jpeg_encoder::Encoder::new(&mut out, 82).encode(&px, w as u16, h as u16, jpeg_encoder::ColorType::Bgra).is_ok() {
                seq += 1;
                {
                    let mut shown = inner.shown.lock().unwrap();
                    shown.push_back((seq, f.clone()));
                    while shown.len() > 6 {
                        shown.pop_front();
                    }
                }
                *inner.preview.lock().unwrap() = (seq, Arc::new(out));
                inner.preview_cv.notify_all();
            }
        }
        let spent = t.elapsed();
        if spent < Duration::from_millis(33) {
            std::thread::sleep(Duration::from_millis(33) - spent);
        }
    }
}

/// Scale a picture down to at most `want` wide (each output pixel the average of its area).
fn shrink(f: &Frame, want: usize) -> (usize, usize, std::borrow::Cow<'_, [u8]>) {
    if f.w <= want {
        return (f.w, f.h, std::borrow::Cow::Borrowed(&f.bgra));
    }
    let s = f.w as f64 / want as f64;
    let (w, h) = (want, ((f.h as f64 / s).round() as usize).max(1));
    let mut out = vec![255u8; w * h * 4];
    for y in 0..h {
        let (y0, y1) = ((y as f64 * s) as usize, (((y + 1) as f64 * s).ceil() as usize).min(f.h).max((y as f64 * s) as usize + 1));
        for x in 0..w {
            let (x0, x1) = ((x as f64 * s) as usize, (((x + 1) as f64 * s).ceil() as usize).min(f.w).max((x as f64 * s) as usize + 1));
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let k = (yy * f.w + xx) * 4;
                    acc[0] += f.bgra[k] as u32;
                    acc[1] += f.bgra[k + 1] as u32;
                    acc[2] += f.bgra[k + 2] as u32;
                    n += 1;
                }
            }
            let k = (y * w + x) * 4;
            for c in 0..3 {
                out[k + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (w, h, std::borrow::Cow::Owned(out))
}

async fn job_worker(app: AppHandle, mut rx: tokio::sync::mpsc::UnboundedReceiver<Job>) {
    while let Some(job) = rx.recv().await {
        let engine = app.state::<Engine>().inner().clone();
        let note = |value: serde_json::Value| engine.0.state.lock().unwrap().event(value);
        match job {
            Job::WriteFile { path, bytes } => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&path, bytes) {
                    log::warn!("planet scan: couldn't keep {}: {e}", path.display());
                }
            }
            Job::SaveScan { planet, planet_name, resource, character_id, cells, coverage } => match seat::save_scan(&app, planet, &resource, character_id, &cells).await {
                Ok(message) => note(json!({ "kind": "saved", "planet_id": planet, "resource": resource, "coverage": coverage, "text": format!("{resource} on {planet_name}: {message}") })),
                Err(why) => note(json!({ "kind": "error", "planet_id": planet, "resource": resource, "text": format!("Couldn't save {resource} on {planet_name}: {why}. Press Save for the corp to try again.") })),
            },
            Job::SaveMap { planet, kind, bytes, coverage, meta, character_id } => match seat::save_map(&app, planet, kind, &bytes, coverage, &meta, character_id).await {
                Ok(message) => note(json!({ "kind": "map-saved", "planet_id": planet, "map": kind, "text": message })),
                Err(why) => {
                    if kind == "sky" {
                        if let Some(d) = engine.0.state.lock().unwrap().sky_dirty.get_mut(&planet) {
                            d.1 = true;
                        }
                    }
                    note(json!({ "kind": "error", "planet_id": planet, "map": kind, "text": format!("Couldn't share the {kind} map with the corp: {why}") }))
                }
            },
            Job::DeleteMap { planet, kind } => {
                if let Err(why) = seat::delete_map(&app, planet, kind).await {
                    note(json!({ "kind": "error", "text": format!("Couldn't remove the corp's {kind} map: {why}") }));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Commands.

fn ensure_worker(app: &AppHandle, engine: &Engine) {
    let mut jobs = engine.0.jobs.lock().unwrap();
    if jobs.is_none() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *jobs = Some(tx);
        tauri::async_runtime::spawn(job_worker(app.clone(), rx));
    }
}

fn send_jobs(engine: &Engine, jobs: Vec<Job>) {
    if let Some(tx) = engine.0.jobs.lock().unwrap().as_ref() {
        for job in jobs {
            let _ = tx.send(job);
        }
    }
}

/// Start watching an EVE client (or switch to another). character_id: for the saves.
#[tauri::command]
pub fn planetscan_start(app: AppHandle, engine: tauri::State<'_, Engine>, client: String, character_id: Option<u64>) -> Result<(), String> {
    ensure_worker(&app, &engine);
    let gen = engine.0.gen.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut s = engine.0.state.lock().unwrap();
        if s.dir.is_none() {
            s.dir = app.path().app_local_data_dir().ok().map(|d| d.join("planetscan"));
        }
        if s.client.as_deref() != Some(client.as_str()) {
            s.size = (0, 0);
            s.pose = None;
            s.mask = None;
        }
        s.client = Some(client.clone());
        s.character_id = character_id;
        s.method = "";
        s.trouble = None;
    }
    *engine.0.frame.lock().unwrap() = None;
    let (a, b, c) = (engine.0.clone(), engine.0.clone(), engine.0.clone());
    std::thread::Builder::new().name("planetscan-capture".into()).spawn(move || capture_loop(a, gen, client)).map_err(|e| e.to_string())?;
    std::thread::Builder::new().name("planetscan-process".into()).spawn(move || process_loop(b, gen)).map_err(|e| e.to_string())?;
    std::thread::Builder::new().name("planetscan-preview".into()).spawn(move || preview_loop(c, gen)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Stop watching (the page closed).
#[tauri::command]
pub fn planetscan_stop(engine: tauri::State<'_, Engine>) {
    engine.0.gen.fetch_add(1, Ordering::SeqCst);
}

/// The newest preview after `after` (waits up to 60 ms for one) and the status, in one answer:
/// 4 bytes (status length), the status JSON, then the JPEG (none if nothing newer).
#[tauri::command]
pub async fn planetscan_poll(engine: tauri::State<'_, Engine>, after: u64, width: Option<usize>) -> Result<tauri::ipc::Response, String> {
    let inner = engine.0.clone();
    if let Some(w) = width {
        inner.preview_width.store(w.clamp(320, 3840), Ordering::Relaxed);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (seq, jpeg) = {
            let mut slot = inner.preview.lock().unwrap();
            let deadline = Instant::now() + Duration::from_millis(60);
            while slot.0 <= after {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                slot = inner.preview_cv.wait_timeout(slot, left).unwrap().0;
            }
            slot.clone()
        };
        let status = inner.status.lock().unwrap().clone();
        let text = format!("{{\"seq\":{seq},\"s\":{status}}}");
        let mut out = Vec::with_capacity(4 + text.len() + jpeg.len());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        if seq > after {
            out.extend_from_slice(&jpeg);
        }
        Ok(tauri::ipc::Response::new(out))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn load_map(app: &AppHandle, dir: Option<PathBuf>, planet: u64, kind: &str, seat_at: Option<i64>) -> Option<Vec<u8>> {
    let local = dir.as_ref().map(|d| d.join(local_name(planet, kind)));
    let stamp = dir.as_ref().map(|d| d.join(local_name(planet, kind).replace(".bin", ".at")));
    let local_at: i64 = stamp.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    let have = local.as_ref().and_then(|p| std::fs::read(p).ok());
    if let Some(at) = seat_at {
        if have.is_none() || at > local_at {
            if let Ok(Some(bytes)) = seat::get_map(app, planet, kind).await {
                if let (Some(p), Some(s)) = (local.as_ref(), stamp.as_ref()) {
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(p, &bytes);
                    let _ = std::fs::write(s, at.to_string());
                }
                return Some(bytes);
            }
        }
    }
    have
}

/// Pick the planet (and tell the app the system's planets with what SeAT has for each). Loads
/// this planet's own sky and bare map, and the best other sky in the system to borrow.
#[tauri::command]
pub async fn planetscan_planet(app: AppHandle, engine: tauri::State<'_, Engine>, planet: PlanetInfo, system: Vec<PlanetInfo>) -> Result<(), String> {
    let inner = engine.0.clone();
    let mut jobs = Vec::new();
    {
        // The same planet again (the page reloading SeAT's list): just the new list.
        let mut s = inner.state.lock().unwrap();
        if s.planet_id() == Some(planet.id) {
            s.system = system;
            s.planet = Some(planet);
            return Ok(());
        }
    }
    let dir = {
        let mut s = inner.state.lock().unwrap();
        // The planet we leave: its sky to the PC and the corp if it changed.
        if let Some(old) = s.planet_id().filter(|&id| id != planet.id) {
            if let (Some(d), Some(sky)) = (s.sky_dirty.get(&old).copied(), s.skies.get(&old)) {
                if let Some(path) = s.map_path(old, "sky") {
                    jobs.push(Job::WriteFile { path, bytes: sky.to_bytes() });
                }
                if d.1 && sky.coverage() > 0.05 {
                    jobs.push(Job::SaveMap { planet: old, kind: "sky", bytes: sky.to_bytes(), coverage: sky.coverage(), meta: sky_meta(sky), character_id: s.character_id });
                }
            }
            s.sky_dirty.remove(&old);
            if let (Some(path), Some(l)) = (s.map_path(old, "rings2"), s.ring_learner.as_ref()) {
                jobs.push(Job::WriteFile { path, bytes: l.to_bytes() });
            }
            if s.base.is_some() && (s.base_dirty.0 || s.base_dirty.1) {
                let corp = s.autosave && s.base_dirty.1 && s.base_cov.0 > 0.05;
                let mut more = Vec::new();
                s.save_base(&mut more, corp);
                jobs.extend(more);
            }
        }
        if s.planet_id() != Some(planet.id) {
            s.pose = None;
            s.run = None;
            s.compare = None;
            s.lost_since = None;
            s.mask = None;
            s.base = None;
            s.base_last = None;
            s.base_cov = (0.0, None);
            s.base_dirty = (false, false, 0.0, Instant::now());
            s.base_done = false;
            s.boot = None;
            s.boot_note = None;
            s.new_sky = false;
            s.base_full = false;
            s.sky_cov = ([0.0; 3], None);
            s.compare_shift = None;
            s.compare_last = None;
            s.compare = None;
            // Nothing until Start on this planet (EVE's opening zoom is under way).
            s.started = false;
            s.rim_lost = None;
            s.last_paint = None;
        }
        s.planet = Some(planet.clone());
        s.system = system.clone();
        // Keep only what this planet needs in memory.
        let keep: Vec<u64> = vec![planet.id];
        s.skies.retain(|id, _| keep.contains(id));
        s.borrowed = None;
        if s.dir.is_none() {
            s.dir = app.path().app_local_data_dir().ok().map(|d| d.join("planetscan"));
        }
        // EVE's windows as kept (a gas giant's rings may have got into the live mask), and this
        // planet's rings if it has been measured before.
        s.reload_windows();
        s.load_rings();
        s.dir.clone()
    };
    send_jobs(&engine, jobs);
    // This planet's own sky and bare map.
    let own_sky = load_map(&app, dir.clone(), planet.id, "sky", planet.sky_at.map(|t| t as i64)).await.filter(|b| Sky::is_current(b)).and_then(|b| Sky::from_bytes(&b));
    let base = load_map(&app, dir.clone(), planet.id, "bare", planet.bare_at.map(|t| t as i64)).await.and_then(|b| Base::from_bytes(&b));
    // Another planet's sky to borrow: the most complete in the system (on this PC or SeAT).
    let mut borrowed: Option<(u64, Sky)> = None;
    let own_cov = own_sky.as_ref().map(|s| s.coverage()).unwrap_or(0.0);
    if own_cov < 0.9 {
        let mut options: Vec<&PlanetInfo> = system.iter().filter(|p| p.id != planet.id && (p.sky_at.is_some() || dir.as_ref().is_some_and(|d| d.join(local_name(p.id, "sky")).exists()))).collect();
        options.sort_by(|a, b| b.sky_coverage.unwrap_or(0.5).total_cmp(&a.sky_coverage.unwrap_or(0.5)));
        for p in options {
            if let Some(sky) = load_map(&app, dir.clone(), p.id, "sky", p.sky_at.map(|t| t as i64)).await.filter(|b| Sky::is_current(b)).and_then(|b| Sky::from_bytes(&b)) {
                borrowed = Some((p.id, sky));
                break;
            }
        }
    }
    let mut s = inner.state.lock().unwrap();
    if s.planet_id() != Some(planet.id) {
        return Ok(());
    }
    if let Some(sky) = own_sky {
        s.skies.insert(planet.id, sky);
    }
    if let Some(base) = base {
        let cov = base.coverage();
        s.base_cov = (cov, Some(Instant::now()));
        s.base_done = cov >= DONE_AT;
        s.base_full = cov >= 0.999;
        s.base = Some(base);
    }
    if let Some((id, sky)) = borrowed {
        s.skies.insert(id, sky);
        s.borrowed = Some(id);
    }
    Ok(())
}

/// The other planets' bare maps (for telling which planet is open), loaded when Auto-switch is on.
#[tauri::command]
pub async fn planetscan_load_bares(app: AppHandle, engine: tauri::State<'_, Engine>) -> Result<usize, String> {
    let (dir, system) = {
        let s = engine.0.state.lock().unwrap();
        (s.dir.clone(), s.system.clone())
    };
    let mut loaded = Vec::new();
    for p in system.iter().filter(|p| p.bare_at.is_some()) {
        if let Some(m) = load_map(&app, dir.clone(), p.id, "bare", p.bare_at.map(|t| t as i64)).await.and_then(|b| Map::from_bare_bytes(&b)) {
            loaded.push((p.id, m));
        }
    }
    let n = loaded.len();
    let mut s = engine.0.state.lock().unwrap();
    for (id, m) in loaded {
        s.bares.insert(id, m);
    }
    Ok(n)
}

/// Start (EVE zoomed all the way out on the picked planet): the edge from the newest picture,
/// then the app watches. Pressed again after a planet change or once the edge was lost.
#[tauri::command]
pub fn planetscan_go(engine: tauri::State<'_, Engine>) -> Result<f64, String> {
    let frame = engine.0.frame.lock().unwrap().clone();
    let f = frame.ok_or("no picture from EVE yet")?;
    let found = sky::rim_radius(&Pic { w: f.w, h: f.h, bgra: &f.bgra }, Some(sky::ZOOMED_OUT_RIM * lens_for(f.w)), lens_for(f.w), None)
        .ok_or("no planet edge at the zoomed-out size: zoom all the way out in EVE (mouse wheel back as far as it goes) and press Start again")?;
    let mut s = engine.0.state.lock().unwrap();
    if s.planet.is_none() {
        return Err("pick the planet first".into());
    }
    let changed = s.edge_r.is_none_or(|r| (r - found).abs() > found * 0.01);
    s.edge_seen.clear();
    s.edge_missing = None;
    s.edge_auto = true;
    if changed {
        s.edge_r = Some(found);
        s.mask = None;
        // The ring cut's reach depends on the planet's size on screen.
        let cut = s.ring_cut;
        s.set_ring_cut(cut);
    }
    s.started = true;
    s.rim_lost = None;
    s.rim_ok = (true, Some(Instant::now()));
    Ok(found)
}

/// Record the pictures for testing (a picture every 200 ms with what the app made of it, at
/// most 3 minutes), or stop. Kept in the app's recordings folder; nothing leaves the PC.
#[tauri::command]
pub fn planetscan_record(app: AppHandle, engine: tauri::State<'_, Engine>, on: bool) -> Result<Option<String>, String> {
    let mut s = engine.0.state.lock().unwrap();
    if !on {
        s.recorder = None;
        return Ok(None);
    }
    if s.recorder.is_some() {
        return Ok(None);
    }
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("scan2-{stamp}.bin"));
    let (tx, rx) = std::sync::mpsc::channel::<(Arc<Frame>, String)>();
    let inner = engine.0.clone();
    let out = path.clone();
    std::thread::Builder::new()
        .name("planetscan-record".into())
        .spawn(move || record_loop(inner, rx, out))
        .map_err(|e| e.to_string())?;
    s.recorder = Some(Recorder { tx, started: Instant::now(), last: None, frames: 0 });
    Ok(Some(path.display().to_string()))
}

/// The recorder: "REC2", then for each picture its note and its JPEG, each as a u32 (LE) length
/// then the bytes. Ends when the recording stops (or at 700 MB).
fn record_loop(inner: Arc<Inner>, rx: std::sync::mpsc::Receiver<(Arc<Frame>, String)>, path: PathBuf) {
    use std::io::Write;
    let Ok(file) = std::fs::File::create(&path) else {
        inner.state.lock().unwrap().event(json!({ "kind": "error", "text": format!("Couldn't start a recording at {}", path.display()) }));
        return;
    };
    let mut out = std::io::BufWriter::new(file);
    let _ = out.write_all(b"REC2");
    let (mut n, mut bytes) = (0u32, 4u64);
    while let Ok((f, note)) = rx.recv() {
        let mut jpeg = Vec::with_capacity(512 * 1024);
        if jpeg_encoder::Encoder::new(&mut jpeg, 90).encode(&f.bgra, f.w as u16, f.h as u16, jpeg_encoder::ColorType::Bgra).is_err() {
            continue;
        }
        let ok = out.write_all(&(note.len() as u32).to_le_bytes()).is_ok() && out.write_all(note.as_bytes()).is_ok() && out.write_all(&(jpeg.len() as u32).to_le_bytes()).is_ok() && out.write_all(&jpeg).is_ok();
        if !ok {
            break;
        }
        n += 1;
        bytes += 8 + note.len() as u64 + jpeg.len() as u64;
        if bytes > 700 << 20 {
            inner.state.lock().unwrap().recorder = None;
            break;
        }
    }
    let _ = out.flush();
    inner.state.lock().unwrap().event(json!({ "kind": "saved", "text": format!("Recording saved: {n} pictures, {} MB, in {}", bytes >> 20, path.display()) }));
}

/// The planet's edge: `find` looks for it in the newest picture; `r` sets it (picture pixels,
/// from a click); neither clears it (found again by itself).
#[tauri::command]
pub fn planetscan_edge(engine: tauri::State<'_, Engine>, r: Option<f64>, find: Option<bool>) -> Result<Option<f64>, String> {
    let frame = engine.0.frame.lock().unwrap().clone();
    let mut s = engine.0.state.lock().unwrap();
    s.mask = None;
    if find == Some(true) {
        let f = frame.ok_or("no picture yet")?;
        let found = sky::rim_radius(&Pic { w: f.w, h: f.h, bgra: &f.bgra }, Some(sky::ZOOMED_OUT_RIM * lens_for(f.w)), lens_for(f.w), None)
            .ok_or("no planet edge at the zoomed-out size: zoom all the way out in EVE (or click the edge)")?;
        s.edge_seen.clear();
        s.edge_r = Some(found);
        s.edge_auto = true;
        return Ok(Some(found));
    }
    s.edge_r = r.filter(|r| *r > 20.0);
    s.edge_auto = r.is_none();
    s.edge_seen.clear();
    s.edge_missing = None;
    Ok(s.edge_r)
}

#[derive(serde::Deserialize)]
pub struct Options {
    autosave: Option<bool>,
    auto_zoom: Option<bool>,
    auto_planet: Option<bool>,
    own_sky_each: Option<bool>,
}

#[tauri::command]
pub fn planetscan_options(engine: tauri::State<'_, Engine>, options: Options) {
    let mut s = engine.0.state.lock().unwrap();
    if let Some(v) = options.autosave {
        s.autosave = v;
    }
    if let Some(v) = options.auto_zoom {
        s.auto_zoom = v;
    }
    if let Some(v) = options.auto_planet {
        s.auto_planet = v;
    }
    if let Some(v) = options.own_sky_each {
        if v != s.own_sky_each && s.pose.as_ref().is_some_and(|p| Some(p.sky_of.unwrap_or(0)) != s.planet_id()) {
            // Was on another planet's sky: start over on this planet's own.
            s.pose = None;
        }
        s.own_sky_each = v;
    }
}

/// Facing from the buildings (the page works it out from buildings pointed out on the picture):
/// the planet's own sky starts recording from it.
#[tauri::command]
pub fn planetscan_pose(engine: tauri::State<'_, Engine>, yaw: f64, pitch: f64, mirrored: bool) {
    let mut s = engine.0.state.lock().unwrap();
    s.pose = Some(Pose { yaw: yaw * DEG, pitch: pitch * DEG, mirrored, sky_of: None, placed: true, coarse: 0.0, stars: 0.0, level: 0, before: None, by_planet: false });
    s.lost_since = None;
    s.last_paint = None;
    s.paint_held = s.held.is_some();
    if !s.paint_held {
        s.event(json!({ "kind": "locked", "text": "Locked on from the buildings: turn the planet slowly, this planet's sky records as you go." }));
    }
}

/// Hold the picture the page shows (preview number `seq`) while buildings are pointed out on it;
/// None lets go. True when a picture is held.
#[tauri::command]
pub fn planetscan_hold(engine: tauri::State<'_, Engine>, seq: Option<u64>) -> bool {
    let frame = seq.and_then(|n| {
        let shown = engine.0.shown.lock().unwrap();
        let found: Option<Arc<Frame>> = shown.iter().rev().find(|(k, _)| *k == n).or(shown.back()).map(|(_, f)| f.clone());
        found
    });
    let mut s = engine.0.state.lock().unwrap();
    s.held = frame;
    s.paint_held = false;
    s.held.is_some()
}

/// Drop the facing and look on the sky again.
#[tauri::command]
pub fn planetscan_find(engine: tauri::State<'_, Engine>) {
    let mut s = engine.0.state.lock().unwrap();
    s.pose = None;
    s.last_find = None;
    s.lost_since = None;
}

/// Start a scan of the resource EVE shows ("" = the bare planet on No filter; row = its place in
/// EVE's list, 0 … 5), or stop one (resource None).
#[tauri::command]
pub fn planetscan_scan(engine: tauri::State<'_, Engine>, resource: Option<String>, row: Option<usize>) -> Result<(), String> {
    let mut s = engine.0.state.lock().unwrap();
    match resource {
        None => s.run = None,
        Some(resource) => {
            if s.planet.is_none() {
                return Err("pick the planet first".into());
            }
            let bare = resource.is_empty();
            s.run = Some(Run { resource, row, map: Map::new(bare), started: Instant::now(), done: false, paused: None, painted_frames: 0 });
        }
    }
    Ok(())
}

/// Finish the scan now (under 99 %) and save it for the corp.
#[tauri::command]
pub fn planetscan_save(engine: tauri::State<'_, Engine>) -> Result<(), String> {
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        let Some(run) = s.run.as_mut() else { return Err("nothing scanned yet".into()) };
        if run.map.coverage() < 0.05 {
            return Err("less than 5 % scanned: turn the planet some more first".into());
        }
        let was = s.autosave;
        s.autosave = true;
        if s.run.as_ref().is_some_and(|r| r.done) {
            // Done already (autosave was off): save it as it is.
            if let Some(r) = s.run.as_mut() {
                r.done = false;
            }
        }
        s.finish(&mut jobs);
        s.autosave = was;
    }
    send_jobs(&engine, jobs);
    Ok(())
}

/// Where EVE's resource list is: picture x over the names, y of No filter, y of the last resource.
#[tauri::command]
pub fn planetscan_list(engine: tauri::State<'_, Engine>, x: f64, y0: f64, y5: f64) -> Result<(), String> {
    if y5 - y0 < 30.0 {
        return Err("click No filter first, then the last resource below it".into());
    }
    let mut s = engine.0.state.lock().unwrap();
    s.list = Some(ListSpot { x, y0, y5 });
    s.list_guessed = false;
    if let Some(path) = s.list_path() {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(path, format!("{x} {y0} {y5}"));
    }
    Ok(())
}

/// The planet's own sky (or the borrowed one) as a picture for the page: 720 × 360 grey bytes,
/// 0 = not recorded. level: 0 coarse, 1 medium, 2 fine.
#[tauri::command]
pub fn planetscan_skymap(engine: tauri::State<'_, Engine>, level: usize) -> tauri::ipc::Response {
    let s = engine.0.state.lock().unwrap();
    let id = s.planet_id().filter(|id| s.skies.contains_key(id)).or(s.borrowed);
    let mut out = vec![0u8; 720 * 360];
    if let Some(sky) = id.and_then(|id| s.skies.get(&id)) {
        let m = &sky.levels[level.min(2)];
        let scale = sky::SPECS[level.min(2)].scale * 0.6;
        for y in 0..360 {
            for x in 0..720 {
                let i = (y * m.h / 360) * m.w + x * m.w / 720;
                if m.n[i] > 0 {
                    out[y * 720 + x] = (128.0 + scale * m.sum[i] / m.n[i] as f32).clamp(1.0, 255.0) as u8;
                }
            }
        }
    }
    tauri::ipc::Response::new(out)
}

/// The map being scanned (or the planet's bare map): 1 byte kind (0 none, 1 resource, 2 bare),
/// then 360 × 180 levels (0 = not seen, 1 … 251) or r, g, b a cell.
#[tauri::command]
pub fn planetscan_map(engine: tauri::State<'_, Engine>) -> tauri::ipc::Response {
    let s = engine.0.state.lock().unwrap();
    let mut out = Vec::new();
    match s.run.as_ref() {
        Some(r) if !r.resource.is_empty() => {
            out.push(1);
            out.extend(r.map.cells());
        }
        Some(r) => {
            out.push(2);
            out.extend(r.map.bare_bytes());
        }
        None => match s.planet_id().and_then(|id| s.bares.get(&id)) {
            Some(b) => {
                out.push(2);
                out.extend(b.bare_bytes());
            }
            None => out.push(0),
        },
    }
    tauri::ipc::Response::new(out)
}

/// A sky from the old scanner (kept in the page's own storage) taken over as this planet's sky.
#[tauri::command]
pub fn planetscan_import_sky(engine: tauri::State<'_, Engine>, planet_id: u64, data: String) -> Result<String, String> {
    let bytes = seat::b64_decode(&data).ok_or("that isn't a sky")?;
    let sky = Sky::from_bytes(&bytes).ok_or("that isn't a sky the scanner knows")?;
    let cov = sky.coverage();
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        if let Some(path) = s.map_path(planet_id, "sky") {
            jobs.push(Job::WriteFile { path, bytes: sky.to_bytes() });
        }
        jobs.push(Job::SaveMap { planet: planet_id, kind: "sky", bytes: sky.to_bytes(), coverage: cov, meta: sky_meta(&sky), character_id: s.character_id });
        s.skies.insert(planet_id, sky);
        s.pose = None;
    }
    send_jobs(&engine, jobs);
    Ok(format!("Imported: {:.0} % of the sky.", cov * 100.0))
}

/// Forget a planet's own sky (on this PC and the corp's copy) to record it again.
#[tauri::command]
pub fn planetscan_forget_sky(engine: tauri::State<'_, Engine>, planet_id: u64) {
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        s.skies.remove(&planet_id);
        s.sky_dirty.remove(&planet_id);
        if let Some(path) = s.map_path(planet_id, "sky") {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("at"));
        }
        s.boot = None;
        s.boot_note = None;
        if s.pose.as_ref().is_some_and(|p| p.sky_of == Some(planet_id)) {
            s.pose = None;
        }
        jobs.push(Job::DeleteMap { planet: planet_id, kind: "sky" });
    }
    send_jobs(&engine, jobs);
}

/// A gas giant's rings measured again from over its poles (Tsa: when tracking is lost, start
/// over). Its sky goes too (recorded with the old cut); its planet map stays until a fresh sky
/// moves longitude 0.
#[tauri::command]
pub fn planetscan_forget_rings(engine: tauri::State<'_, Engine>, planet_id: u64) {
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        for kind in ["rings3", "rings2", "sky"] {
            if let Some(path) = s.map_path(planet_id, kind) {
                let _ = std::fs::remove_file(path);
            }
        }
        s.skies.remove(&planet_id);
        s.sky_dirty.remove(&planet_id);
        jobs.push(Job::DeleteMap { planet: planet_id, kind: "sky" });
        if s.planet_id() == Some(planet_id) {
            s.ring_learner = None;
            s.load_rings();
            s.pose = None;
            s.boot = None;
            s.boot_note = None;
            s.lost_since = None;
        }
    }
    send_jobs(&engine, jobs);
}

/// Record the base map (the planet's own colours) while on the sky and EVE shows No filter, or not.
#[tauri::command]
pub fn planetscan_base(engine: tauri::State<'_, Engine>, on: bool) {
    let mut s = engine.0.state.lock().unwrap();
    s.base_on = on;
}

/// This planet's base map from scratch (here and the corp's copy).
#[tauri::command]
pub fn planetscan_forget_base(engine: tauri::State<'_, Engine>, planet_id: u64) {
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        if s.planet_id() == Some(planet_id) {
            s.base = None;
            s.base_last = None;
            s.base_cov = (0.0, None);
            s.base_done = false;
            s.base_full = false;
            s.base_dirty = (false, false, 0.0, Instant::now());
        }
        if let Some(path) = s.map_path(planet_id, "bare") {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("at"));
        }
        jobs.push(Job::DeleteMap { planet: planet_id, kind: "bare" });
    }
    send_jobs(&engine, jobs);
}

/// Start this planet's own sky from nothing, instead of finding it on another planet's.
#[tauri::command]
pub fn planetscan_new_sky(engine: tauri::State<'_, Engine>) {
    let mut s = engine.0.state.lock().unwrap();
    s.new_sky = true;
    s.pose = None;
    s.boot = None;
    s.boot_note = None;
}

/// Save this planet's sky and base map for the corp now (also below 99 %).
#[tauri::command]
pub fn planetscan_save_maps(engine: tauri::State<'_, Engine>) -> Result<(), String> {
    let mut jobs = Vec::new();
    {
        let mut s = engine.0.state.lock().unwrap();
        let pid = s.planet_id().ok_or("pick the planet first")?;
        if s.base.is_some() {
            s.save_base(&mut jobs, true);
        } else if let Some(sky) = s.skies.get(&pid) {
            jobs.push(Job::SaveMap { planet: pid, kind: "sky", bytes: sky.to_bytes(), coverage: sky.coverage(), meta: sky_meta(sky), character_id: s.character_id });
        } else {
            return Err("nothing recorded on this planet yet".into());
        }
    }
    send_jobs(&engine, jobs);
    Ok(())
}

/// A map as a JPEG for the page: "sky" (this planet's own sky, else the borrowed one: the fine
/// stars, 2880 × 1440) or "planet" (the base map, 2048 × 1024). Empty when there's none.
#[tauri::command]
pub async fn planetscan_mapview(engine: tauri::State<'_, Engine>, kind: String) -> Result<tauri::ipc::Response, String> {
    let inner = engine.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Inside the lock only a quick copy: the scanner keeps going while the picture is made.
        enum Snapshot {
            Planet(usize, usize, Vec<u8>),
            Sky { w: usize, h: usize, sum: Vec<f32>, n: Vec<u16>, mw: usize, mh: usize, msum: Vec<f32>, mn: Vec<u16>, reach: Option<[bool; 181]> },
            None,
        }
        let copy = {
            let s = inner.state.lock().unwrap();
            if kind == "planet" {
                match s.base.as_ref() {
                    Some(b) => Snapshot::Planet(b.w, b.h, b.rgb()),
                    None => Snapshot::None,
                }
            } else {
                let id = s.planet_id().filter(|id| s.skies.contains_key(id)).or(s.borrowed);
                match id.and_then(|id| s.skies.get(&id)) {
                    Some(sky) => {
                        let (m, mm) = (&sky.levels[sky::FINE], &sky.levels[sky::MEDIUM]);
                        Snapshot::Sky { w: m.w, h: m.h, sum: m.sum.clone(), n: m.n.clone(), mw: mm.w, mh: mm.h, msum: mm.sum.clone(), mn: mm.n.clone(), reach: s.reach.filter(|_| s.is_gas()) }
                    }
                    None => Snapshot::None,
                }
            }
        };
        let (w, h, rgb) = match copy {
            Snapshot::None => return tauri::ipc::Response::new(Vec::new()),
            Snapshot::Planet(w, h, mut rgb) => {
                base::fill_gaps(&mut rgb, w, h);
                // Not seen yet: purple, so a missed spot stands out on a dark planet.
                for px in rgb.chunks_exact_mut(3) {
                    if px[0] == 0 && px[1] == 0 && px[2] == 0 {
                        px.copy_from_slice(&[96, 0, 88]);
                    }
                }
                (w, h, rgb)
            }
            Snapshot::Sky { w, h, sum, n, mw, mh, msum, mn, reach } => {
                let scale = sky::SPECS[sky::FINE].scale * 0.8;
                let mscale = sky::SPECS[sky::MEDIUM].scale * 0.8;
                let mut out = vec![0u8; w * h * 3];
                for i in 0..w * h {
                    let px = if n[i] > 0 {
                        let v = (40.0 + scale * sum[i] / n[i] as f32).clamp(0.0, 255.0) as u8;
                        [v, v, v]
                    } else {
                        // Not recorded at this level yet: the medium level shows through (bluish), else dark blue.
                        let (row, col) = (i / w, i % w);
                        let k = (row * mh / h) * mw + col * mw / w;
                        if mn[k] > 0 {
                            let v = (40.0 + mscale * msum[k] / mn[k] as f32).clamp(0.0, 255.0) as u8;
                            [v / 2, v / 2, v / 2 + 30]
                        } else if reach.is_some_and(|ok| !ok[sky::height_index(row, h)]) {
                            // Behind a gas giant's rings from every side: can't be recorded.
                            if (row / 6 + col / 6) % 2 == 0 { [44, 44, 52] } else { [34, 34, 40] }
                        } else {
                            [96, 0, 88]
                        }
                    };
                    out[i * 3..i * 3 + 3].copy_from_slice(&px);
                }
                (w, h, out)
            }
        };
        let mut jpeg = Vec::with_capacity(1 << 20);
        if jpeg_encoder::Encoder::new(&mut jpeg, 88).encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).is_err() {
            jpeg.clear();
        }
        tauri::ipc::Response::new(jpeg)
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_keeps_the_shape() {
        let f = Frame { w: 1920, h: 1024, bgra: vec![100; 1920 * 1024 * 4], n: 1 };
        let (w, h, px) = shrink(&f, 1280);
        assert_eq!((w, h), (1280, 683));
        assert_eq!(px[0], 100);
    }
}
