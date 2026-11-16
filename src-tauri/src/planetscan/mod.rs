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

pub mod geom;
pub mod scan;
pub mod seat;
pub mod sky;
#[cfg(windows)]
mod wgc;

use geom::{apart, pose_q, Disc};
use scan::{ListSpot, Map};
use serde_json::json;
use sky::{Pic, Rings, Sky, SkyMask, UiMask, ViewBuffers, DEG, RINGS};
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
    #[serde(default)]
    pub sky_at: Option<i64>,
    #[serde(default)]
    pub bare_at: Option<i64>,
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
    auto_zoom: bool,
    pose: Option<Pose>,
    skies: HashMap<u64, Sky>,
    /// Own sky changed since saved: (on this PC, for the corp, coverage when last sent).
    sky_dirty: HashMap<u64, (bool, bool, f64, Instant)>,
    borrowed: Option<u64>,
    rings: HashMap<u64, bool>,
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
            auto_zoom: false,
            pose: None,
            skies: HashMap::new(),
            sky_dirty: HashMap::new(),
            borrowed: None,
            rings: HashMap::new(),
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
        }
    }
}

pub struct Inner {
    state: Mutex<Session>,
    frame: Mutex<Option<Arc<Frame>>>,
    frame_cv: Condvar,
    preview: Mutex<(u64, Arc<Vec<u8>>)>,
    preview_cv: Condvar,
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
            status: Mutex::new(Arc::new("{}".into())),
            gen: AtomicU64::new(0),
            cap_fps: AtomicU64::new(0),
            preview_width: AtomicUsize::new(1280),
            jobs: Mutex::new(None),
        }))
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
    /// Rings to leave out of the sky now: known for this planet, or worth trying on a gas giant.
    fn rings_now(&self) -> Option<Rings> {
        let id = self.planet_id()?;
        match self.rings.get(&id) {
            Some(true) => Some(RINGS),
            Some(false) => None,
            None => self.is_gas().then_some(RINGS),
        }
    }
    fn ui_path(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("ui-{}-{}x{}.bin", safe_name(self.client.as_deref()?), self.size.0, self.size.1)))
    }
    fn list_path(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("list-{}-{}x{}.txt", safe_name(self.client.as_deref()?), self.size.0, self.size.1)))
    }
    fn map_path(&self, planet: u64, kind: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{planet}-{kind}.bin")))
    }

    /// A new picture size (or client): the edge scales along, the windows mask and the list spot
    /// are this client's at this size.
    fn resized(&mut self, w: usize, h: usize) {
        if let (Some(r), true) = (self.edge_r, self.size.0 > 0) {
            self.edge_r = Some(r * w as f64 / self.size.0 as f64);
        }
        self.size = (w, h);
        self.mask = None;
        self.ui = self.ui_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| UiMask::from_bytes(&b, w, h)).or_else(|| Some(UiMask::new(w, h)));
        self.ui_version += 1;
        let saved = self.list_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| {
            let v: Vec<f64> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| ListSpot { x: v[0], y0: v[1], y5: v[2] })
        });
        self.list_guessed = saved.is_none();
        // EVE's planet panel at 90 % UI scale on a 1918 × 1024 window (Tsa's): No filter at y 216,
        // the fifth resource at 342, names round x 140. Scaled by the window's height.
        self.list = saved.or(Some(ListSpot { x: 140.0 * h as f64 / 1024.0, y0: 216.0 * h as f64 / 1024.0, y5: 342.0 * h as f64 / 1024.0 }));
    }

    /// The sky to place this picture with: the planet's own (if it has any), else the borrowed one.
    fn skies_to_try(&self) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(id) = self.planet_id() {
            if self.skies.get(&id).is_some_and(|s| s.coverage() > 0.01) {
                out.push(id);
            }
        }
        if let Some(b) = self.borrowed {
            if self.skies.contains_key(&b) && !out.contains(&b) {
                out.push(b);
            }
        }
        out
    }

    fn step(&mut self, f: &Frame, buf: &mut ViewBuffers, jobs: &mut Vec<Job>) {
        let start = Instant::now();
        if (f.w, f.h) != self.size {
            self.resized(f.w, f.h);
        }
        self.frames += 1;
        self.proc.tick();
        let pic = Pic { w: f.w, h: f.h, bgra: &f.bgra };
        let disc = self.disc();

        // EVE's windows (every sixth picture) and which row of EVE's resource list is lit.
        if self.frames % 6 == 0 {
            let path = self.ui_path();
            if let Some(ui) = self.ui.as_mut() {
                if ui.learn(&pic, disc.as_ref()) {
                    self.ui_version += 1;
                    if ui.ready() && self.ui_saved.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
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

        // The edge: found once (zoomed all the way out) and then kept; Auto-zoom follows it.
        if self.edge_r.is_none() && self.edge_auto && self.frames % 15 == 1 {
            if let Some(r) = sky::rim_radius(&pic, None, self.lens(), self.ui.as_ref()) {
                self.edge_r = Some(r);
                self.mask = None;
            }
        } else if self.auto_zoom && self.frames % 15 == 0 {
            if let Some(r0) = self.edge_r {
                if let Some(r) = sky::rim_radius(&pic, Some(r0), self.lens(), self.ui.as_ref()) {
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

        // Which pixels are sky (rebuilt when the edge, EVE's windows or the rings' tilt change).
        let rings = self.rings_now();
        let ring_pitch = self.pose.as_ref().map(|p| p.pitch).unwrap_or(0.0);
        let key = ((disc.r * 4.0) as i64, f.w as i64, f.h as i64, self.ui_version, if rings.is_some() && self.pose.is_some() { (ring_pitch / DEG).round() as i64 } else { i64::MIN });
        if self.mask.as_ref().is_none_or(|(k, _)| *k != key) {
            let ui = self.ui.as_ref().filter(|u| u.ready());
            let ring_now = rings.filter(|_| self.pose.is_some()).map(|rg| (rg, sky::axis_for_pitch(ring_pitch), disc.d));
            self.mask = Some((key, SkyMask::new(f.w, f.h, Some(&disc), lens, ui, ring_now)));
        }
        let mask = &self.mask.as_ref().unwrap().1;
        let view = sky::view(buf, &pic, mask, lens);
        let find_rings = |r: Option<Rings>| r.map(|rg| (rg, disc.d));

        // ---- the facing ----
        let tries = self.skies_to_try();
        if let Some(pose) = self.pose.as_mut() {
            pose.placed = false;
            let predicted = match pose.before {
                Some(b) => (2.0 * pose.yaw - b.0, 2.0 * pose.pitch - b.1),
                None => (pose.yaw, pose.pitch),
            };
            let now = (pose.yaw, pose.pitch);
            for id in &tries {
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
            if pose.placed {
                self.lost_since = None;
            } else if self.lost_since.is_none() {
                self.lost_since = Some(Instant::now());
            }
        }
        let lost_long = self.lost_since.is_some_and(|t| t.elapsed() > Duration::from_millis(400));
        if (self.pose.is_none() || lost_long) && !tries.is_empty() && self.last_find.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            self.last_find = Some(Instant::now());
            // A gas giant: try with and without its rings left out (and remember which works).
            let ring_options: Vec<Option<Rings>> = match (self.is_gas(), self.planet_id().and_then(|id| self.rings.get(&id).copied())) {
                (true, None) => vec![None, Some(RINGS)],
                (true, Some(true)) => vec![Some(RINGS)],
                _ => vec![None],
            };
            let mut best: Option<(sky::Find, u64, Option<Rings>)> = None;
            for id in &tries {
                let Some(sky) = self.skies.get_mut(id) else { continue };
                for rg in &ring_options {
                    if let Some(found) = sky::find(&view, sky, find_rings(*rg)) {
                        if best.as_ref().is_none_or(|b| found.coarse + found.stars > b.0.coarse + b.0.stars) {
                            best = Some((found, *id, *rg));
                        }
                    }
                }
            }
            self.last_find_note = best.as_ref().map(|(f, id, rg)| json!({ "coarse": f.coarse, "stars": f.stars, "margin": f.margin, "sure": f.sure(), "sky": id, "rings": rg.is_some() }));
            if let Some((found, id, rg)) = best.filter(|b| b.0.sure()) {
                let mirrored = self.skies.get(&id).map(|s| s.mirrored).unwrap_or(true);
                let was_none = self.pose.is_none();
                self.pose = Some(Pose { yaw: found.yaw, pitch: found.pitch, mirrored, sky_of: Some(id), placed: true, coarse: found.coarse, stars: found.stars, level: found.level, before: None });
                self.lost_since = None;
                if let Some(pid) = self.planet_id() {
                    if self.is_gas() {
                        self.rings.insert(pid, rg.is_some());
                    }
                }
                if was_none {
                    let whose = if Some(id) == self.planet_id() { "this planet's own sky".to_string() } else { format!("{}'s sky", self.planet_name(id)) };
                    self.event(json!({ "kind": "locked", "text": format!("Locked on by {whose}.") }));
                }
            }
        }

        // ---- recording this planet's own sky ----
        let fresh_buildings = self.pose.as_ref().is_some_and(|p| p.sky_of.is_none());
        if let (Some(pid), Some(pose)) = (self.planet_id(), self.pose.as_ref()) {
            if (pose.placed || fresh_buildings) && self.ui.as_ref().is_some_and(|u| u.ready()) {
                if self.rim_ok.1.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                    self.rim_ok = (sky::rim_still_there(&pic, &disc), Some(Instant::now()));
                }
                let moved = self.last_paint.is_none_or(|(y, p, t)| apart((y, p), (pose.yaw, pose.pitch)) > 0.6 * DEG && t.elapsed() > Duration::from_millis(120));
                let own = self.skies.get(&pid);
                let want = own.is_none_or(|s| s.coverage() < 0.995 || s.frames < 600);
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

        // ---- own sky vs the borrowed one (the per-planet test) ----
        if let (Some(pid), Some(b), Some(pose)) = (self.planet_id(), self.borrowed, self.pose.as_ref()) {
            if pose.placed && b != pid && self.compare_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) && self.skies.get(&pid).is_some_and(|s| s.coverage() > 0.1) {
                self.compare_at = Some(Instant::now());
                let at = (pose.yaw, pose.pitch);
                let mine = self.skies.get_mut(&pid).and_then(|s| sky::follow(&view, s, at, None).ok()).map(|f| (f.yaw, f.pitch));
                let theirs = self.skies.get_mut(&b).and_then(|s| sky::follow(&view, s, at, None).ok()).map(|f| (f.yaw, f.pitch));
                if let (Some(m), Some(t)) = (mine, theirs) {
                    self.compare = Some((apart(m, t) / DEG, b, Instant::now()));
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

        // Keep this planet's own sky on the PC every 20 s, and with the corp every 2 minutes.
        if let Some(pid) = self.planet_id() {
            if let (Some(d), Some(sky)) = (self.sky_dirty.get(&pid).copied(), self.skies.get(&pid)) {
                if d.0 && d.3.elapsed() > Duration::from_secs(20) {
                    if let Some(path) = self.map_path(pid, "sky") {
                        jobs.push(Job::WriteFile { path, bytes: sky.to_bytes() });
                    }
                    let cov = sky.coverage();
                    let send = d.1 && (cov - d.2 > 0.05) && d.3.elapsed() > Duration::from_secs(120);
                    if send {
                        jobs.push(Job::SaveMap { planet: pid, kind: "sky", bytes: sky.to_bytes(), coverage: cov, meta: sky_meta(sky), character_id: self.character_id });
                    }
                    self.sky_dirty.insert(pid, (false, d.1 && !send, if send { cov } else { d.2 }, Instant::now()));
                }
            }
        }

        // Which planet is open (from its look), when asked to switch by itself.
        if self.auto_planet && self.identify_at.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.identify_at = Some(Instant::now());
            self.identify(&pic, &disc);
        }
        self.step_ms = start.elapsed().as_secs_f64() * 1000.0;
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
        let pose = self.pose.as_ref().map(|p| {
            json!({
                "yaw": p.yaw / DEG, "pitch": p.pitch / DEG, "mirrored": p.mirrored, "placed": p.placed,
                "q": pose_q(p.yaw, p.pitch), "coarse": p.coarse, "stars": p.stars, "level": sky::SPECS[p.level].name,
                "sky_of": p.sky_of, "sky_name": p.sky_of.map(|id| self.planet_name(id)),
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
                "own": own.map(|s| json!({ "coverage": s.coverage(), "levels": s.levels.iter().map(|l| l.coverage()).collect::<Vec<_>>(), "mirrored": s.mirrored, "frames": s.frames })),
                "borrowed": self.borrowed.map(|b| json!({ "id": b, "name": self.planet_name(b), "loaded": self.skies.contains_key(&b) })),
                "compare": self.compare.map(|(deg, b, _)| json!({ "deg": deg, "with": self.planet_name(b) })),
                "rings": self.planet_id().and_then(|id| self.rings.get(&id).copied()),
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
    let local = dir.as_ref().map(|d| d.join(format!("{planet}-{kind}.bin")));
    let stamp = dir.as_ref().map(|d| d.join(format!("{planet}-{kind}.at")));
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
        if s.planet_id() == Some(planet.id) && (s.skies.contains_key(&planet.id) || s.borrowed.is_some_and(|b| s.skies.contains_key(&b))) {
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
        }
        if s.planet_id() != Some(planet.id) {
            s.pose = None;
            s.run = None;
            s.compare = None;
            s.lost_since = None;
            s.mask = None;
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
        s.dir.clone()
    };
    send_jobs(&engine, jobs);
    // This planet's own sky and bare map.
    let own_sky = load_map(&app, dir.clone(), planet.id, "sky", planet.sky_at).await.and_then(|b| Sky::from_bytes(&b));
    let bare = load_map(&app, dir.clone(), planet.id, "bare", planet.bare_at).await.and_then(|b| Map::from_bare_bytes(&b));
    // Another planet's sky to borrow: the most complete in the system (on this PC or SeAT).
    let mut borrowed: Option<(u64, Sky)> = None;
    let own_cov = own_sky.as_ref().map(|s| s.coverage()).unwrap_or(0.0);
    if own_cov < 0.9 {
        let mut options: Vec<&PlanetInfo> = system.iter().filter(|p| p.id != planet.id && (p.sky_at.is_some() || dir.as_ref().is_some_and(|d| d.join(format!("{}-sky.bin", p.id)).exists()))).collect();
        options.sort_by(|a, b| b.sky_coverage.unwrap_or(0.5).total_cmp(&a.sky_coverage.unwrap_or(0.5)));
        for p in options {
            if let Some(sky) = load_map(&app, dir.clone(), p.id, "sky", p.sky_at).await.and_then(|b| Sky::from_bytes(&b)) {
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
    if let Some(bare) = bare {
        s.bares.insert(planet.id, bare);
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
        if let Some(m) = load_map(&app, dir.clone(), p.id, "bare", p.bare_at).await.and_then(|b| Map::from_bare_bytes(&b)) {
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

/// The planet's edge: `find` looks for it in the newest picture; `r` sets it (picture pixels,
/// from a click); neither clears it (found again by itself).
#[tauri::command]
pub fn planetscan_edge(engine: tauri::State<'_, Engine>, r: Option<f64>, find: Option<bool>) -> Result<Option<f64>, String> {
    let frame = engine.0.frame.lock().unwrap().clone();
    let mut s = engine.0.state.lock().unwrap();
    s.mask = None;
    if find == Some(true) {
        let f = frame.ok_or("no picture yet")?;
        let found = sky::rim_radius(&Pic { w: f.w, h: f.h, bgra: &f.bgra }, None, lens_for(f.w), s.ui.as_ref()).ok_or("couldn't find the planet's edge: zoom all the way out in EVE, or click the edge")?;
        s.edge_r = Some(found);
        s.edge_auto = true;
        return Ok(Some(found));
    }
    s.edge_r = r.filter(|r| *r > 20.0);
    s.edge_auto = r.is_none();
    Ok(s.edge_r)
}

#[derive(serde::Deserialize)]
pub struct Options {
    autosave: Option<bool>,
    auto_zoom: Option<bool>,
    auto_planet: Option<bool>,
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
}

/// Facing from the buildings (the page works it out from buildings pointed out on the picture):
/// the planet's own sky starts recording from it.
#[tauri::command]
pub fn planetscan_pose(engine: tauri::State<'_, Engine>, yaw: f64, pitch: f64, mirrored: bool) {
    let mut s = engine.0.state.lock().unwrap();
    s.pose = Some(Pose { yaw: yaw * DEG, pitch: pitch * DEG, mirrored, sky_of: None, placed: true, coarse: 0.0, stars: 0.0, level: 0, before: None });
    s.lost_since = None;
    s.last_paint = None;
    s.event(json!({ "kind": "locked", "text": "Locked on from the buildings: turn the planet slowly, this planet's sky records as you go." }));
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
        if s.pose.as_ref().is_some_and(|p| p.sky_of == Some(planet_id)) {
            s.pose = None;
        }
        jobs.push(Job::DeleteMap { planet: planet_id, kind: "sky" });
    }
    send_jobs(&engine, jobs);
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
