//! The sky lock (David's idea): EVE's planet view moves the camera round the planet, and the
//! planet and everything behind it (stars, nebula, the sun) stay put. So the background turns
//! exactly with the planet, and a sky map recorded once (in planet directions, while locked)
//! tells which way the camera faces from any picture with sky in it.
//!
//! The picture is measured as features: a spot's brightness minus its surroundings, at fixed
//! sizes in degrees, kept at three levels (coarse: the nebula's shapes, for finding with no
//! guess; medium; fine: single stars at full size). The map keeps the same features, so picture
//! and map compare like with like. EVE's own windows (screen-fixed), the planet with its glow,
//! and a gas giant's rings are left out.

use super::geom::{apply, mul, pose_q, rotation_about, stand, transpose, Disc, M3, V3};
use std::f64::consts::PI;

pub const DEG: f64 = PI / 180.0;

/// One picture, 4 bytes a pixel: blue, green, red, unused (what Windows hands over).
pub struct Pic<'a> {
    pub w: usize,
    pub h: usize,
    pub bgra: &'a [u8],
}

impl Pic<'_> {
    #[inline]
    pub fn lum(&self, x: usize, y: usize) -> u32 {
        let k = (y * self.w + x) * 4;
        (28 * self.bgra[k] as u32 + 151 * self.bgra[k + 1] as u32 + 77 * self.bgra[k + 2] as u32) >> 8
    }
    #[inline]
    pub fn rgb(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let k = (y * self.w + x) * 4;
        (self.bgra[k + 2], self.bgra[k + 1], self.bgra[k])
    }
}

/// The three levels: map size, feature size (spot and surroundings, degrees), how densely
/// pictures are measured when painting and when matching (degrees), and the clip (so a window's
/// text or a sun flare can't dominate).
pub struct Spec {
    pub name: &'static str,
    pub w: usize,
    pub h: usize,
    pub spot: f64,
    pub ring: f64,
    pub paint: f64,
    pub matching: f64,
    pub clip: f32,
    /// Bytes: value × scale around 128.
    pub scale: f32,
}

pub const COARSE: usize = 0;
pub const MEDIUM: usize = 1;
pub const FINE: usize = 2;
pub const SPECS: [Spec; 3] = [
    Spec { name: "coarse", w: 360, h: 180, spot: 1.5, ring: 7.0, paint: 1.0, matching: 2.2, clip: 5.0, scale: 24.0 },
    Spec { name: "medium", w: 1440, h: 720, spot: 0.3, ring: 1.4, paint: 0.25, matching: 0.9, clip: 8.0, scale: 15.0 },
    Spec { name: "fine", w: 2880, h: 1440, spot: 0.15, ring: 0.7, paint: 0.125, matching: 0.45, clip: 10.0, scale: 12.0 },
];

/// The planet's disc plus its glow (lava, atmosphere) isn't sky.
pub const GLOW: f64 = 1.25;

pub use super::rings::Rings;

/// The planet's axis in camera space for a pitch (the rings don't care about yaw).
#[inline]
pub fn axis_for_pitch(pitch: f64) -> V3 {
    [0.0, pitch.cos(), pitch.sin()]
}

// ---------------------------------------------------------------------------------------------
// EVE's windows: a screen-fixed mask, learnt as the camera turns.

/// Blocks of the picture that are EVE's windows: their sharp pixels stay put while the sky
/// behind moves, or they are bright and don't change at all.
#[derive(Clone)]
pub struct UiMask {
    pub size: usize,
    pub bw: usize,
    pub bh: usize,
    pub w: usize,
    pub h: usize,
    pub ui: Vec<u8>,
    still: Vec<f32>,
    seen: Vec<u8>,
    flat: Vec<f32>,
    /// Block means at an earlier look, `FLAT_GAP` counted looks back: a flat window is told from
    /// smooth nebula by how little it changed since then (looks a moment apart barely differ
    /// anywhere when EVE draws 60 pictures a second).
    flat_ref: Option<(Vec<f32>, u32)>,
    prev: Option<Vec<u8>>,
    pub views: u32,
}

/// Counted looks between the flat-window comparisons (about a second of turning at 60 fps).
const FLAT_GAP: u32 = 8;

impl UiMask {
    pub fn new(w: usize, h: usize) -> UiMask {
        let size = 8;
        let (bw, bh) = (w.div_ceil(size), h.div_ceil(size));
        UiMask { size, bw, bh, w, h, ui: vec![0; bw * bh], still: vec![0.0; bw * bh], seen: vec![0; bw * bh], flat: vec![9.0; bw * bh], flat_ref: None, prev: None, views: 0 }
    }
    /// Share of the picture marked as EVE's windows.
    pub fn share(&self) -> f64 {
        self.ui.iter().filter(|&&u| u != 0).count() as f64 / self.ui.len().max(1) as f64
    }
    pub fn ready(&self) -> bool {
        self.views >= 4
    }
    #[inline]
    pub fn blocked(&self, x: usize, y: usize) -> bool {
        self.ui[(y / self.size) * self.bw + x / self.size] != 0
    }
    /// Learn from one picture (only pictures where the camera turned count). True when it changed.
    /// Only once the planet's edge is known: the planet and its glow are never a window (smooth and
    /// slow to change, they'd look like one).
    pub fn learn(&mut self, pic: &Pic, disc: Option<&Disc>) -> bool {
        let Some(disc) = disc else { return false };
        let disc = Some(disc);
        if pic.w != self.w || pic.h != self.h {
            return false;
        }
        let (w, h, size, bw, bh) = (pic.w, pic.h, self.size, self.bw, self.bh);
        let mut lum = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                lum[y * w + x] = pic.lum(x, y) as u8;
            }
        }
        let nb = bw * bh;
        let mut mean = vec![0f32; nb];
        let mut cnt = vec![0f32; nb];
        for y in (0..h).step_by(2) {
            for x in (0..w).step_by(2) {
                let b = (y / size) * bw + x / size;
                mean[b] += lum[y * w + x] as f32;
                cnt[b] += 1.0;
            }
        }
        for b in 0..nb {
            mean[b] /= cnt[b].max(1.0);
        }
        let Some(prev) = self.prev.replace(lum.clone()) else { return false };
        let out = disc.map(|d| (d.r * GLOW).powi(2)).unwrap_or(-1.0);
        let mut tex = vec![0u16; nb];
        let mut still = vec![0u16; nb];
        let mut all_tex = 0u32;
        for y in 0..h - 1 {
            let row = (y / size) * bw;
            let dy2 = disc.map(|d| (y as f64 - d.cy).powi(2)).unwrap_or(0.0);
            for x in 0..w - 1 {
                let i = y * w + x;
                let l = lum[i] as i32;
                if (lum[i + 1] as i32 - l).abs() + (lum[i + w] as i32 - l).abs() < 28 {
                    continue;
                }
                if let Some(d) = disc {
                    if (x as f64 - d.cx).powi(2) + dy2 < out {
                        continue;
                    }
                }
                let b = row + x / size;
                tex[b] = tex[b].saturating_add(1);
                if (l - prev[i] as i32).abs() <= 3 {
                    still[b] = still[b].saturating_add(1);
                }
                all_tex += 1;
            }
        }
        let mut tex_blocks = 0;
        let mut moved = 0;
        for b in 0..nb {
            if tex[b] < 4 {
                continue;
            }
            tex_blocks += 1;
            if (still[b] as f32) < tex[b] as f32 * 0.4 {
                moved += 1;
            }
        }
        if all_tex < 50 || moved < 10 || (moved as f32) < tex_blocks as f32 * 0.3 {
            return false;
        }
        self.views += 1;
        // Flat windows: how much each block changed since a look at least a second and FLAT_GAP
        // looks back (the camera has turned a few degrees by then; nebula has changed, a window not).
        let compare = match &self.flat_ref {
            None => {
                self.flat_ref = Some((mean.clone(), self.views));
                false
            }
            Some((_, v)) => self.views >= v + FLAT_GAP,
        };
        if compare {
            if let Some((last, _)) = self.flat_ref.take() {
                for b in 0..nb {
                    let d = (mean[b] - last[b]).abs();
                    self.flat[b] = if self.flat[b] == 9.0 { d } else { 0.7 * self.flat[b] + 0.3 * d };
                }
            }
            self.flat_ref = Some((mean.clone(), self.views));
        }
        for b in 0..nb {
            if tex[b] < 4 {
                continue;
            }
            let share = still[b] as f32 / tex[b] as f32;
            self.still[b] = if self.seen[b] != 0 { 0.8 * self.still[b] + 0.2 * share } else { share };
            self.seen[b] = 1;
        }
        let mut core: Vec<u8> = (0..nb).map(|b| ((self.seen[b] != 0 && self.still[b] > 0.6) || (mean[b] > 35.0 && self.flat[b] < 0.6)) as u8).collect();
        // A window's flat inside: still blocks within 5 on both sides.
        let near = |core: &Vec<u8>, bx: i64, by: i64, sx: i64, sy: i64| -> bool {
            for k in 1..=5 {
                let (x, y) = (bx + sx * k, by + sy * k);
                if x < 0 || y < 0 || x >= bw as i64 || y >= bh as i64 {
                    return false;
                }
                if core[y as usize * bw + x as usize] != 0 {
                    return true;
                }
            }
            false
        };
        let inside: Vec<u8> = (0..nb)
            .map(|b| {
                let (bx, by) = ((b % bw) as i64, (b / bw) as i64);
                (core[b] != 0 || (near(&core, bx, by, -1, 0) && near(&core, bx, by, 1, 0)) || (near(&core, bx, by, 0, -1) && near(&core, bx, by, 0, 1))) as u8
            })
            .collect();
        core = inside;
        let mut changed = false;
        for b in 0..nb {
            let (bx, by) = ((b % bw) as i64, (b / bw) as i64);
            let mut ui = 0u8;
            'n: for dy in -1..=1i64 {
                for dx in -1..=1i64 {
                    let (x, y) = (bx + dx, by + dy);
                    if x >= 0 && y >= 0 && x < bw as i64 && y < bh as i64 && core[y as usize * bw + x as usize] != 0 {
                        ui = 1;
                        break 'n;
                    }
                }
            }
            if self.views < 4 {
                ui = 0;
            }
            if let Some(d) = disc {
                let (px, py) = ((bx as f64 + 0.5) * size as f64, (by as f64 + 0.5) * size as f64);
                if (px - d.cx).powi(2) + (py - d.cy).powi(2) < out {
                    ui = 0;
                }
            }
            if ui != self.ui[b] {
                self.ui[b] = ui;
                changed = true;
            }
        }
        changed
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.size as u8, (self.bw >> 8) as u8, self.bw as u8, (self.bh >> 8) as u8, self.bh as u8, self.views.min(255) as u8];
        out.extend_from_slice(&self.ui);
        out
    }
    pub fn from_bytes(bytes: &[u8], w: usize, h: usize) -> Option<UiMask> {
        let mut m = UiMask::new(w, h);
        if bytes.len() < 6 || bytes[0] as usize != m.size {
            return None;
        }
        let (bw, bh) = (((bytes[1] as usize) << 8) | bytes[2] as usize, ((bytes[3] as usize) << 8) | bytes[4] as usize);
        if bw != m.bw || bh != m.bh || bytes.len() != 6 + bw * bh {
            return None;
        }
        m.ui.copy_from_slice(&bytes[6..]);
        m.views = bytes[5] as u32;
        for i in 0..m.ui.len() {
            if m.ui[i] != 0 {
                m.still[i] = 1.0;
                m.seen[i] = 1;
            }
        }
        Some(m)
    }
}

// ---------------------------------------------------------------------------------------------
// A picture's sky, ready to measure.

/// Buffers for the box sums, kept between pictures (two 16 MB arrays at full size).
#[derive(Default)]
pub struct ViewBuffers {
    lum: Vec<u32>,
    sky: Vec<u32>,
}

pub struct View<'a> {
    pub w: usize,
    pub h: usize,
    pub lens: f64,
    pub cx0: f64,
    pub cy0: f64,
    /// Pixels per degree.
    pub ppd: f64,
    lum: &'a [u32],
    sky: &'a [u32],
}

/// Which pixels are sky: not the planet with its glow, not EVE's windows, not the rings. Made
/// once and kept while those don't change (the rings only with the pitch).
pub struct SkyMask {
    pub w: usize,
    pub h: usize,
    bits: Vec<u8>,
}

impl SkyMask {
    pub fn new(w: usize, h: usize, disc: Option<&Disc>, lens: f64, ui: Option<&UiMask>, rings: Option<(Rings, V3, f64)>) -> SkyMask {
        let mut bits = vec![1u8; w * h];
        let (cx0, cy0) = (w as f64 / 2.0, h as f64 / 2.0);
        let r2 = disc.map(|d| (d.r * GLOW).powi(2)).unwrap_or(-1.0);
        let (dcx, dcy) = disc.map(|d| (d.cx, d.cy)).unwrap_or((0.0, 0.0));
        for y in 0..h {
            let dy2 = (y as f64 + 0.5 - dcy).powi(2);
            for x in 0..w {
                if (x as f64 + 0.5 - dcx).powi(2) + dy2 < r2 || ui.is_some_and(|ui| ui.blocked(x, y)) {
                    bits[y * w + x] = 0;
                }
            }
        }
        // The rings, tested on 4 × 4 blocks (their edges are soft anyway).
        if let Some((rg, n, d)) = &rings {
            for by in (0..h).step_by(4) {
                let v = -((by + 2) as f64 - cy0) / lens;
                for bx in (0..w).step_by(4) {
                    let u = ((bx + 2) as f64 - cx0) / lens;
                    let l = (u * u + v * v + 1.0).sqrt();
                    if rg.hit(&[(u / l) as f32, (v / l) as f32, (-1.0 / l) as f32], n, *d) {
                        for y in by..(by + 4).min(h) {
                            bits[y * w + bx..y * w + (bx + 4).min(w)].fill(0);
                        }
                    }
                }
            }
        }
        SkyMask { w, h, bits }
    }
    /// Share of the picture that's sky (not the planet, EVE's windows or rings).
    pub fn share(&self) -> f64 {
        self.bits.iter().step_by(7).filter(|&&b| b != 0).count() as f64 / self.bits.len().div_ceil(7).max(1) as f64
    }
}

/// Measure a picture: brightness and "is sky" summed for quick box averages. lens = focal length
/// in pixels at this size.
pub fn view<'a>(buf: &'a mut ViewBuffers, pic: &Pic, mask: &SkyMask, lens: f64) -> View<'a> {
    let (w, h) = (pic.w, pic.h);
    debug_assert!(mask.w == w && mask.h == h);
    let w1 = w + 1;
    let size = w1 * (h + 1);
    buf.lum.resize(size, 0);
    buf.sky.resize(size, 0);
    buf.lum[..=w].fill(0);
    buf.sky[..=w].fill(0);
    for y in 0..h {
        let (mut row_l, mut row_s) = (0u32, 0u32);
        let base = (y + 1) * w1;
        let up = y * w1;
        buf.lum[base] = 0;
        buf.sky[base] = 0;
        let px = &pic.bgra[y * w * 4..(y + 1) * w * 4];
        let bits = &mask.bits[y * w..(y + 1) * w];
        let (lum_up, lum_row) = buf.lum.split_at_mut(base);
        let (sky_up, sky_row) = buf.sky.split_at_mut(base);
        let lum_up = &lum_up[up..up + w1];
        let sky_up = &sky_up[up..up + w1];
        for x in 0..w {
            let m = bits[x] as u32;
            let k = x * 4;
            row_l += m * ((28 * px[k] as u32 + 151 * px[k + 1] as u32 + 77 * px[k + 2] as u32) >> 8);
            row_s += m;
            lum_row[x + 1] = lum_up[x + 1] + row_l;
            sky_row[x + 1] = sky_up[x + 1] + row_s;
        }
    }
    View { w, h, lens, cx0: w as f64 / 2.0, cy0: h as f64 / 2.0, ppd: lens * DEG, lum: &buf.lum, sky: &buf.sky }
}

impl View<'_> {
    #[inline]
    fn boxsum(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> (u32, u32) {
        let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
        let (x1, y1) = ((x1.max(0) as usize).min(self.w), (y1.max(0) as usize).min(self.h));
        if x1 <= x0 || y1 <= y0 {
            return (0, 0);
        }
        let w1 = self.w + 1;
        let (a, b, c, d) = (y0 * w1 + x0, y0 * w1 + x1, y1 * w1 + x0, y1 * w1 + x1);
        let n = self.sky[d].wrapping_sub(self.sky[b]).wrapping_sub(self.sky[c]).wrapping_add(self.sky[a]);
        let s = self.lum[d].wrapping_sub(self.lum[b]).wrapping_sub(self.lum[c]).wrapping_add(self.lum[a]);
        (n, s)
    }
    /// Spot minus surroundings at (x, y), or None where it isn't mostly sky.
    #[inline]
    fn feature(&self, x: i64, y: i64, hb: i64, hr: i64) -> Option<f32> {
        let (n, s) = self.boxsum(x - hb, y - hb, x + hb + 1, y + hb + 1);
        let side = (2 * hb + 1) as f32;
        if (n as f32) < side * side * 0.8 {
            return None;
        }
        let (n2, s2) = self.boxsum(x - hr, y - hr, x + hr + 1, y + hr + 1);
        let side2 = (2 * hr + 1) as f32;
        if (n2 as f32) < side2 * side2 * 0.35 {
            return None;
        }
        Some(s as f32 / n as f32 - s2 as f32 / n2 as f32)
    }
    /// Camera-space direction of a picture point (camera looking down −z).
    #[inline]
    pub fn ray(&self, x: f64, y: f64) -> [f32; 3] {
        let u = (x + 0.5 - self.cx0) / self.lens;
        let v = -(y + 0.5 - self.cy0) / self.lens;
        let l = (u * u + v * v + 1.0).sqrt();
        [(u / l) as f32, (v / l) as f32, (-1.0 / l) as f32]
    }
}

#[derive(Clone, Copy)]
pub struct Pt {
    pub d: [f32; 3],
    pub g: f32,
}

/// At most this many points per level when matching (spread over the picture).
const MATCH_CAP: [usize; 3] = [800, 3000, 6000];

/// Sky features for matching: on the level's grid, thinned out evenly to the level's cap.
pub fn match_points(view: &View, level: usize) -> Vec<Pt> {
    let all = points(view, level, SPECS[level].matching);
    let cap = MATCH_CAP[level];
    if all.len() <= cap {
        return all;
    }
    let keep = cap as f64 / all.len() as f64;
    let mut acc = 0.0;
    all.into_iter()
        .filter(|_| {
            acc += keep;
            if acc >= 1.0 {
                acc -= 1.0;
                true
            } else {
                false
            }
        })
        .collect()
}

/// Sky features on a grid every `gap` degrees.
pub fn points(view: &View, level: usize, gap: f64) -> Vec<Pt> {
    let s = &SPECS[level];
    let hb = ((s.spot * view.ppd / 2.0).round() as i64).max(1);
    let hr = ((s.ring * view.ppd / 2.0).round() as i64).max(hb + 2);
    let step = ((gap * view.ppd).round() as i64).max(2);
    let mut out = Vec::new();
    let mut y = hb + step / 2;
    while y < view.h as i64 - hb {
        let mut x = hb + step / 2;
        while x < view.w as i64 - hb {
            if let Some(g) = view.feature(x, y, hb, hr) {
                out.push(Pt { d: view.ray(x as f64, y as f64), g: g.clamp(-s.clip, s.clip) });
            }
            x += step;
        }
        y += step;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The sky map.

pub struct Level {
    pub w: usize,
    pub h: usize,
    pub sum: Vec<f32>,
    pub n: Vec<u16>,
    grey: Option<Vec<f32>>,
}

impl Level {
    fn new(level: usize) -> Level {
        let s = &SPECS[level];
        Level { w: s.w, h: s.h, sum: vec![0.0; s.w * s.h], n: vec![0; s.w * s.h], grey: None }
    }
    #[inline]
    fn cell_of(&self, s: &[f64; 3]) -> usize {
        let lat = s[2].clamp(-1.0, 1.0).acos();
        let mut lon = s[1].atan2(s[0]);
        if lon < 0.0 {
            lon += 2.0 * PI;
        }
        ((lat / PI * self.h as f64).floor() as usize).min(self.h - 1) * self.w + ((lon / (2.0 * PI) * self.w as f64).floor() as usize).min(self.w - 1)
    }
    pub fn grey(&mut self) -> &Vec<f32> {
        if self.grey.is_none() {
            self.grey = Some((0..self.sum.len()).map(|i| if self.n[i] > 0 { self.sum[i] / self.n[i] as f32 } else { f32::NAN }).collect());
        }
        self.grey.as_ref().unwrap()
    }
    /// Share of this level painted (area-weighted).
    pub fn coverage(&self) -> f64 {
        self.coverage_in(None)
    }
    /// The same, counting only the heights of the sky that can be seen at all (a gas giant's
    /// broad rings hide the rest: `rings::reachable`, by degree above the equator).
    pub fn coverage_in(&self, heights: Option<&[bool; 181]>) -> f64 {
        let (mut seen, mut all) = (0.0, 0.0);
        for row in 0..self.h {
            if heights.is_some_and(|ok| !ok[height_index(row, self.h)]) {
                continue;
            }
            let wgt = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            for col in 0..self.w {
                all += wgt;
                if self.n[row * self.w + col] > 0 {
                    seen += wgt;
                }
            }
        }
        if all == 0.0 {
            return 1.0;
        }
        seen / all
    }
}

/// A map row's height above the planet's equator, as an index into `rings::reachable` (0 … 180).
#[inline]
pub fn height_index(row: usize, h: usize) -> usize {
    let lat = (row as f64 + 0.5) / h as f64 * 180.0;
    ((90.0 - lat).round() + 90.0).clamp(0.0, 180.0) as usize
}

pub struct Sky {
    pub levels: [Level; 3],
    pub mirrored: bool,
    pub frames: u32,
}

impl Sky {
    pub fn new(mirrored: bool) -> Sky {
        Sky { levels: [Level::new(COARSE), Level::new(MEDIUM), Level::new(FINE)], mirrored, frames: 0 }
    }
    pub fn coverage(&self) -> f64 {
        self.levels[COARSE].coverage()
    }
    /// Where the biggest part of the sky not recorded yet is, seen from the camera at facing Q
    /// (the medium level: the stars): a few words for the pilot.
    pub fn missing_hint(&self, q: &M3, heights: Option<&[bool; 181]>) -> Option<String> {
        let m = &self.levels[MEDIUM];
        let (mut sx, mut sy, mut sz, mut n) = (0.0, 0.0, 0.0, 0.0);
        for row in (0..m.h).step_by(6) {
            if heights.is_some_and(|ok| !ok[height_index(row, m.h)]) {
                continue;
            }
            let lat = (row as f64 + 0.5) / m.h as f64 * PI;
            let wgt = lat.sin();
            for col in (0..m.w).step_by(6) {
                if m.n[row * m.w + col] > 0 {
                    continue;
                }
                let lon = (col as f64 + 0.5) / m.w as f64 * 2.0 * PI;
                // Sky direction → camera (s = Qᵀ·d, so d = Q·s); the camera looks along −z.
                let d = apply(q, &[lat.sin() * lon.cos(), lat.sin() * lon.sin(), lat.cos()]);
                sx += d[0] * wgt;
                sy += d[1] * wgt;
                sz += d[2] * wgt;
                n += wgt;
            }
        }
        if n == 0.0 {
            return None;
        }
        let l = (sx * sx + sy * sy + sz * sz).sqrt();
        if l < n * 0.15 {
            return Some("in patches all round: keep turning every way".into());
        }
        let v = [sx / l, sy / l, sz / l];
        if v[2] < -0.6 {
            return Some("straight behind the planet: turn a little either way to see past it".into());
        }
        if v[2] > 0.6 {
            return Some("behind you: turn the planet right round, to the opposite side".into());
        }
        let mut parts = Vec::new();
        if v[1] > 0.3 {
            parts.push("up");
        } else if v[1] < -0.3 {
            parts.push("down");
        }
        if v[0] > 0.3 {
            parts.push("right");
        } else if v[0] < -0.3 {
            parts.push("left");
        }
        if parts.is_empty() {
            return Some("in patches all round: keep turning every way".into());
        }
        Some(format!("{}: turn the view that way", parts.join(" and ")))
    }
    /// Paint a picture's sky at facing Q. Which levels: all by default.
    pub fn paint(&mut self, view: &View, q: &M3, levels: &[usize]) -> usize {
        let qt = transpose(q);
        let mut painted = 0;
        for &level in levels {
            let pts = points(view, level, SPECS[level].paint);
            let m = &mut self.levels[level];
            for p in &pts {
                let d = [p.d[0] as f64, p.d[1] as f64, p.d[2] as f64];
                let i = m.cell_of(&apply(&qt, &d));
                if m.n[i] < u16::MAX {
                    m.sum[i] += p.g;
                    m.n[i] += 1;
                }
            }
            m.grey = None;
            painted += pts.len();
        }
        self.frames += 1;
        painted
    }
    /// Paint ready-made points on one level (a start from nothing works from points alone).
    pub fn paint_points(&mut self, level: usize, pts: &[Pt], q: &M3) {
        let qt = transpose(q);
        let m = &mut self.levels[level];
        for p in pts {
            let d = [p.d[0] as f64, p.d[1] as f64, p.d[2] as f64];
            let i = m.cell_of(&apply(&qt, &d));
            if m.n[i] < u16::MAX {
                m.sum[i] += p.g;
                m.n[i] += 1;
            }
        }
        m.grey = None;
    }
    /// Bytes to keep: "SKY3" (a sky in its own frame: north the planet's axis, longitude 0 where
    /// the system's sky started), mirrored, then each level (w, h, a byte a cell; 0 = not seen).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"SKY3".to_vec();
        out.push(self.mirrored as u8);
        out.push(3);
        for (k, m) in self.levels.iter().enumerate() {
            out.extend_from_slice(&(m.w as u16).to_le_bytes());
            out.extend_from_slice(&(m.h as u16).to_le_bytes());
            let scale = SPECS[k].scale;
            out.extend(m.sum.iter().zip(&m.n).map(|(s, &n)| if n == 0 { 0 } else { (128.0 + scale * s / n as f32).round().clamp(1.0, 255.0) as u8 }));
        }
        out
    }
    /// Bytes from this scanner (SKY3), not an older one.
    pub fn is_current(bytes: &[u8]) -> bool {
        bytes.len() > 6 && &bytes[0..4] == b"SKY3"
    }
    /// From bytes ("SKY3"/"SKY2", or the old scanner's sky: its fine level is our medium one).
    pub fn from_bytes(bytes: &[u8]) -> Option<Sky> {
        if bytes.len() > 6 && (&bytes[0..4] == b"SKY3" || &bytes[0..4] == b"SKY2") {
            let mut sky = Sky::new(bytes[4] != 0);
            let mut at = 6;
            for k in 0..(bytes[5] as usize).min(3) {
                if bytes.len() < at + 4 {
                    return None;
                }
                let w = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
                let h = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
                at += 4;
                if w != SPECS[k].w || h != SPECS[k].h || bytes.len() < at + w * h {
                    return None;
                }
                let m = &mut sky.levels[k];
                for i in 0..w * h {
                    let b = bytes[at + i];
                    if b != 0 {
                        m.sum[i] = 4.0 * (b as f32 - 128.0) / SPECS[k].scale;
                        m.n[i] = 4;
                    }
                }
                at += w * h;
            }
            sky.frames = 1;
            return Some(sky);
        }
        // The old scanner: [1, mirrored, 1440 × 720, 360 × 180], value = (byte − 128) / 2.
        let (fw, cw) = (SPECS[MEDIUM].w * SPECS[MEDIUM].h, SPECS[COARSE].w * SPECS[COARSE].h);
        if bytes.len() == 2 + fw + cw && bytes[0] == 1 {
            let mut sky = Sky::new(bytes[1] == 1);
            for (level, at, len) in [(MEDIUM, 2, fw), (COARSE, 2 + fw, cw)] {
                let m = &mut sky.levels[level];
                for i in 0..len {
                    let b = bytes[at + i];
                    if b != 0 {
                        m.sum[i] = 4.0 * (b as f32 - 128.0) / 2.0;
                        m.n[i] = 4;
                    }
                }
            }
            sky.frames = 1;
            return Some(sky);
        }
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Matching. The camera has no roll, so a facing is a pitch and a yaw, and the yaw only spins the
// map round its axis: for one pitch each point's place on the map is worked out once and every
// yaw is just a shift in longitude.

#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub yaw: f64,
    pub pitch: f64,
    pub score: f64,
    pub seen: f64,
}

#[inline]
fn sample(m: &Level, g: &[f32], fy: f32, fx: f32, smooth: bool) -> f32 {
    let (w, h) = (m.w as i64, m.h as i64);
    if !smooth {
        let r = (fy.round() as i64).clamp(0, h - 1);
        let c = (fx.round() as i64).rem_euclid(w);
        return g[(r * w + c) as usize];
    }
    let r0 = (fy.floor() as i64).clamp(0, h - 2);
    let ty = (fy - r0 as f32).clamp(0.0, 1.0);
    let flo = fx.floor();
    let tx = fx - flo;
    let c0 = (flo as i64).rem_euclid(w);
    let c1 = if c0 + 1 == w { 0 } else { c0 + 1 };
    let (a, b, c, d) = (g[(r0 * w + c0) as usize], g[(r0 * w + c1) as usize], g[((r0 + 1) * w + c0) as usize], g[((r0 + 1) * w + c1) as usize]);
    if !(a.is_nan() || b.is_nan() || c.is_nan() || d.is_nan()) {
        return (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
    }
    let r = ((r0 as f32 + ty).round() as i64).min(h - 1);
    g[(r * w + (fx.round() as i64).rem_euclid(w)) as usize]
}

/// Scores (correlation, share seen) of yaws at one pitch. rings: skip points whose ray hits them.
fn scan_pitch(points: &[Pt], m: &Level, g: &[f32], pitch: f64, yaws: &[f64], min_seen: f64, rings: Option<(Rings, f64)>, smooth: bool) -> Vec<(f64, f64)> {
    let mm = mul(&transpose(&stand()), &rotation_about(&[1.0, 0.0, 0.0], -pitch));
    let axis = axis_for_pitch(pitch);
    let mut fy = Vec::with_capacity(points.len());
    let mut fx = Vec::with_capacity(points.len());
    let mut gs = Vec::with_capacity(points.len());
    for p in points {
        if let Some((rg, d)) = &rings {
            if rg.hit(&p.d, &axis, *d) {
                continue;
            }
        }
        let d = [p.d[0] as f64, p.d[1] as f64, p.d[2] as f64];
        let v = apply(&mm, &d);
        fy.push((v[2].clamp(-1.0, 1.0).acos() / PI * m.h as f64 - 0.5) as f32);
        fx.push((v[1].atan2(v[0]) / (2.0 * PI) * m.w as f64 - 0.5) as f32);
        gs.push(p.g);
    }
    // Shares count among the points that aren't behind the rings.
    let n = fy.len();
    let need = 30f64.max(n as f64 * min_seen);
    yaws.iter()
        .map(|&yaw| {
            let shift = (yaw / (2.0 * PI) * m.w as f64) as f32;
            let (mut c, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for i in 0..n {
                let v = sample(m, g, fy[i], fx[i] - shift, smooth);
                if v.is_nan() {
                    continue;
                }
                let (a, v) = (gs[i] as f64, v as f64);
                c += 1.0;
                sa += a;
                sb += v;
                saa += a * a;
                sbb += v * v;
                sab += a * v;
            }
            if c < need {
                return (f64::NAN, c / n.max(1) as f64);
            }
            let (va, vb) = (saa - sa * sa / c, sbb - sb * sb / c);
            if va <= 0.0 || vb <= 0.0 {
                return (f64::NAN, c / n as f64);
            }
            ((sab - sa * sb / c) / (va * vb).sqrt(), c / n as f64)
        })
        .collect()
}

/// Best facing near a guess on one level: a grid, then finer grids round the best.
pub fn align(points: &[Pt], sky: &mut Sky, level: usize, around: (f64, f64), span: f64, step: f64, min_seen: f64, rings: Option<(Rings, f64)>) -> Option<Found> {
    align_box(points, sky, level, around, (span, span), step, min_seen, rings)
}

/// As `align`, with its own reach in yaw and in pitch.
pub fn align_box(points: &[Pt], sky: &mut Sky, level: usize, around: (f64, f64), spans: (f64, f64), step: f64, min_seen: f64, rings: Option<(Rings, f64)>) -> Option<Found> {
    let smooth = level != COARSE;
    let m = &mut sky.levels[level];
    m.grey();
    let m = &sky.levels[level];
    let g = m.grey.as_ref().unwrap();
    let mut best: Option<Found> = None;
    let grid = |cy: f64, cp: f64, half: (i64, i64), st: f64, best: &mut Option<Found>| {
        let yaws: Vec<f64> = (-half.0..=half.0).map(|k| cy + k as f64 * st).collect();
        for j in -half.1..=half.1 {
            let pitch = cp + j as f64 * st;
            for (k, (score, seen)) in scan_pitch(points, m, g, pitch, &yaws, min_seen, rings, smooth).into_iter().enumerate() {
                if score.is_finite() && best.is_none_or(|b| score > b.score) {
                    *best = Some(Found { yaw: yaws[k], pitch, score, seen });
                }
            }
        }
    };
    grid(around.0, around.1, ((spans.0 / step).round() as i64, (spans.1 / step).round() as i64), step, &mut best);
    // Then three rounds of 3 × 3 at half the step each time (to an eighth of it).
    let mut st = step / 2.0;
    for _ in 0..3 {
        if let Some(b) = best {
            grid(b.yaw, b.pitch, (1, 1), st, &mut best);
        }
        st /= 2.0;
    }
    best
}

// ---------------------------------------------------------------------------------------------
// Starting a sky from nothing: no buildings, no other planet's sky.
//
// The first picture is put at yaw 0 (that picks where longitude 0 is: any place will do) and at a
// trial pitch. A sideways turn of the planet spins the sky round the planet's axis, so the later
// pictures only fit that first one at the right pitch; at a wrong one they'd need the camera to
// roll, which EVE's camera never does. So: every trial pitch, scored by how well the later
// pictures fit, coarse to fine.

/// What a start from nothing found.
#[derive(Clone, Debug)]
pub struct Boot {
    /// The first picture's pitch (the yaw is 0).
    pub pitch: f64,
    /// Each later picture's facing and fit (medium level); None: it didn't fit.
    pub later: Vec<Option<(f64, f64, f64)>>,
    /// Mean fit of the later pictures at the best pitch (coarse), and the best of any pitch more
    /// than 8° away: how clearly this pitch is the one.
    pub coarse: f64,
    pub other: f64,
    /// How far the pictures that fit turned from the first.
    pub turned: f64,
}

impl Boot {
    pub fn fitted(&self) -> usize {
        self.later.iter().filter(|f| f.is_some_and(|f| f.2 >= 0.5)).count()
    }
    /// Over 27 starts on three recordings, every one that turned 20° or more got the pitch within
    /// 0.3°; the score margin over other pitches is small even then (0.03 at least), so it's only a
    /// sanity check.
    pub fn sure(&self) -> bool {
        self.fitted() >= 3 && self.turned >= 20.0 * DEG && self.coarse - self.other >= 0.02
    }
}

/// One picture's points for a start from nothing: for matching and for painting, coarse and medium.
#[derive(Clone)]
pub struct BootFrame {
    pub match_coarse: Vec<Pt>,
    pub match_medium: Vec<Pt>,
    pub paint_coarse: Vec<Pt>,
    pub paint_medium: Vec<Pt>,
}

impl BootFrame {
    pub fn of(view: &View) -> BootFrame {
        BootFrame {
            match_coarse: match_points(view, COARSE),
            match_medium: match_points(view, MEDIUM),
            paint_coarse: points(view, COARSE, SPECS[COARSE].paint),
            paint_medium: points(view, MEDIUM, SPECS[MEDIUM].paint),
        }
    }
}

/// Points (the same grid place on the screen in every picture) whose value hardly changed over a
/// start's pictures while the sky turned past: dropped from every picture. A place counts when
/// it's in at least half the pictures; still when its spread is under 0.3 of the spread of all
/// values (the sky's places see different stars and nebula as it turns; blank sky drops too,
/// which costs nothing).
pub fn drop_still(frames: &mut [BootFrame]) {
    use std::collections::HashMap;
    let key = |p: &Pt| ((p.d[0] * 100_000.0) as i32, (p.d[1] * 100_000.0) as i32);
    let n = frames.len();
    if n < 4 {
        return;
    }
    fn list(f: &mut BootFrame, pick: usize) -> &mut Vec<Pt> {
        match pick {
            0 => &mut f.match_coarse,
            1 => &mut f.match_medium,
            2 => &mut f.paint_coarse,
            _ => &mut f.paint_medium,
        }
    }
    for pick in 0..4 {
        let mut by: HashMap<(i32, i32), (f64, f64, u32)> = HashMap::new();
        let (mut all, mut all2, mut count) = (0f64, 0f64, 0f64);
        for f in frames.iter_mut() {
            for p in list(f, pick).iter() {
                let e = by.entry(key(p)).or_insert((0.0, 0.0, 0));
                let g = p.g as f64;
                e.0 += g;
                e.1 += g * g;
                e.2 += 1;
                all += g;
                all2 += g * g;
                count += 1.0;
            }
        }
        if count < 10.0 {
            continue;
        }
        let spread = (all2 / count - (all / count).powi(2)).max(1e-9).sqrt();
        let still: std::collections::HashSet<(i32, i32)> = by
            .iter()
            .filter(|(_, &(s, s2, c))| c as usize * 2 >= n && ((s2 / c as f64 - (s / c as f64).powi(2)).max(0.0).sqrt()) < 0.3 * spread)
            .map(|(k, _)| *k)
            .collect();
        if still.is_empty() {
            continue;
        }
        for f in frames.iter_mut() {
            list(f, pick).retain(|p| !still.contains(&key(p)));
        }
    }
}

/// One trial pitch: the pictures in turn, each matched to all the sky placed so far (so the turn
/// can go on past where the first picture's sky ends) and then added to it. Mean fit, and each
/// picture's facing and fit.
fn boot_try(first: &BootFrame, later: &[BootFrame], pitch: f64, medium: bool, rings: Option<(Rings, f64)>) -> (f64, Vec<Option<(f64, f64, f64)>>) {
    let mut sky = Sky::new(false);
    let q0 = pose_q(0.0, pitch);
    // A gas giant: what would be ring at this trial pitch is left out of the painting (the
    // matching leaves it out at each pitch it tries).
    let clear = |pts: &[Pt], p: f64| -> Vec<Pt> {
        match &rings {
            Some((rg, d)) => {
                let n = axis_for_pitch(p);
                pts.iter().copied().filter(|q| !rg.hit(&q.d, &n, *d)).collect()
            }
            None => pts.to_vec(),
        }
    };
    sky.paint_points(COARSE, &clear(&first.paint_coarse, pitch), &q0);
    if medium {
        sky.paint_points(MEDIUM, &clear(&first.paint_medium, pitch), &q0);
    }
    let mut at = (0.0, pitch);
    let mut total = 0.0;
    let mut out = Vec::with_capacity(later.len());
    for f in later {
        let coarse = align_box(&f.match_coarse, &mut sky, COARSE, at, (25.0 * DEG, 10.0 * DEG), 2.5 * DEG, 0.25, rings);
        let found = match (coarse, medium) {
            (Some(c), true) if c.score >= 0.3 => align_box(&f.match_medium, &mut sky, MEDIUM, (c.yaw, c.pitch), (1.5 * DEG, 1.5 * DEG), 0.5 * DEG, 0.25, rings),
            (c, false) => c,
            _ => None,
        };
        match found {
            Some(fd) if fd.score.is_finite() && fd.score >= if medium { 0.45 } else { 0.35 } => {
                total += fd.score;
                at = (fd.yaw, fd.pitch);
                let q = pose_q(fd.yaw, fd.pitch);
                sky.paint_points(COARSE, &clear(&f.paint_coarse, fd.pitch), &q);
                if medium {
                    sky.paint_points(MEDIUM, &clear(&f.paint_medium, fd.pitch), &q);
                }
                out.push(Some((fd.yaw, fd.pitch, fd.score)));
            }
            _ => out.push(None),
        }
    }
    (total / later.len().max(1) as f64, out)
}

/// The first picture's pitch from the later ones (the planet turned sideways in between).
/// rings: a gas giant's (or everything that could be), left out at each trial pitch: rings turn
/// with the planet, not with the sky, and would fit "not turned" best.
pub fn boot(first: &BootFrame, later: &[BootFrame], rings: Option<(Rings, f64)>) -> Option<Boot> {
    if later.is_empty() {
        return None;
    }
    // What stayed put on the screen while the planet turned isn't sky: a gas giant's rings (round
    // the axis the camera turns on) or a window the mask doesn't know yet. Left out of all of them.
    let mut frames: Vec<BootFrame> = std::iter::once(first).chain(later.iter()).map(|f| f.clone()).collect();
    drop_still(&mut frames);
    let (first, later) = frames.split_first()?;
    // How far the fitted pictures turned from the first. A trial pitch whose fit says "hardly
    // turned" is wrong: what fitted there is what turns with the planet (a gas giant's rings, a
    // window the mask missed), not the sky, so it scores half.
    let turned_of = |pitch: f64, poses: &[Option<(f64, f64, f64)>]| poses.iter().flatten().map(|&(y, p, _)| super::geom::apart((0.0, pitch), (y, p))).fold(0.0, f64::max);
    let judged = |pitch: f64, (score, poses): (f64, Vec<Option<(f64, f64, f64)>>)| {
        let turned = turned_of(pitch, &poses);
        (if turned < 8.0 * DEG { score * 0.5 } else { score }, poses)
    };
    // Every 5°, coarse.
    let trials: Vec<(f64, f64)> = (-17..=17).map(|k| {
        let p = k as f64 * 5.0 * DEG;
        (p, judged(p, boot_try(first, later, p, false, rings)).0)
    }).collect();
    let (p0, coarse) = trials.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let other = trials.iter().filter(|t| (t.0 - p0).abs() > 8.0 * DEG).map(|t| t.1).fold(0.0, f64::max);
    // Then 1° round the best, then 0.2°, on the medium level.
    let mut best: (f64, f64, Vec<Option<(f64, f64, f64)>>) = (p0, f64::NEG_INFINITY, Vec::new());
    for (span, step) in [(5.0, 1.0), (1.0, 0.2)] {
        let centre = best.0;
        let n = (span / step) as i64;
        for k in -n..=n {
            let p = centre + k as f64 * step * DEG;
            let (score, poses) = judged(p, boot_try(first, later, p, true, rings));
            if score > best.1 {
                best = (p, score, poses);
            }
        }
    }
    let turned = best.2.iter().flatten().map(|&(y, p, _)| super::geom::apart((0.0, best.0), (y, p))).fold(0.0, f64::max);
    Some(Boot { pitch: best.0, later: best.2, coarse, other, turned })
}

/// Share of the points that land on recorded sky at a facing (no score).
pub fn seen_at(points: &[Pt], sky: &Sky, level: usize, q: &M3) -> f64 {
    let m = &sky.levels[level];
    let qt = transpose(q);
    let mut seen = 0;
    for p in points {
        let d = [p.d[0] as f64, p.d[1] as f64, p.d[2] as f64];
        if m.n[m.cell_of(&apply(&qt, &d))] > 0 {
            seen += 1;
        }
    }
    seen as f64 / points.len().max(1) as f64
}

#[derive(Clone, Copy, Debug)]
pub struct Find {
    pub yaw: f64,
    pub pitch: f64,
    /// Coarse match (the nebula's shapes).
    pub coarse: f64,
    /// Star match (the finer level that had sky recorded there), and which level it was.
    pub stars: f64,
    pub level: usize,
    pub margin: f64,
}

impl Find {
    pub fn sure(&self) -> bool {
        self.coarse >= 0.5 && self.stars >= 0.4 && self.margin >= 0.3
    }
}

/// Where the camera faces from the sky alone (no guess): the whole sky searched every 2° on
/// the coarse map, the best few refined on the finer ones.
pub fn find(view: &View, sky: &mut Sky, rings: Option<(Rings, f64)>) -> Option<Find> {
    let coarse = match_points(view, COARSE);
    if coarse.len() < 40 {
        return None;
    }
    let every = coarse.len().div_ceil(400).max(1);
    let using: Vec<Pt> = coarse.iter().copied().step_by(every).collect();
    let step = 2.0 * DEG;
    let m = &mut sky.levels[COARSE];
    m.grey();
    let m = &sky.levels[COARSE];
    let g = m.grey.as_ref().unwrap();
    let w = m.w as i64;
    let mm_base = transpose(&stand());
    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    let mut row_at = vec![0usize; using.len()];
    let mut col_at = vec![0i64; using.len()];
    let mut keep = vec![true; using.len()];
    let mut pitch = -PI;
    while pitch < PI - 1e-9 {
        let mm = mul(&mm_base, &rotation_about(&[1.0, 0.0, 0.0], -pitch));
        let axis = axis_for_pitch(pitch);
        let mut kept = 0;
        for (i, p) in using.iter().enumerate() {
            keep[i] = !rings.is_some_and(|(rg, d)| rg.hit(&p.d, &axis, d));
            if !keep[i] {
                continue;
            }
            kept += 1;
            let v = apply(&mm, &[p.d[0] as f64, p.d[1] as f64, p.d[2] as f64]);
            row_at[i] = ((v[2].clamp(-1.0, 1.0).acos() / PI * m.h as f64).floor() as usize).min(m.h - 1) * m.w;
            col_at[i] = (((v[1].atan2(v[0]) / (2.0 * PI) + 1.0) * m.w as f64).floor() as i64).rem_euclid(w) + w * 4;
        }
        let need = 30f64.max(kept as f64 * 0.5);
        let mut yaw = -PI;
        while yaw < PI - 1e-9 {
            let shift = (yaw / (2.0 * PI) * m.w as f64).round() as i64;
            let (mut c, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for i in 0..using.len() {
                if !keep[i] {
                    continue;
                }
                let v = g[row_at[i] + ((col_at[i] - shift).rem_euclid(w)) as usize];
                if v.is_nan() {
                    continue;
                }
                let (a, v) = (using[i].g as f64, v as f64);
                c += 1.0;
                sa += a;
                sb += v;
                saa += a * a;
                sbb += v * v;
                sab += a * v;
            }
            if c >= need {
                let (va, vb) = (saa - sa * sa / c, sbb - sb * sb / c);
                if va > 0.0 && vb > 0.0 {
                    found.push((yaw, pitch, (sab - sa * sb / c) / (va * vb).sqrt()));
                }
            }
            yaw += step;
        }
        pitch += step;
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    let axis_of = |y: f64, p: f64| apply(&pose_q(y, p), &[0.0, 0.0, 1.0]);
    let ang = |a: V3, b: V3| (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0).acos();
    let mut tops: Vec<(f64, f64, f64)> = Vec::new();
    for f in &found {
        if tops.len() >= 4 {
            break;
        }
        if tops.iter().all(|t| ang(axis_of(t.0, t.1), axis_of(f.0, f.1)) > 0.15) {
            tops.push(*f);
        }
    }
    let medium = match_points(view, MEDIUM);
    let fine = match_points(view, FINE);
    let mut best: Option<Find> = None;
    for t in &tops {
        let Some(c) = align(&coarse, sky, COARSE, (t.0, t.1), step, step / 2.0, 0.5, rings) else { continue };
        let (mut stars, mut level, mut at) = (0.0, COARSE, (c.yaw, c.pitch));
        if let Some(r) = align(&medium, sky, MEDIUM, (c.yaw, c.pitch), 0.75 * DEG, 0.25 * DEG, 0.3, rings) {
            stars = r.score;
            level = MEDIUM;
            at = (r.yaw, r.pitch);
        }
        if let Some(r) = align(&fine, sky, FINE, at, 0.375 * DEG, 0.125 * DEG, 0.3, rings) {
            if r.score > stars || level == COARSE {
                stars = r.score;
                level = FINE;
                at = (r.yaw, r.pitch);
            }
        }
        let out = Find { yaw: at.0, pitch: at.1, coarse: c.score, stars, level, margin: 0.0 };
        if best.is_none_or(|b| out.coarse + out.stars > b.coarse + b.stars) {
            best = Some(out);
        }
    }
    let mut best = best?;
    let other = found.iter().find(|f| ang(axis_of(f.0, f.1), axis_of(best.yaw, best.pitch)) > 0.25).map(|f| f.2).unwrap_or(0.0);
    best.margin = best.coarse - other;
    Some(best)
}

/// Following: the facing near the last one (coarse ±3°, wider if that misses, then the finer
/// levels). Returns (yaw, pitch, coarse score, star score, star level, share seen) or None.
pub struct Follow {
    pub yaw: f64,
    pub pitch: f64,
    pub coarse: f64,
    pub stars: f64,
    pub level: usize,
    pub seen: f64,
}

/// Following at full speed: the fine level only, near a predicted facing (the last one plus the
/// last turn). None when it isn't clearly right there (then `follow`).
pub fn follow_fast(view: &View, sky: &mut Sky, predicted: (f64, f64), rings: Option<(Rings, f64)>) -> Option<Follow> {
    let mut fine = match_points(view, FINE);
    if fine.len() > 4000 {
        let keep = fine.len() / 4000 + 1;
        fine = fine.into_iter().step_by(keep).collect();
    }
    let r = align(&fine, sky, FINE, predicted, 0.25 * DEG, 0.125 * DEG, 0.5, rings)?;
    if r.score < 0.45 || r.seen < 0.5 || (r.yaw - predicted.0).abs() > 0.24 * DEG && (r.pitch - predicted.1).abs() > 0.24 * DEG {
        return None;
    }
    Some(Follow { yaw: r.yaw, pitch: r.pitch, coarse: f64::NAN, stars: r.score, level: FINE, seen: r.seen })
}

pub fn follow(view: &View, sky: &mut Sky, around: (f64, f64), rings: Option<(Rings, f64)>) -> Result<Follow, f64> {
    let coarse = match_points(view, COARSE);
    let mut c = align(&coarse, sky, COARSE, around, 3.0 * DEG, 1.0 * DEG, 0.3, rings);
    if c.is_none_or(|c| c.score < 0.45) {
        if let Some(wide) = align(&coarse, sky, COARSE, around, 10.0 * DEG, 1.0 * DEG, 0.3, rings) {
            if wide.score >= 0.5 {
                c = Some(wide);
            }
        }
    }
    let Some(c) = c.filter(|c| c.score >= 0.45) else {
        // How much of the view is recorded sky (low: new sky, not a disagreement).
        return Err(c.map(|c| c.seen).unwrap_or(0.0));
    };
    let mut out = Follow { yaw: c.yaw, pitch: c.pitch, coarse: c.score, stars: 0.0, level: COARSE, seen: c.seen };
    let medium = match_points(view, MEDIUM);
    if let Some(r) = align(&medium, sky, MEDIUM, (c.yaw, c.pitch), 0.5 * DEG, 0.25 * DEG, 0.3, rings) {
        if r.score >= 0.25 {
            out.yaw = r.yaw;
            out.pitch = r.pitch;
            out.stars = r.score;
            out.level = MEDIUM;
        }
    }
    let fine = match_points(view, FINE);
    if let Some(r) = align(&fine, sky, FINE, (out.yaw, out.pitch), 0.25 * DEG, 0.125 * DEG, 0.3, rings) {
        if r.score >= 0.25 && (r.score >= out.stars * 0.8 || out.level == COARSE) {
            out.yaw = r.yaw;
            out.pitch = r.pitch;
            out.stars = r.score;
            out.level = FINE;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// The planet's edge.

/// Fully zoomed out, every planet fills the same angle on screen: its edge sits about 0.26 lens
/// lengths from the middle (196 px at 1280 wide, 289 px at 1914, on barren, lava, oceanic and gas).
pub const ZOOMED_OUT_RIM: f64 = 0.26;

/// EVE always puts the planet in the middle of the window, so only the radius is unknown. Along
/// 360 rays from the middle, every place where the picture steps down (brighter inside than
/// outside) votes for its radius; the radius most rays agree on is the planet's edge. A window's
/// straight edge or a nebula's patch crosses the rays at all different radii, so it can't win.
/// Searched around where a zoomed-out planet's edge is (or within ±15 % of `near`), skipping
/// EVE's windows when they're known.
pub fn rim_radius(pic: &Pic, near: Option<f64>, lens: f64, ui: Option<&UiMask>) -> Option<f64> {
    let (cx, cy) = (pic.w as f64 / 2.0, pic.h as f64 / 2.0);
    let r0 = near.unwrap_or(ZOOMED_OUT_RIM * lens);
    let (lo, hi) = match near {
        Some(n) => (n * 0.85, n * 1.15),
        None => (r0 * 0.6, r0 * 1.5),
    };
    let step = (r0 / 200.0).max(1.0);
    let bin = r0 * 0.005;
    let bins = ((hi - lo) / bin).ceil() as usize + 1;
    let mut votes = vec![0.0f64; bins];
    let ui = ui.filter(|u| u.w == pic.w && u.h == pic.h && u.ready());
    const SIDE: usize = 4;
    const RAYS: usize = 360;
    let mut rays_used = 0usize;
    let mut prof: Vec<f64> = Vec::new();
    for i in 0..RAYS {
        let a = i as f64 / RAYS as f64 * 2.0 * PI;
        let (dx, dy) = (a.cos(), a.sin());
        prof.clear();
        let first = lo - (SIDE as f64 + 1.0) * step;
        let mut r = first;
        while r <= hi + (SIDE as f64 + 1.0) * step {
            let (x, y) = (cx + dx * r, cy + dy * r);
            let v = if x < 0.0 || y < 0.0 || x >= pic.w as f64 || y >= pic.h as f64 || ui.is_some_and(|u| u.blocked(x as usize, y as usize)) {
                f64::NAN
            } else {
                pic.lum(x as usize, y as usize) as f64
            };
            prof.push(v);
            r += step;
        }
        if prof.len() < 2 * SIDE + 3 {
            continue;
        }
        // Inside minus outside, over SIDE samples each side.
        let mut g = vec![f64::NAN; prof.len()];
        for k in SIDE..prof.len() - SIDE {
            let inside: f64 = prof[k - SIDE..k].iter().sum();
            let outside: f64 = prof[k + 1..=k + SIDE].iter().sum();
            g[k] = (inside - outside) / SIDE as f64;
        }
        let mut peaks: Vec<(f64, f64)> = Vec::new();
        for k in SIDE + 1..prof.len() - SIDE - 1 {
            let v = g[k];
            if v.is_nan() || v < 10.0 || !(v >= g[k - 1]) || !(v > g[k + 1]) {
                continue;
            }
            let rr = first + k as f64 * step;
            if rr < lo || rr > hi {
                continue;
            }
            peaks.push((v, rr));
        }
        if g.iter().any(|v| !v.is_nan()) {
            rays_used += 1;
        }
        peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(v, rr) in peaks.iter().take(3) {
            let b = ((rr - lo) / bin).round() as usize;
            if b < bins {
                votes[b] += (v / 40.0).min(1.0);
            }
        }
    }
    if rays_used < RAYS / 4 {
        return None;
    }
    // Smooth over ±2 bins (±1 % of the radius) and take the best.
    let smooth: Vec<f64> = (0..bins).map(|b| (b.saturating_sub(2)..(b + 3).min(bins)).map(|k| votes[k]).sum()).collect();
    let (best, &score) = smooth.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if score < rays_used as f64 * 0.15 {
        return None;
    }
    // Centre of the votes around the best bin.
    let (mut sw, mut sr) = (0.0, 0.0);
    for k in best.saturating_sub(2)..(best + 3).min(bins) {
        sw += votes[k];
        sr += votes[k] * (lo + k as f64 * bin);
    }
    // The visible edge, glow and all: the size the old scanner lined buildings up with.
    Some(sr / sw)
}

/// Is the edge still where the disc says (not zoomed past it)? The strongest brightness step
/// along rays near the rim, whichever way it goes.
pub fn rim_still_there(pic: &Pic, disc: &Disc) -> bool {
    let (mut rays, mut agree) = (0, 0);
    for i in 0..72 {
        let a = i as f64 / 72.0 * 2.0 * PI;
        let (dx, dy) = (a.cos(), a.sin());
        let mut prof: Vec<(f64, f64)> = Vec::new();
        let mut r = disc.r * 0.8;
        let mut ok = true;
        while r <= disc.r * 1.2 {
            let (x, y) = (disc.cx + dx * r, disc.cy + dy * r);
            if x < 1.0 || y < 1.0 || x >= pic.w as f64 - 1.0 || y >= pic.h as f64 - 1.0 {
                ok = false;
                break;
            }
            prof.push((r, pic.lum(x as usize, y as usize) as f64));
            r += (disc.r * 0.01).max(1.0);
        }
        if !ok || prof.len() < 12 {
            continue;
        }
        rays += 1;
        let (mut best, mut at) = (0.0, 0.0);
        for k in 2..prof.len() - 2 {
            let step = (prof[k + 2].1 + prof[k + 1].1 - prof[k - 1].1 - prof[k - 2].1).abs() / 2.0;
            if step > best {
                best = step;
                at = prof[k].0;
            }
        }
        if best > 10.0 && (at - disc.r).abs() < disc.r * 0.06 {
            agree += 1;
        }
    }
    rays >= 12 && agree as f64 >= rays as f64 * 0.35
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_bytes_round_trip() {
        let mut sky = Sky::new(true);
        for (k, m) in sky.levels.iter_mut().enumerate() {
            for i in (0..m.sum.len()).step_by(97) {
                m.sum[i] = ((i % 13) as f32 - 6.0) * 0.3 * (k + 1) as f32;
                m.n[i] = 1;
            }
        }
        let back = Sky::from_bytes(&sky.to_bytes()).unwrap();
        assert!(back.mirrored);
        for k in 0..3 {
            let (a, b) = (&sky.levels[k], &back.levels[k]);
            for i in (0..a.sum.len()).step_by(97) {
                let (va, vb) = (a.sum[i] / a.n[i] as f32, b.sum[i] / b.n[i] as f32);
                assert!((va - vb).abs() <= 0.5 / SPECS[k].scale + 1e-4, "level {k} cell {i}: {va} vs {vb}");
            }
        }
        assert!((back.coverage() - sky.coverage()).abs() < 1e-9);
    }

    #[test]
    fn old_scanner_sky_reads_as_medium_and_coarse() {
        let (fw, cw) = (1440 * 720, 360 * 180);
        let mut bytes = vec![0u8; 2 + fw + cw];
        bytes[0] = 1;
        bytes[1] = 1;
        bytes[2 + 5] = 140; // medium cell 5: +6
        bytes[2 + fw + 7] = 120; // coarse cell 7: −4
        let sky = Sky::from_bytes(&bytes).unwrap();
        assert!(sky.mirrored);
        assert_eq!(sky.levels[MEDIUM].sum[5] / sky.levels[MEDIUM].n[5] as f32, 6.0);
        assert_eq!(sky.levels[COARSE].sum[7] / sky.levels[COARSE].n[7] as f32, -4.0);
        assert_eq!(sky.levels[FINE].n.iter().filter(|&&n| n > 0).count(), 0);
    }

    /// A lit disc of the zoomed-out size, half of it in shadow, with a bright window over one
    /// side and a bright patch of nebula: the edge is found within 1 %. Zoomed in: nothing.
    fn planet_picture(w: usize, h: usize, r: f64) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - w as f64 / 2.0, y as f64 - h as f64 / 2.0);
                let mut v = ((x * 7 + y * 13) % 11) as u8; // speckle
                if (dx * dx + dy * dy).sqrt() < r {
                    v = if dx > -0.3 * r { 150 } else { 12 };
                }
                if x > w * 3 / 4 && y < h / 3 {
                    v = 200; // a window
                }
                if (x as f64 - 0.2 * w as f64).hypot(y as f64 - 0.8 * h as f64) < 60.0 {
                    v = v.max(90); // nebula
                }
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        px
    }

    #[test]
    fn rim_found_at_the_zoomed_out_size() {
        let (w, h) = (1280, 683);
        let lens = 750.0;
        let r = ZOOMED_OUT_RIM * lens;
        let px = planet_picture(w, h, r);
        let got = rim_radius(&Pic { w, h, bgra: &px }, None, lens, None).expect("found");
        assert!((got - r).abs() < r * 0.01, "got {got}, want {r}");
        let near = rim_radius(&Pic { w, h, bgra: &px }, Some(got), lens, None).expect("found near");
        assert!((near - r).abs() < r * 0.01);
        let zoomed = planet_picture(w, h, r * 2.2);
        assert!(rim_radius(&Pic { w, h, bgra: &zoomed }, None, lens, None).is_none());
    }

    /// A slow turn (0.5° between looks) past gentle, bright nebula: between two looks it barely
    /// changes, so only a longer comparison tells it from a window. The nebula and the planet must
    /// stay sky; a window (sharp text on a flat panel) must be learnt.
    #[test]
    fn windows_learnt_without_the_nebula_or_the_planet() {
        let (w, h) = (640usize, 360usize);
        let disc = Disc::with_lens(320.0, 180.0, 94.0, 375.0);
        let mut ui = UiMask::new(w, h);
        // Nothing is learnt before the planet's edge is known.
        let blank = vec![0u8; w * h * 4];
        assert!(!ui.learn(&Pic { w, h, bgra: &blank }, None));
        for k in 0..120 {
            let shift = k as f64 * 10.0; // px a look: a slow turn
            let mut px = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let sx = x as f64 + shift;
                    // nebula: broad and bright; stars: sharp dots on a grid that moves with it
                    let neb = 60.0 + 15.0 * ((sx / 150.0).sin() * (y as f64 / 120.0).cos());
                    // stars: soft dots scattered over 6 px cells (a hash picks which cells have one,
                    // and where in it), moving with the sky
                    let (cx6, cy6) = ((sx / 6.0).floor() as i64, (y / 6) as i64);
                    let hash = (cx6.wrapping_mul(73_856_093) ^ cy6.wrapping_mul(19_349_663)).rem_euclid(1009);
                    let star = if hash < 220 {
                        let (ox, oy) = (cx6 as f64 * 6.0 + 1.5 + (hash % 3) as f64, cy6 as f64 * 6.0 + 1.5 + (hash % 5) as f64 * 0.6);
                        let d2 = (sx - ox).powi(2) + (y as f64 - oy).powi(2);
                        200.0 * (-d2 / 1.5).exp()
                    } else {
                        0.0
                    };
                    let mut v = neb + star;
                    let (dx, dy) = (x as f64 - disc.cx, y as f64 - disc.cy);
                    if dx * dx + dy * dy < disc.r * disc.r {
                        v = 140.0 + 5.0 * ((x as f64 + shift * 0.3) / 60.0).sin(); // the planet, smooth
                    }
                    if x < 96 && y < 150 {
                        v = if (x % 9 < 2 && y % 11 < 6) || y % 40 == 0 { 230.0 } else { 70.0 }; // a window
                    }
                    let v = v.clamp(0.0, 255.0) as u8;
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
            ui.learn(&Pic { w, h, bgra: &px }, Some(&disc));
        }
        assert!(ui.ready());
        let share = |x0: usize, x1: usize, y0: usize, y1: usize| {
            let (mut n, mut b) = (0, 0);
            for y in (y0..y1).step_by(4) {
                for x in (x0..x1).step_by(4) {
                    n += 1;
                    b += ui.blocked(x, y) as usize;
                }
            }
            b as f64 / n as f64
        };
        assert!(share(8, 88, 8, 142) > 0.8, "window {}", share(8, 88, 8, 142));
        assert!(share(260, 380, 120, 240) == 0.0, "planet {}", share(260, 380, 120, 240));
        assert!(share(450, 640, 0, 360) < 0.05, "nebula {}", share(450, 640, 0, 360));
        assert!(ui.share() < 0.35);
    }
}
