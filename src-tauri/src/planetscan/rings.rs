//! A gas giant's rings, measured instead of guessed (VII's are thin, about 1.6–1.9 planet radii;
//! VIII's are broad and reach past 4.6). The rings lie in the planet's equator, so they're round
//! about the axis the camera turns on: while the planet turns sideways they stay put on the
//! screen, as EVE's windows do, and the stars behind them move. Each such look, with the facing
//! known, turns every textured pixel outside the planet into a vote at the distance where its ray
//! crosses the ring plane: "still" (ring) or "moved" (sky). Distances that mostly vote still are
//! cut out of the sky at every facing; the gaps between bands, where stars move, stay in.
//!
//! The same cut says which parts of the sky can be seen at all: past broad rings the sky beyond
//! the planet's poles can only be looked at through them, so the sky counts as complete at 99.9 %
//! of what can be seen.

use super::geom::{Disc, V3};
use super::sky::{axis_for_pitch, Pic, UiMask, DEG, GLOW};

/// Distances in planet radii: bins of 0.025 from 1.0 (400 bins, out to 11 radii).
pub const RING_FROM: f64 = 1.0;
pub const RING_STEP: f64 = 0.025;
pub const RING_BINS: usize = 400;

/// Until a gas giant's rings are measured: anything that could be ring (out to 7 planet radii) is
/// left out. Near the ring plane that still leaves the sky above it.
pub const RINGS_UNKNOWN: (f64, f64) = (1.0, 7.0);

/// Which distances in the ring plane are ring (cut out of the sky), and how far either way in
/// pitch the cut is also made (while the pitch isn't known well: a start from nothing tries
/// pitches a few degrees apart, and a broad ring's texture just past the cut would fit "not
/// turned" at a slightly wrong one).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rings {
    bits: [u64; 7],
    spread: f64,
}

impl Default for Rings {
    fn default() -> Self {
        Rings::none()
    }
}

/// Where a camera ray crosses the ring plane, in planet radii from the middle (None: it doesn't,
/// in front of the camera). n = the planet's axis in camera space, d = the camera's distance.
#[inline]
pub fn plane_radius(ray: &[f32; 3], n: &V3, d: f64) -> Option<f64> {
    let nd = n[0] * ray[0] as f64 + n[1] * ray[1] as f64 + n[2] * ray[2] as f64;
    if nd.abs() < 1e-9 {
        return None;
    }
    let t = -d * n[2] / nd;
    if t <= 0.0 {
        return None;
    }
    let p = [t * ray[0] as f64, t * ray[1] as f64, d + t * ray[2] as f64];
    Some((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt())
}

#[inline]
fn bin_of(r: f64) -> Option<usize> {
    if r < RING_FROM {
        return None;
    }
    let b = ((r - RING_FROM) / RING_STEP) as usize;
    (b < RING_BINS).then_some(b)
}

impl Rings {
    pub fn none() -> Rings {
        Rings { bits: [0; 7], spread: 0.0 }
    }
    /// The same cut, also made a pitch `spread` (radians) either way.
    pub fn with_spread(&self, spread: f64) -> Rings {
        Rings { bits: self.bits, spread }
    }
    /// Rings from r1 to r2 planet radii (tests, and pictures made up for them).
    pub fn band(r1: f64, r2: f64) -> Rings {
        let mut out = Rings::none();
        for b in 0..RING_BINS {
            let r = RING_FROM + (b as f64 + 0.5) * RING_STEP;
            if r >= r1 && r <= r2 {
                out.set(b);
            }
        }
        out
    }
    fn set(&mut self, b: usize) {
        self.bits[b / 64] |= 1 << (b % 64);
    }
    #[inline]
    fn get(&self, b: usize) -> bool {
        self.bits[b / 64] & (1 << (b % 64)) != 0
    }
    pub fn any(&self) -> bool {
        self.bits.iter().any(|&w| w != 0)
    }
    /// Both cuts together.
    pub fn union(&self, other: &Rings) -> Rings {
        let mut out = *self;
        for (a, b) in out.bits.iter_mut().zip(other.bits.iter()) {
            *a |= *b;
        }
        out.spread = self.spread.max(other.spread);
        out
    }
    #[inline]
    pub fn cut_at(&self, r: f64) -> bool {
        bin_of(r).is_some_and(|b| self.get(b))
    }
    /// Does a camera ray end on the rings (n = the planet's axis in camera space, d = distance)?
    /// With a spread, also at the pitch that much either way.
    #[inline]
    pub fn hit(&self, ray: &[f32; 3], n: &V3, d: f64) -> bool {
        if !self.any() {
            return false;
        }
        if plane_radius(ray, n, d).is_some_and(|r| self.cut_at(r)) {
            return true;
        }
        if self.spread > 0.0 {
            let pitch = n[2].atan2(n[1]);
            for p in [pitch - self.spread, pitch + self.spread, pitch - self.spread / 2.0, pitch + self.spread / 2.0] {
                let m = [0.0, p.cos(), p.sin()];
                if plane_radius(ray, &m, d).is_some_and(|r| self.cut_at(r)) {
                    return true;
                }
            }
        }
        false
    }
    /// The bands, (from, to) in planet radii.
    pub fn bands(&self) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut start: Option<usize> = None;
        for b in 0..=RING_BINS {
            let on = b < RING_BINS && self.get(b);
            match (on, start) {
                (true, None) => start = Some(b),
                (false, Some(s)) => {
                    out.push((RING_FROM + s as f64 * RING_STEP, RING_FROM + b as f64 * RING_STEP));
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
}

/// Votes for minimum: a distance with fewer is unknown (not cut, unless between bands).
const MIN_VOTES: u32 = 20;
/// A distance is ring when this share of its votes is "still" (the sky's almost never are)…
const RING_SHARE: f64 = 0.3;
/// …all round the planet: votes are kept by direction from the planet's middle (16 sectors). A
/// ring is round, so it's still in most sectors that see it; a window EVE draws (or a pop-up the
/// windows mask doesn't know) sits in two or three.
const SECTORS: usize = 16;
const SECTOR_VOTES: u32 = 4;
const RING_SECTORS: usize = 4;
/// Bins of margin each side of a ring band (0.1 planet radii).
const MARGIN: usize = 4;
/// Votes in all before the rings count as measured (a planet with none measures as "none").
const MEASURED_VOTES: u64 = 6_000;

/// The votes (by distance and sector), and the last look they're compared with.
pub struct RingLearner {
    still: Vec<u32>,
    moved: Vec<u32>,
    prev: Option<Look>,
    pub looks: u32,
    /// Tests: each half-size pixel's vote (1 moved, 2 still) from the last look.
    pub debug: Option<Vec<u8>>,
}

/// A look kept for the next comparison: its fine detail at half size (brightness minus the 3 × 3
/// round it, so stars and stripes count and smooth nebula doesn't), and its facing.
struct Look {
    w: usize,
    h: usize,
    detail: Vec<i16>,
    yaw: f64,
    pitch: f64,
}

/// Half size (each pixel the mean of its 2 × 2), then the detail: minus the mean of the 3 × 3 round.
fn detail_of(pic: &Pic) -> (usize, usize, Vec<i16>) {
    let (w2, h2) = (pic.w / 2, pic.h / 2);
    let mut lum = vec![0i32; w2 * h2];
    for y in 0..h2 {
        for x in 0..w2 {
            lum[y * w2 + x] = ((pic.lum(2 * x, 2 * y) + pic.lum(2 * x + 1, 2 * y) + pic.lum(2 * x, 2 * y + 1) + pic.lum(2 * x + 1, 2 * y + 1)) / 4) as i32;
        }
    }
    let mut out = vec![0i16; w2 * h2];
    for y in 1..h2.saturating_sub(1) {
        for x in 1..w2 - 1 {
            let mut acc = 0;
            for yy in y - 1..=y + 1 {
                for xx in x - 1..=x + 1 {
                    acc += lum[yy * w2 + xx];
                }
            }
            out[y * w2 + x] = (9 * lum[y * w2 + x] - acc) as i16;
        }
    }
    (w2, h2, out)
}

/// Patches 5 × 5 (half size) for comparing; one in every 2 × 2 pixels is tried.
const PATCH: i64 = 2;
/// The same patch: correlation at least this (the sky's patches, rearranged stars, fall far below).
const SAME: f64 = 0.7;
/// Clearly not the same: under this (in between doesn't vote).
const NOT_SAME: f64 = 0.35;

impl Default for RingLearner {
    fn default() -> Self {
        RingLearner::new()
    }
}

impl RingLearner {
    pub fn new() -> RingLearner {
        RingLearner { still: vec![0; RING_BINS * SECTORS], moved: vec![0; RING_BINS * SECTORS], prev: None, looks: 0, debug: None }
    }
    pub fn votes(&self) -> u64 {
        self.still.iter().chain(&self.moved).map(|&v| v as u64).sum()
    }
    pub fn measured(&self) -> bool {
        self.votes() >= MEASURED_VOTES && self.looks >= 6
    }
    /// One picture at a known facing. Compared with the last kept one when the camera turned
    /// sideways (pitch the same within 0.15°) by at least 1°: then the stars have moved a few
    /// pixels everywhere and anything still is the planet's. True when it voted.
    pub fn learn(&mut self, pic: &Pic, disc: &Disc, lens: f64, yaw: f64, pitch: f64, windows: Option<&UiMask>) -> bool {
        if pic.w < 32 || pic.h < 32 {
            return false;
        }
        let fresh = |pic: &Pic| {
            let (_, _, detail) = detail_of(pic);
            Some(Look { w: pic.w, h: pic.h, detail, yaw, pitch })
        };
        let Some(prev) = self.prev.as_ref() else {
            self.prev = fresh(pic);
            return false;
        };
        if (prev.w, prev.h) != (pic.w, pic.h) || (prev.pitch - pitch).abs() > 3.0 * DEG || pitch.abs() < 3.0 * DEG {
            // Tilted a lot (the rings look different), or edge-on: start again from here.
            self.prev = fresh(pic);
            return false;
        }
        let turned = {
            let d = yaw - prev.yaw;
            d.sin().atan2(d.cos()).abs()
        };
        if turned < 2.0 * DEG {
            return false;
        }
        let (w2, h2, cur) = detail_of(pic);
        let (n, n0) = (axis_for_pitch(pitch), axis_for_pitch(prev.pitch));
        // The ring plane's directions in camera space now and in the kept look: e1 across the
        // screen (the same in both), e2 = n × e1 (it tips with the pitch).
        let e2 = [0.0, n[2], -n[1]];
        let e2_0 = [0.0, n0[2], -n0[1]];
        let d = disc.d;
        let glow2 = (disc.r * GLOW).powi(2);
        let (cx0, cy0) = (pic.w as f64 / 2.0, pic.h as f64 / 2.0);
        let windows = windows.filter(|u| u.w == pic.w && u.h == pic.h);
        // The sky behind a pixel turns about the planet's axis by turned × the sine of its angle
        // from the axis: under 6 half-size pixels, its own stars may still overlap themselves.
        let moves = |u: f64, v: f64| -> bool {
            let l = (u * u + v * v + 1.0).sqrt();
            let c = (n[0] * u + n[1] * v - n[2]) / l;
            turned * (1.0 - c * c).max(0.0).sqrt() * lens / 2.0 >= 6.0
        };
        // A half-size pixel → where its ray crosses the ring plane (distance), and where that
        // place on the plane was in the kept look (half-size pixel, rounded).
        let place = |x: usize, y: usize| -> Option<(f64, i64, i64)> {
            let (px, py) = ((2 * x) as f64 + 1.0, (2 * y) as f64 + 1.0);
            let u = (px - cx0) / lens;
            let v = -(py - cy0) / lens;
            let nd = n[0] * u + n[1] * v - n[2];
            if nd.abs() < 1e-9 {
                return None;
            }
            // Ray (u, v, −1) from the camera at (0, 0, d): t along it to the plane.
            let t = -d * n[2] / nd;
            if t <= 0.0 {
                return None;
            }
            let p = [t * u, t * v, d - t];
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            let (a, b) = (p[0], p[1] * e2[1] + p[2] * e2[2]);
            let q = [a, b * e2_0[1], b * e2_0[2]];
            let depth = d - q[2];
            if depth <= 0.05 {
                return None;
            }
            let (qx, qy) = (cx0 + lens * q[0] / depth, cy0 - lens * q[1] / depth);
            Some((r, ((qx - 1.0) / 2.0).round() as i64, ((qy - 1.0) / 2.0).round() as i64))
        };
        let (wi, hi) = (w2 as i64, h2 as i64);
        let inside = |x: i64, y: i64| x >= PATCH + 1 && y >= PATCH + 1 && x < wi - PATCH - 1 && y < hi - PATCH - 1;
        let mut voted = 0u32;
        for y in ((PATCH + 2) as usize..h2 - PATCH as usize - 2).step_by(2) {
            let dy = (2 * y + 1) as f64 - disc.cy;
            for x in ((PATCH + 2) as usize..w2 - PATCH as usize - 2).step_by(2) {
                let dx = (2 * x + 1) as f64 - disc.cx;
                if dx * dx + dy * dy < glow2 {
                    continue;
                }
                if windows.is_some_and(|u| u.blocked(2 * x + 1, 2 * y + 1)) {
                    continue;
                }
                // Enough fine detail in the patch to tell (a few stars, a ring's stripes).
                let (xi, yi) = (x as i64, y as i64);
                let (mut e, mut s1) = (0f64, 0f64);
                for oy in -PATCH..=PATCH {
                    for ox in -PATCH..=PATCH {
                        let v = cur[((yi + oy) * wi + xi + ox) as usize] as f64;
                        e += v * v;
                        s1 += v;
                    }
                }
                let np = ((2 * PATCH + 1) * (2 * PATCH + 1)) as f64;
                let var = e / np - (s1 / np).powi(2);
                if var < 60.0 {
                    continue;
                }
                if !moves((2.0 * x as f64 + 1.0 - cx0) / lens, -(2.0 * y as f64 + 1.0 - cy0) / lens) {
                    continue;
                }
                let Some((r, qx, qy)) = place(x, y) else { continue };
                let Some(bin) = bin_of(r) else { continue };
                // The best match round where that place was (a pixel either way: facings are known
                // to a tenth of a degree or so).
                let mut best = -1.0f64;
                for sy in -1..=1 {
                    for sx in -1..=1 {
                        let (cx, cy) = (qx + sx, qy + sy);
                        if !inside(cx, cy) {
                            continue;
                        }
                        let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
                        for oy in -PATCH..=PATCH {
                            for ox in -PATCH..=PATCH {
                                let a = cur[((yi + oy) * wi + xi + ox) as usize] as f64;
                                let b = prev.detail[((cy + oy) * wi + cx + ox) as usize] as f64;
                                sa += a;
                                sb += b;
                                saa += a * a;
                                sbb += b * b;
                                sab += a * b;
                            }
                        }
                        let (va, vb) = (saa - sa * sa / np, sbb - sb * sb / np);
                        if va > 0.0 && vb > 0.0 {
                            best = best.max((sab - sa * sb / np) / (va * vb).sqrt());
                        }
                    }
                }
                let still = if best >= SAME {
                    true
                } else if best < NOT_SAME {
                    false
                } else {
                    continue;
                };
                let sector = (((dy.atan2(dx) / (2.0 * std::f64::consts::PI) + 1.0) * SECTORS as f64) as usize) % SECTORS;
                let k = bin * SECTORS + sector;
                if still {
                    self.still[k] += 1;
                } else {
                    self.moved[k] += 1;
                }
                if let Some(dbg) = self.debug.as_mut() {
                    dbg[y * w2 + x] = 1 + still as u8;
                }
                voted += 1;
            }
        }
        self.prev = Some(Look { w: pic.w, h: pic.h, detail: cur, yaw, pitch });
        if voted > 0 {
            self.looks += 1;
        }
        voted > 0
    }
    /// Each distance's votes: (from, share still, votes), for looking into it.
    pub fn shares(&self) -> Vec<(f64, f64, u32)> {
        (0..RING_BINS)
            .filter_map(|b| {
                let (st, n) = (0..SECTORS).fold((0u32, 0u32), |(a, n), s| (a + self.still[b * SECTORS + s], n + self.still[b * SECTORS + s] + self.moved[b * SECTORS + s]));
                (n > 0).then(|| (RING_FROM + b as f64 * RING_STEP, st as f64 / n as f64, n))
            })
            .collect()
    }
    /// The cut: distances that voted still well above the sky's few, all round the planet; all
    /// of it from the innermost to the outermost; and 0.1 radii more each side.
    pub fn rings(&self) -> Rings {
        let mut on = vec![false; RING_BINS];
        for b in 0..RING_BINS {
            let (mut n, mut sectors, mut ring_sectors) = (0u32, 0usize, 0usize);
            for s in 0..SECTORS {
                let (st, mv) = (self.still[b * SECTORS + s], self.moved[b * SECTORS + s]);
                n += st + mv;
                if st + mv >= SECTOR_VOTES {
                    sectors += 1;
                    if st as f64 >= RING_SHARE * (st + mv) as f64 {
                        ring_sectors += 1;
                    }
                }
            }
            if n >= MIN_VOTES && sectors >= RING_SECTORS {
                on[b] = ring_sectors >= RING_SECTORS && ring_sectors * 2 >= sectors;
            }
        }
        // A lone distance isn't a ring (two in a row at least, 0.05 radii).
        let lone: Vec<bool> = (0..RING_BINS).map(|b| on[b] && !(b > 0 && on[b - 1]) && !(b + 1 < RING_BINS && on[b + 1])).collect();
        for b in 0..RING_BINS {
            if lone[b] {
                on[b] = false;
            }
        }
        // Everything between the innermost and the outermost ring distance is cut too: faint
        // stripes between bright bands still make features that turn with the planet.
        if let (Some(a), Some(b)) = (on.iter().position(|&x| x), on.iter().rposition(|&x| x)) {
            for k in a..=b {
                on[k] = true;
            }
        }
        // A margin each side (a ring's bright edge half in a bin still makes features there).
        let mut out = Rings::none();
        for b in 0..RING_BINS {
            if (b.saturating_sub(MARGIN)..=(b + MARGIN).min(RING_BINS - 1)).any(|k| on[k]) {
                out.set(b);
            }
        }
        out
    }
    /// The votes to keep with the planet: "RNG2", looks, then still and moved by bin and sector
    /// (u32 LE each).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"RNG2".to_vec();
        out.extend_from_slice(&self.looks.to_le_bytes());
        for v in self.still.iter().chain(&self.moved) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
    pub fn from_bytes(bytes: &[u8]) -> Option<RingLearner> {
        let cells = RING_BINS * SECTORS;
        if bytes.len() != 8 + cells * 8 || &bytes[0..4] != b"RNG2" {
            return None;
        }
        let word = |k: usize| u32::from_le_bytes([bytes[k], bytes[k + 1], bytes[k + 2], bytes[k + 3]]);
        let mut out = RingLearner::new();
        out.looks = word(4);
        for k in 0..cells {
            out.still[k] = word(8 + k * 4);
            out.moved[k] = word(8 + (cells + k) * 4);
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------------------------
// Measured before there's any sky (Tsa's idea): with the camera tilted all the way up (or down),
// looking at the planet's pole, the rings are circles round its middle. Turning the planet then
// only wheels the stars round the middle: whatever stays put, all the way round, is ring. No
// facing is needed, so the sky can start afterwards with the rings already cut out.

/// Rings seen from over a pole: votes by distance from the planet's middle and direction.
pub struct PoleRings {
    still: Vec<u32>,
    moved: Vec<u32>,
    prev: Option<(usize, usize, Vec<i16>)>,
    pub looks: u32,
}

/// Looks with votes, and votes, before a pole's rings count as measured.
const POLE_LOOKS: u32 = 25;
const POLE_VOTES: u64 = 8_000;

impl Default for PoleRings {
    fn default() -> Self {
        PoleRings::new()
    }
}

impl PoleRings {
    pub fn new() -> PoleRings {
        PoleRings { still: vec![0; RING_BINS * SECTORS], moved: vec![0; RING_BINS * SECTORS], prev: None, looks: 0 }
    }
    pub fn votes(&self) -> u64 {
        self.still.iter().chain(&self.moved).map(|&v| v as u64).sum()
    }
    /// 0 … 1: how far the measuring has got.
    pub fn progress(&self) -> f64 {
        (self.votes() as f64 / POLE_VOTES as f64).min(self.looks as f64 / POLE_LOOKS as f64).min(1.0)
    }
    pub fn measured(&self) -> bool {
        self.progress() >= 1.0
    }
    /// The distance in the ring plane of a picture point seen from straight over the pole: the
    /// camera sits on the axis D radii out, so a ring at r shows at lens · r / D from the middle.
    #[inline]
    fn distance(rho: f64, disc: &Disc, lens: f64) -> f64 {
        disc.d * rho / lens
    }
    /// One picture (a few a second). Compared with the last: where the stars moved (the planet is
    /// turning), patches that stayed put vote "still", the rest "moved". Pictures where hardly
    /// anything moved (not turning) don't vote. True when it voted.
    pub fn learn(&mut self, pic: &Pic, disc: &Disc, lens: f64, windows: Option<&UiMask>) -> bool {
        if pic.w < 32 || pic.h < 32 {
            return false;
        }
        let (w2, h2, cur) = detail_of(pic);
        let Some((pw, ph, prev)) = self.prev.replace((w2, h2, cur.clone())) else { return false };
        if (pw, ph) != (w2, h2) {
            return false;
        }
        let windows = windows.filter(|u| u.w == pic.w && u.h == pic.h);
        let glow2 = (disc.r * GLOW).powi(2);
        let (wi, hi) = (w2 as i64, h2 as i64);
        let np = ((2 * PATCH + 1) * (2 * PATCH + 1)) as f64;
        let mut votes: Vec<(usize, bool)> = Vec::new();
        for y in ((PATCH + 2) as usize..h2 - PATCH as usize - 2).step_by(2) {
            let dy = (2 * y + 1) as f64 - disc.cy;
            for x in ((PATCH + 2) as usize..w2 - PATCH as usize - 2).step_by(2) {
                let dx = (2 * x + 1) as f64 - disc.cx;
                let rho2 = dx * dx + dy * dy;
                if rho2 < glow2 {
                    continue;
                }
                if windows.is_some_and(|u| u.blocked(2 * x + 1, 2 * y + 1)) {
                    continue;
                }
                let (xi, yi) = (x as i64, y as i64);
                let (mut e, mut s1) = (0f64, 0f64);
                for oy in -PATCH..=PATCH {
                    for ox in -PATCH..=PATCH {
                        let v = cur[((yi + oy) * wi + xi + ox) as usize] as f64;
                        e += v * v;
                        s1 += v;
                    }
                }
                if e / np - (s1 / np).powi(2) < 60.0 {
                    continue;
                }
                let Some(bin) = bin_of(Self::distance(rho2.sqrt(), disc, lens)) else { continue };
                // The same place in the last picture (a pixel either way for a shaky hand).
                let mut best = -1.0f64;
                for sy in -1..=1i64 {
                    for sx in -1..=1i64 {
                        let (cx, cy) = (xi + sx, yi + sy);
                        if cx < PATCH || cy < PATCH || cx >= wi - PATCH || cy >= hi - PATCH {
                            continue;
                        }
                        let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
                        for oy in -PATCH..=PATCH {
                            for ox in -PATCH..=PATCH {
                                let a = cur[((yi + oy) * wi + xi + ox) as usize] as f64;
                                let b = prev[((cy + oy) * wi + cx + ox) as usize] as f64;
                                sa += a;
                                sb += b;
                                saa += a * a;
                                sbb += b * b;
                                sab += a * b;
                            }
                        }
                        let (va, vb) = (saa - sa * sa / np, sbb - sb * sb / np);
                        if va > 0.0 && vb > 0.0 {
                            best = best.max((sab - sa * sb / np) / (va * vb).sqrt());
                        }
                    }
                }
                let still = if best >= SAME {
                    true
                } else if best < NOT_SAME {
                    false
                } else {
                    continue;
                };
                let sector = (((dy.atan2(dx) / (2.0 * std::f64::consts::PI) + 1.0) * SECTORS as f64) as usize) % SECTORS;
                votes.push((bin * SECTORS + sector, still));
            }
        }
        // Turning? Enough of the sky's patches must have moved (a still picture says nothing;
        // past broad rings only a little sky shows, so it's a count, not a share).
        let moved = votes.iter().filter(|v| !v.1).count();
        if votes.len() < 50 || moved < 40 || (moved as f64) < votes.len() as f64 * 0.08 {
            return false;
        }
        for (k, still) in votes {
            if still {
                self.still[k] += 1;
            } else {
                self.moved[k] += 1;
            }
        }
        self.looks += 1;
        true
    }
    /// The cut, as `RingLearner::rings`: distances still all round the planet (in most of the
    /// directions that show them), filled between the innermost and the outermost, with a margin.
    /// Rings that still go on where the picture ends (broad ones, seen from over a pole) are cut
    /// all the way out: where they stop can't be seen from here.
    pub fn rings(&self) -> Rings {
        let mut on = vec![false; RING_BINS];
        let mut last_seen = 0usize;
        for b in 0..RING_BINS {
            let (mut n, mut sectors, mut ring_sectors) = (0u32, 0usize, 0usize);
            for s in 0..SECTORS {
                let (st, mv) = (self.still[b * SECTORS + s], self.moved[b * SECTORS + s]);
                n += st + mv;
                if st + mv >= SECTOR_VOTES {
                    sectors += 1;
                    if st as f64 >= RING_SHARE * (st + mv) as f64 {
                        ring_sectors += 1;
                    }
                }
            }
            if n >= MIN_VOTES && sectors >= 3 {
                last_seen = b;
                on[b] = ring_sectors >= 3 && ring_sectors as f64 >= 0.6 * sectors as f64;
            }
        }
        let lone: Vec<bool> = (0..RING_BINS).map(|b| on[b] && !(b > 0 && on[b - 1]) && !(b + 1 < RING_BINS && on[b + 1])).collect();
        for b in 0..RING_BINS {
            if lone[b] {
                on[b] = false;
            }
        }
        if let (Some(a), Some(b)) = (on.iter().position(|&x| x), on.iter().rposition(|&x| x)) {
            let to = if b + 3 >= last_seen { RING_BINS - 1 } else { b };
            for k in a..=to {
                on[k] = true;
            }
        }
        let mut out = Rings::none();
        for b in 0..RING_BINS {
            if (b.saturating_sub(MARGIN)..=(b + MARGIN).min(RING_BINS - 1)).any(|k| on[k]) {
                out.set(b);
            }
        }
        out
    }
}

/// The cut as bytes to keep with the planet: "RNG3", then the 7 words (u64 LE).
pub fn cut_to_bytes(r: &Rings) -> Vec<u8> {
    let mut out = b"RNG3".to_vec();
    for w in r.bits {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

pub fn cut_from_bytes(bytes: &[u8]) -> Option<Rings> {
    if bytes.len() != 4 + 7 * 8 || &bytes[0..4] != b"RNG3" {
        return None;
    }
    let mut out = Rings::none();
    for (k, w) in out.bits.iter_mut().enumerate() {
        let at = 4 + k * 8;
        *w = u64::from_le_bytes(bytes[at..at + 8].try_into().ok()?);
    }
    Some(out)
}

/// Which heights of the sky (degrees above the planet's equator, −90 … 90; index + 90) can be
/// recorded at all: seen past the planet and its rings from some facing where enough of the sky
/// shows to stay locked on it (a third of what's round the planet).
pub fn reachable(rings: &Rings, disc: &Disc, w: usize, h: usize, lens: f64) -> [bool; 181] {
    let mut out = [false; 181];
    if !rings.any() {
        return [true; 181];
    }
    let (cx0, cy0) = (w as f64 / 2.0, h as f64 / 2.0);
    let glow2 = (disc.r * GLOW).powi(2);
    let step = 12usize;
    let mut rays: Vec<[f32; 3]> = Vec::new();
    for y in (step / 2..h).step_by(step) {
        for x in (step / 2..w).step_by(step) {
            if (x as f64 - disc.cx).powi(2) + (y as f64 - disc.cy).powi(2) < glow2 {
                continue;
            }
            let u = (x as f64 + 0.5 - cx0) / lens;
            let v = -(y as f64 + 0.5 - cy0) / lens;
            let l = (u * u + v * v + 1.0).sqrt();
            rays.push([(u / l) as f32, (v / l) as f32, (-1.0 / l) as f32]);
        }
    }
    if rays.is_empty() {
        return out;
    }
    let mut heights = Vec::with_capacity(rays.len());
    for p in -89..=89 {
        let n = axis_for_pitch(p as f64 * DEG);
        heights.clear();
        for r in &rays {
            if !rings.hit(r, &n, disc.d) {
                heights.push((n[0] * r[0] as f64 + n[1] * r[1] as f64 + n[2] * r[2] as f64).clamp(-1.0, 1.0).asin() / DEG);
            }
        }
        if (heights.len() as f64) < rays.len() as f64 / 3.0 {
            continue;
        }
        for &e in &heights {
            out[(e.round() + 90.0).clamp(0.0, 180.0) as usize] = true;
        }
    }
    out
}

/// Share of the sky (area) in the heights that can be recorded.
pub fn reachable_share(rows: &[bool; 181]) -> f64 {
    let (mut seen, mut all) = (0.0, 0.0);
    for (i, &ok) in rows.iter().enumerate() {
        let w = ((i as f64 - 90.0) * DEG).cos().max(0.0) + 1e-9;
        all += w;
        if ok {
            seen += w;
        }
    }
    seen / all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planetscan::geom::Disc;

    /// A made-up turn: rings (from 1.6 to 1.9 radii, textured by distance) round the planet at
    /// 40° pitch, stars fixed on the sky (so they move with the yaw), and a window. The rings are
    /// found to within a bin or two; the window and the sky aren't rings.
    fn picture(w: usize, h: usize, disc: &Disc, lens: f64, yaw: f64, pitch: f64) -> Vec<u8> {
        picture_with(w, h, disc, lens, yaw, pitch, &|r| (1.6..=1.9).contains(&r).then(|| (110.0 + 60.0 * (r * 60.0).sin(), 0.8)))
    }

    /// The same with any rings: distance → (brightness, opacity).
    fn picture_with(w: usize, h: usize, disc: &Disc, lens: f64, yaw: f64, pitch: f64, ring: &dyn Fn(f64) -> Option<(f64, f64)>) -> Vec<u8> {
        use crate::planetscan::geom::{apply, pose_q, transpose};
        let n = axis_for_pitch(pitch);
        let qt = transpose(&pose_q(yaw, pitch));
        let mut px = vec![0u8; w * h * 4];
        let (cx0, cy0) = (w as f64 / 2.0, h as f64 / 2.0);
        for y in 0..h {
            for x in 0..w {
                let u = (x as f64 + 0.5 - cx0) / lens;
                let v = -(y as f64 + 0.5 - cy0) / lens;
                let l = (u * u + v * v + 1.0).sqrt();
                let ray = [(u / l) as f32, (v / l) as f32, (-1.0 / l) as f32];
                let s = apply(&qt, &[u / l, v / l, -1.0 / l]);
                let (a, e) = ((s[1].atan2(s[0]) / (0.4 * DEG)).floor() as i64, (s[2].clamp(-1.0, 1.0).acos() / (0.4 * DEG)).floor() as i64);
                let hash = (a.wrapping_mul(73_856_093) ^ e.wrapping_mul(19_349_663)).rem_euclid(97);
                let mut val = 10.0 + if hash < 9 { 160.0 } else { 0.0 };
                if let Some(r) = plane_radius(&ray, &n, disc.d) {
                    if let Some((lum, a)) = ring(r) {
                        val = (1.0 - a) * val + a * lum;
                    }
                }
                if (x as f64 - disc.cx).hypot(y as f64 - disc.cy) < disc.r {
                    val = 120.0;
                }
                if x < 80 && y < 120 {
                    val = if (x / 3 + y / 5) % 2 == 0 { 220.0 } else { 40.0 };
                }
                let b = val.clamp(0.0, 255.0) as u8;
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[b, b, b, 255]);
            }
        }
        px
    }

    #[test]
    fn rings_measured_from_a_sideways_turn() {
        let (w, h) = (640usize, 360usize);
        let lens = 375.0;
        let disc = Disc::with_lens(320.0, 180.0, 0.26 * lens, lens);
        let mut learner = RingLearner::new();
        // A sideways turn that drifts in tilt too (as a hand on a mouse does).
        for k in 0..14 {
            let (yaw, pitch) = (k as f64 * 1.5 * DEG, (40.0 + 0.4 * k as f64) * DEG);
            let px = picture(w, h, &disc, lens, yaw, pitch);
            learner.learn(&Pic { w, h, bgra: &px }, &disc, lens, yaw, pitch, None);
        }
        let rings = learner.rings();
        let bands = rings.bands();
        assert!(!bands.is_empty(), "no rings found");
        let (from, to) = (bands.first().unwrap().0, bands.last().unwrap().1);
        // The rings, with their 0.1-radius margin each side.
        assert!(from > 1.4 && from < 1.6 && to > 1.9 && to < 2.1, "bands {bands:?}");
        // Sky well past the rings isn't cut.
        assert!(!rings.cut_at(3.0) && !rings.cut_at(1.3), "bands {bands:?}");
        let back = RingLearner::from_bytes(&learner.to_bytes()).unwrap();
        assert_eq!(back.rings(), rings);
    }

    /// From over a pole (top or bottom, or not quite: EVE may stop short of straight down), a turn
    /// of the planet finds thin rings where they are, broad ones cut all the way out, and none
    /// where there are none; a window isn't a ring and a still picture says nothing.
    #[test]
    fn rings_measured_from_over_the_poles() {
        let (w, h) = (640usize, 360usize);
        let lens = 375.0;
        let disc = Disc::with_lens(320.0, 180.0, 0.26 * lens, lens);
        let thin = |r: f64| (1.6..=1.9).contains(&r).then(|| (110.0 + 60.0 * (r * 60.0).sin(), 0.8));
        let broad = |r: f64| (2.0..=6.0).contains(&r).then(|| (90.0 + 50.0 * (r * 23.0).sin() + 25.0 * (r * 71.0).sin(), 0.85));
        let none = |_: f64| None;
        let measure = |pitch: f64, ring: &dyn Fn(f64) -> Option<(f64, f64)>, turn: f64| {
            let mut pole = PoleRings::new();
            for k in 0..POLE_LOOKS as usize + 3 {
                let px = picture_with(w, h, &disc, lens, k as f64 * turn * DEG, pitch * DEG, ring);
                pole.learn(&Pic { w, h, bgra: &px }, &disc, lens, None);
            }
            pole
        };
        for pitch in [88.0, -88.0, 75.0] {
            let p = measure(pitch, &thin, 3.0);
            assert!(p.measured(), "thin at {pitch}: progress {}", p.progress());
            let rg = p.rings();
            eprintln!("thin from {pitch}°: {} looks, {} votes, bands {:?}", p.looks, p.votes(), rg.bands());
            assert!(rg.cut_at(1.75) && rg.cut_at(1.62) && rg.cut_at(1.88), "thin at {pitch}: {:?}", rg.bands());
            assert!(!rg.cut_at(1.35) && !rg.cut_at(2.3) && !rg.cut_at(4.0), "thin at {pitch}: {:?}", rg.bands());
        }
        let p = measure(85.0, &broad, 3.0);
        let rg = p.rings();
        eprintln!("broad: {} looks, {} votes, bands {:?}", p.looks, p.votes(), rg.bands());
        assert!(rg.cut_at(2.2) && rg.cut_at(3.0) && rg.cut_at(9.0), "broad: {:?}", rg.bands());
        assert!(!rg.cut_at(1.6), "broad: {:?}", rg.bands());
        let p = measure(88.0, &none, 3.0);
        assert!(p.measured() && !p.rings().any(), "none: {:?}", p.rings().bands());
        let p = measure(88.0, &thin, 0.0);
        assert_eq!(p.looks, 0);
    }

    #[test]
    fn cut_bytes_round_trip() {
        let rg = Rings::band(1.5, 2.1);
        assert_eq!(cut_from_bytes(&cut_to_bytes(&rg)), Some(rg));
        assert_eq!(cut_from_bytes(b"RNG3"), None);
    }

    #[test]
    fn broad_rings_hide_the_sky_past_the_poles() {
        let (w, h) = (1918usize, 1024usize);
        let lens = 750.0 * 1918.0 / 1280.0;
        let disc = Disc::with_lens(959.0, 512.0, 292.0, lens);
        assert_eq!(reachable(&Rings::none(), &disc, w, h, lens), [true; 181]);
        let thin = reachable_share(&reachable(&Rings::band(1.08, 1.9), &disc, w, h, lens));
        let broad = reachable_share(&reachable(&Rings::band(1.08, 4.6), &disc, w, h, lens));
        assert!(thin > 0.95, "thin {thin}");
        assert!(broad > 0.4 && broad < 0.75, "broad {broad}");
        // A ray straight at the planet's middle never hits them; looking past it at a tilt does.
        let rg = Rings::band(1.08, 4.6);
        assert!(!rg.hit(&[0.0, 0.0, -1.0], &axis_for_pitch(20.0 * DEG), 4.0));
        assert!(!rg.hit(&[0.3, -0.2, -0.93], &axis_for_pitch(0.0), 4.0));
        let l = (0.28f32 * 0.28 + 1.0).sqrt();
        assert!(rg.hit(&[0.0, -0.28 / l, -1.0 / l], &axis_for_pitch(20.0 * DEG), 4.0));
    }
}
