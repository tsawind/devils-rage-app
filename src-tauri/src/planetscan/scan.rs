//! Reading EVE's scan colours onto a whole-planet map (one cell per degree, the size SeAT keeps),
//! the bare planet's own colours (for unmixing), and EVE's resource list (which row is picked).

use super::geom::{apply, facing_of, lat_lon, planet_vector, to_sphere, transpose, Disc, M3};
use super::sky::Pic;
use std::f64::consts::PI;

pub const GRID_W: usize = 360;
pub const GRID_H: usize = 180;
/// Resource scans: 0.5° cells (4× the 1° grid SeAT keeps; SeAT gets them averaged to 1°).
pub const RES_W: usize = 720;
pub const RES_H: usize = 360;

/// Paint from within about 37° of the disc's middle (facing ≥ 0.8); a cell keeps its most
/// face-on look, and is settled once seen at 0.97.
pub const PAINT_FACING: f64 = 0.8;
/// A resource scan from within about 45° (facing ≥ 0.7): EVE's resource fields are smooth on a
/// 1° grid, so a bit more of each picture counts and a planet fills in faster.
pub const RESOURCE_FACING: f64 = 0.7;
const SETTLED: f32 = 0.97;
/// A planet map colour this bright (glowing lava) can't be seen through: EVE's resource colours
/// laid over it read as nothing.
pub const GLOW: f32 = 215.0;
/// A cell with only unreadable looks this face-on, at least this many over at least this many
/// pictures (about 1.5 s: not one blink of EVE's colours fading in), can't be read either.
const BLIND_FACING: f32 = 0.85;
const BLIND_LOOKS: u16 = 6;
const BLIND_SPAN: u32 = 45;

/// EVE's scan colours to heat 0..1 when the bare planet isn't known (blue .2 → red .8 → white 1).
pub fn heat(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 510.0;
    let s = if max == min { 0.0 } else { (max - min) / (255.0 - (max + min - 255.0).abs()) };
    if l > 0.82 && s < 0.35 {
        return 1.0;
    }
    if s < 0.3 || max < 60.0 {
        return 0.0;
    }
    let d = max - min;
    let mut h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h * 60.0 + 360.0) % 360.0;
    if h > 300.0 {
        h = 0.0;
    }
    if h > 240.0 {
        h = 240.0;
    }
    0.2 + 0.6 * (1.0 - h / 240.0) * (max / 200.0).min(1.0)
}

// EVE's scan colour band, low → high, laid over the planet:
// what you see = (1 − OVERLAY) × planet + OVERLAY × BOOST × band colour.
const BAND: [[f32; 3]; 14] = [
    [20., 14., 25.], [21., 21., 40.], [19., 36., 55.], [18., 57., 49.], [18., 75., 33.], [37., 99., 7.], [74., 112., 7.],
    [118., 126., 6.], [145., 138., 5.], [161., 109., 12.], [169., 58., 4.], [179., 5., 3.], [187., 43., 42.], [195., 191., 190.],
];
const OVERLAY: f32 = 0.35;
const BOOST: f32 = 1.6;
const STEPS: usize = 60;

fn band_fine() -> &'static [[f32; 3]; STEPS] {
    use std::sync::OnceLock;
    static FINE: OnceLock<[[f32; 3]; STEPS]> = OnceLock::new();
    FINE.get_or_init(|| {
        let mut out = [[0.0; 3]; STEPS];
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 / (STEPS - 1) as f32 * (BAND.len() - 1) as f32;
            let a = t.floor() as usize;
            let b = (a + 1).min(BAND.len() - 1);
            let f = t - a as f32;
            for c in 0..3 {
                o[c] = OVERLAY * BOOST * (BAND[a][c] * (1.0 - f) + BAND[b][c] * f);
            }
        }
        out
    })
}

/// EVE's band colour for a level (for showing a scan), brightened to full strength.
pub fn band_rgb(level: f32) -> [u8; 3] {
    let t = level.clamp(0.0, 1.0) * (BAND.len() - 1) as f32;
    let a = t.floor() as usize;
    let b = (a + 1).min(BAND.len() - 1);
    let f = t - a as f32;
    let mut out = [0u8; 3];
    for c in 0..3 {
        out[c] = ((BAND[a][c] * (1.0 - f) + BAND[b][c] * f) * 1.3).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Resource level 0 … 1 at a pixel, knowing the bare planet's colour there; None where the
/// planet is too bright to see through (glowing lava). Every 4th step of the band first, then
/// the steps round the best (a third of the work of trying them all, the same answer).
pub fn unmix(r: u8, g: u8, b: u8, br: f32, bg: f32, bb: f32) -> Option<f32> {
    if br.max(bg).max(bb) > GLOW {
        return None;
    }
    let k = 1.0 - OVERLAY;
    let d = [k * br - r as f32, k * bg - g as f32, k * bb - b as f32];
    let fine = band_fine();
    let err = |i: usize| {
        let c = &fine[i];
        (d[0] + c[0]).powi(2) + (d[1] + c[1]).powi(2) + (d[2] + c[2]).powi(2)
    };
    let (mut best, mut at) = (f32::INFINITY, 0);
    let mut i = 0;
    while i < STEPS {
        let e = err(i);
        if e < best {
            best = e;
            at = i;
        }
        i += 4;
    }
    let e = err(STEPS - 1);
    if e < best {
        best = e;
        at = STEPS - 1;
    }
    for i in at.saturating_sub(3)..=(at + 3).min(STEPS - 1) {
        let e = err(i);
        if e < best {
            best = e;
            at = i;
        }
    }
    if best > 4000.0 {
        None
    } else {
        Some(at as f32 / (STEPS - 1) as f32)
    }
}

/// The same by trying every step (for checking the quick one).
#[cfg(test)]
fn unmix_every_step(r: u8, g: u8, b: u8, br: f32, bg: f32, bb: f32) -> Option<f32> {
    if br.max(bg).max(bb) > 215.0 {
        return None;
    }
    let k = 1.0 - OVERLAY;
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let (mut best, mut at) = (f32::INFINITY, 0);
    for (i, c) in band_fine().iter().enumerate() {
        let e = (k * br + c[0] - r).powi(2) + (k * bg + c[1] - g).powi(2) + (k * bb + c[2] - b).powi(2);
        if e < best {
            best = e;
            at = i;
        }
    }
    if best > 4000.0 {
        None
    } else {
        Some(at as f32 / (STEPS - 1) as f32)
    }
}

/// A whole-planet map: a resource's levels, or (bare) the planet's own colours.
pub struct Map {
    pub bare: bool,
    pub w: usize,
    pub h: usize,
    /// Resource: level; bare: r, g, b.
    pub value: Vec<f32>,
    /// Resource: the colours seen (r, g, b), kept so the levels can be read again later against
    /// a better planet map.
    pub raw: Vec<f32>,
    pub seen: Vec<bool>,
    pub best: Vec<f32>,
    pub version: u64,
    /// Each picture's sums per cell (r, g, b, count, level) and how face-on, kept between pictures.
    scratch: Vec<[f32; 5]>,
    square: Vec<f32>,
    /// How face-on this picture's unreadable looks at each cell were.
    failed: Vec<f32>,
    /// Cells EVE's colours can't be read through (glowing lava): unreadable looks so far (MAX =
    /// glows on the planet map), and the pictures of the first and the last of them. They don't
    /// count as still to see.
    blind: Vec<u16>,
    blind_span: Vec<(u32, u32)>,
    pictures: u32,
    /// The coverage worked out last, and the version it was for (+1; 0 = none yet): it's asked
    /// for many times a second and only changes when cells are painted.
    cov_cache: (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64),
}

impl Map {
    pub fn new(bare: bool) -> Map {
        Map::with_size(bare, GRID_W, GRID_H)
    }
    /// A map on a w × h grid (rows from the north pole, longitude round).
    pub fn with_size(bare: bool, w: usize, h: usize) -> Map {
        let n = w * h;
        Map {
            bare,
            w,
            h,
            value: vec![0.0; if bare { n * 3 } else { n }],
            raw: if bare { Vec::new() } else { vec![0.0; n * 3] },
            seen: vec![false; n],
            best: vec![0.0; n],
            version: 0,
            scratch: vec![[0.0; 5]; n],
            square: vec![0.0; n],
            failed: vec![0.0; n],
            blind: vec![0; n],
            blind_span: vec![(0, 0); n],
            pictures: 0,
            cov_cache: Default::default(),
        }
    }
    /// Seen, or can't be seen (glowing lava): nothing more to do there.
    #[inline]
    pub fn known(&self, i: usize) -> bool {
        self.seen[i] || self.blind_cell(i)
    }
    #[inline]
    fn blind_cell(&self, i: usize) -> bool {
        let b = self.blind[i];
        b == u16::MAX || (b >= BLIND_LOOKS && self.blind_span[i].1 - self.blind_span[i].0 >= BLIND_SPAN)
    }
    /// Cells the planet map shows glowing (lava too bright to see EVE's colours through, the same
    /// rule the reading uses): they can never be read, so they aren't left to see. Returns their
    /// share of the planet.
    pub fn mark_glow(&mut self, bare: &Map) -> f64 {
        if self.bare || bare.w != self.w || bare.h != self.h {
            return 0.0;
        }
        let (mut hot, mut all) = (0.0, 0.0);
        for row in 0..self.h {
            let w = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            for col in 0..self.w {
                let i = row * self.w + col;
                all += w;
                if bare.bare_rgb(i).is_some_and(|(r, g, b)| r.max(g).max(b) > GLOW) {
                    self.blind[i] = u16::MAX;
                    hot += w;
                }
            }
        }
        self.version += 1;
        hot / all
    }
    /// The share of the planet that can't be read (glowing lava).
    pub fn blind_share(&self) -> f64 {
        let (mut hot, mut all) = (0.0, 0.0);
        for row in 0..self.h {
            let w = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            for col in 0..self.w {
                let i = row * self.w + col;
                all += w;
                if !self.seen[i] && self.blind_cell(i) {
                    hot += w;
                }
            }
        }
        hot / all
    }
    pub fn coverage(&self) -> f64 {
        use std::sync::atomic::Ordering::Relaxed;
        if self.cov_cache.0.load(Relaxed) == self.version + 1 {
            return f64::from_bits(self.cov_cache.1.load(Relaxed));
        }
        let cov = self.coverage_now();
        self.cov_cache.1.store(cov.to_bits(), Relaxed);
        self.cov_cache.0.store(self.version + 1, Relaxed);
        cov
    }
    fn coverage_now(&self) -> f64 {
        let (mut seen, mut all) = (0.0, 0.0);
        for row in 0..self.h {
            let w = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            for col in 0..self.w {
                all += w;
                if self.known(row * self.w + col) || self.gap(row, col) {
                    seen += w;
                }
            }
        }
        seen / all
    }
    /// An unseen cell with at least 6 of its 8 neighbours seen: a gap between pixels, not a part
    /// of the planet still to see.
    #[inline]
    fn gap(&self, row: usize, col: usize) -> bool {
        if row == 0 || row + 1 >= self.h {
            return false;
        }
        let mut n = 0;
        for dr in [-1i64, 0, 1] {
            for dc in [-1i64, 0, 1] {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let r = (row as i64 + dr) as usize;
                let c = (col as i64 + dc).rem_euclid(self.w as i64) as usize;
                if self.known(r * self.w + c) {
                    n += 1;
                }
            }
        }
        n >= 6
    }
    #[inline]
    pub fn bare_rgb(&self, cell: usize) -> Option<(f32, f32, f32)> {
        if !self.bare || !self.seen[cell] {
            return None;
        }
        Some((self.value[cell * 3], self.value[cell * 3 + 1], self.value[cell * 3 + 2]))
    }
    /// Paint one picture: every disc pixel facing the camera well enough (`every` = pixel step).
    /// With `bare`, a resource map unmixed against the planet's own colours. Returns cells painted.
    /// The pixels are worked out on several cores (the unmixing is the slow part).
    pub fn paint(&mut self, pic: &Pic, disc: &Disc, q: &M3, mirrored: bool, every: usize, bare: Option<&Map>) -> usize {
        let qt = transpose(q);
        let every = every.max(1);
        let (y0, y1) = ((disc.cy - disc.r).floor().max(0.0) as usize, ((disc.cy + disc.r).ceil() as usize).min(pic.h - 1));
        let (x0, x1) = ((disc.cx - disc.r).floor().max(0.0) as usize, ((disc.cx + disc.r).ceil() as usize).min(pic.w - 1));
        let rows: Vec<usize> = (y0..=y1).step_by(every).collect();
        let threads = super::sky::paint_cores();
        let chunk = rows.len().div_ceil(threads).max(1);
        let is_bare = self.bare;
        let (gw, gh) = (self.w, self.h);
        let min_facing = if is_bare { PAINT_FACING } else { RESOURCE_FACING };
        // Each pixel: its cell, colour, level and how face-on.
        let looks: Vec<Vec<(u32, [f32; 4], f32)>> = std::thread::scope(|sc| {
            let handles: Vec<_> = rows
                .chunks(chunk)
                .map(|band| {
                    sc.spawn(move || {
                        super::sky::background_thread();
                        let mut out = Vec::with_capacity(band.len() * (x1 - x0) / every + 1);
                        for &y in band {
                            let mut x = x0;
                            while x <= x1 {
                                if let Some(v) = to_sphere(x as f64, y as f64, disc) {
                                    let f = facing_of(&v, disc);
                                    if f >= min_facing {
                                        let (lat, lon) = lat_lon(&apply(&qt, &v), mirrored);
                                        let row = ((lat / PI * gh as f64).floor() as usize).min(gh - 1);
                                        let col = ((lon / (2.0 * PI) * gw as f64).floor() as usize).min(gw - 1);
                                        let cell = row * gw + col;
                                        let (r, g, b) = pic.rgb(x, y);
                                        let level = if is_bare {
                                            Some(0.0)
                                        } else if let Some((br, bg, bb)) = bare.and_then(|m| m.bare_rgb(cell)) {
                                            unmix(r, g, b, br, bg, bb)
                                        } else {
                                            Some(heat(r, g, b))
                                        };
                                        // Unreadable (glowing lava under EVE's colours): kept as such.
                                        out.push((cell as u32, [r as f32, g as f32, b as f32, level.unwrap_or(f32::NAN)], f as f32));
                                    }
                                }
                                x += every;
                            }
                        }
                        out
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        // This picture's own look at each cell: sums and how face-on.
        let mut touched: Vec<usize> = Vec::new();
        self.pictures += 1;
        for part in &looks {
            for &(cell, c, f) in part {
                let cell = cell as usize;
                let s = &mut self.scratch[cell];
                if s[3] == 0.0 && self.failed[cell] == 0.0 {
                    touched.push(cell);
                }
                if c[3].is_nan() {
                    self.failed[cell] = self.failed[cell].max(f).max(1e-6);
                    continue;
                }
                s[0] += c[0];
                s[1] += c[1];
                s[2] += c[2];
                s[3] += 1.0;
                s[4] += c[3];
                self.square[cell] = self.square[cell].max(f);
            }
        }
        let mut painted = 0;
        let mut blinded = false;
        for cell in touched {
            let s = std::mem::replace(&mut self.scratch[cell], [0.0; 5]);
            let square = std::mem::replace(&mut self.square[cell], 0.0);
            let failed = std::mem::replace(&mut self.failed[cell], 0.0);
            if s[3] == 0.0 {
                // Only unreadable looks this picture.
                if failed >= BLIND_FACING && self.blind[cell] != u16::MAX {
                    if self.blind[cell] == 0 {
                        self.blind_span[cell].0 = self.pictures;
                    }
                    let was = self.blind_cell(cell);
                    self.blind[cell] = (self.blind[cell] + 1).min(u16::MAX - 1);
                    self.blind_span[cell].1 = self.pictures;
                    blinded |= !was && self.blind_cell(cell);
                }
                continue;
            }
            if self.best[cell] >= SETTLED || square <= self.best[cell] + 0.02 {
                continue;
            }
            self.best[cell] = square;
            if self.bare {
                self.value[cell * 3] = s[0] / s[3];
                self.value[cell * 3 + 1] = s[1] / s[3];
                self.value[cell * 3 + 2] = s[2] / s[3];
            } else {
                self.value[cell] = s[4] / s[3];
                self.raw[cell * 3] = s[0] / s[3];
                self.raw[cell * 3 + 1] = s[1] / s[3];
                self.raw[cell * 3 + 2] = s[2] / s[3];
            }
            self.seen[cell] = true;
            painted += 1;
        }
        if painted > 0 || blinded {
            self.version += 1;
        }
        painted
    }
    /// A resource scan to keep on the PC: "RAW2", width and height (u16 LE), then for each cell its
    /// level byte (as `cells`) and the colours seen (r, g, b; 0 where not seen), so it can be read
    /// again later against a better planet map.
    pub fn raw_bytes(&self) -> Vec<u8> {
        let n = self.w * self.h;
        let mut out = b"RAW2".to_vec();
        out.extend_from_slice(&(self.w as u16).to_le_bytes());
        out.extend_from_slice(&(self.h as u16).to_le_bytes());
        out.extend(self.cells());
        for i in 0..n {
            for c in 0..3 {
                out.push(if self.seen[i] && !self.bare { self.raw[i * 3 + c].round().clamp(0.0, 255.0) as u8 } else { 0 });
            }
        }
        out
    }
    /// The cells still to see (for the green targets): not seen, and not a gap between pixels.
    pub fn missing(&self) -> Vec<bool> {
        (0..self.w * self.h).map(|i| !self.known(i) && !self.gap(i / self.w, i % self.w)).collect()
    }
    /// The levels on SeAT's 1° grid: each 1° cell the mean of the seen cells in it (0 where none).
    pub fn cells_1deg(&self) -> Vec<u8> {
        if (self.w, self.h) == (GRID_W, GRID_H) {
            return self.cells();
        }
        let mut sum = vec![(0f32, 0u32); GRID_W * GRID_H];
        for row in 0..self.h {
            let gr = (row * GRID_H / self.h).min(GRID_H - 1);
            for col in 0..self.w {
                let i = row * self.w + col;
                if self.seen[i] {
                    let gc = (col * GRID_W / self.w).min(GRID_W - 1);
                    let s = &mut sum[gr * GRID_W + gc];
                    s.0 += self.value[i];
                    s.1 += 1;
                }
            }
        }
        sum.iter().map(|&(v, n)| if n > 0 { 1 + ((v / n as f32).clamp(0.0, 1.0) * 250.0).round() as u8 } else { 0 }).collect()
    }
    /// EVE's resource fields are smooth: a lone cell far from the cells round it takes their median.
    pub fn despeckle(&mut self, limit: f32) {
        if self.bare {
            return;
        }
        for _ in 0..2 {
            let mut fix = Vec::new();
            for row in 0..self.h {
                for col in 0..self.w {
                    let i = row * self.w + col;
                    if !self.seen[i] {
                        continue;
                    }
                    let mut near = Vec::with_capacity(25);
                    for dr in -2i64..=2 {
                        let r = row as i64 + dr;
                        if r < 0 || r >= self.h as i64 {
                            continue;
                        }
                        for dc in -2i64..=2 {
                            let j = r as usize * self.w + (col as i64 + dc).rem_euclid(self.w as i64) as usize;
                            if self.seen[j] {
                                near.push(self.value[j]);
                            }
                        }
                    }
                    if near.len() < 9 {
                        continue;
                    }
                    near.sort_by(f32::total_cmp);
                    let med = near[near.len() / 2];
                    if (self.value[i] - med).abs() > limit {
                        fix.push((i, med));
                    }
                }
            }
            if fix.is_empty() {
                break;
            }
            for (i, v) in fix {
                self.value[i] = v;
            }
        }
    }
    /// SeAT's bytes: 0 = not seen, 1 … 251 = level 0 … 1 (resource maps).
    pub fn cells(&self) -> Vec<u8> {
        (0..self.w * self.h).map(|i| if self.seen[i] { 1 + (self.value[i].clamp(0.0, 1.0) * 250.0).round() as u8 } else { 0 }).collect()
    }
    /// Bare map bytes: r, g, b a cell (0, 0, 0 = not seen).
    pub fn bare_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h * 3];
        for i in 0..self.w * self.h {
            if self.seen[i] {
                for c in 0..3 {
                    out[i * 3 + c] = self.value[i * 3 + c].round().clamp(if c == 0 { 1.0 } else { 0.0 }, 255.0) as u8;
                }
            }
        }
        out
    }
    pub fn from_bare_bytes(bytes: &[u8]) -> Option<Map> {
        let (w, h) = if bytes.len() == GRID_W * GRID_H * 3 {
            (GRID_W, GRID_H)
        } else if bytes.len() == RES_W * RES_H * 3 {
            (RES_W, RES_H)
        } else {
            return None;
        };
        let mut m = Map::with_size(true, w, h);
        for i in 0..w * h {
            if bytes[i * 3] == 0 && bytes[i * 3 + 1] == 0 && bytes[i * 3 + 2] == 0 {
                continue;
            }
            m.seen[i] = true;
            m.best[i] = 1.0; // a kept bare map is settled
            for c in 0..3 {
                m.value[i * 3 + c] = bytes[i * 3 + c] as f32;
            }
        }
        Some(m)
    }
    /// Where the biggest unseen part is, in words relative to the picture now.
    pub fn missing_hint(&self, q: &M3, mirrored: bool) -> Option<String> {
        let (mut sx, mut sy, mut sz, mut n) = (0.0, 0.0, 0.0, 0.0);
        let k = (self.h / 60).max(1);
        for row in (0..self.h).step_by(k) {
            let lat = (row as f64 + 0.5) / self.h as f64 * PI;
            let w = lat.sin();
            for col in (0..self.w).step_by(k) {
                if self.known(row * self.w + col) {
                    continue;
                }
                let v = apply(q, &planet_vector(lat, (col as f64 + 0.5) / self.w as f64 * 2.0 * PI, mirrored));
                sx += v[0] * w;
                sy += v[1] * w;
                sz += v[2] * w;
                n += w;
            }
        }
        if n == 0.0 {
            return None;
        }
        let l = (sx * sx + sy * sy + sz * sz).sqrt().max(1e-9);
        let v = [sx / l, sy / l, sz / l];
        if v[2] > 0.6 {
            return Some("in front of you: hold still a moment over it".into());
        }
        let mut parts = Vec::new();
        if v[1] > 0.3 {
            parts.push("top");
        } else if v[1] < -0.3 {
            parts.push("bottom");
        }
        if v[0] > 0.3 {
            parts.push("right");
        } else if v[0] < -0.3 {
            parts.push("left");
        }
        if parts.is_empty() {
            return Some("on the far side: turn the planet right round".into());
        }
        Some(format!("past the {} edge: turn that side round to the front", parts.join("-")))
    }
}

/// How well the planet in the picture matches a bare map at facing Q: correlation of their
/// brightness patterns (a resource filter tints the planet but keeps its pattern). Used to tell
/// which planet is open.
pub fn looks_like(pic: &Pic, disc: &Disc, q: &M3, mirrored: bool, bare: &Map) -> Option<f64> {
    let qt = transpose(q);
    let (mut c, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let step = (disc.r / 40.0).max(2.0);
    let mut y = disc.cy - disc.r;
    while y < disc.cy + disc.r {
        let mut x = disc.cx - disc.r;
        while x < disc.cx + disc.r {
            if x >= 0.0 && y >= 0.0 && (x as usize) < pic.w && (y as usize) < pic.h {
                if let Some(v) = to_sphere(x, y, disc) {
                    if facing_of(&v, disc) >= 0.6 {
                        let (lat, lon) = lat_lon(&apply(&qt, &v), mirrored);
                        let cell = ((lat / PI * bare.h as f64) as usize).min(bare.h - 1) * bare.w + ((lon / (2.0 * PI) * bare.w as f64) as usize).min(bare.w - 1);
                        if let Some((r, g, b)) = bare.bare_rgb(cell) {
                            let a = pic.lum(x as usize, y as usize) as f64;
                            let m = (0.3 * r + 0.59 * g + 0.11 * b) as f64;
                            c += 1.0;
                            sa += a;
                            sb += m;
                            saa += a * a;
                            sbb += m * m;
                            sab += a * m;
                        }
                    }
                }
            }
            x += step;
        }
        y += step;
    }
    if c < 200.0 {
        return None;
    }
    let (va, vb) = (saa - sa * sa / c, sbb - sb * sb / c);
    if va <= 0.0 || vb <= 0.0 {
        return None;
    }
    Some((sab - sa * sb / c) / (va * vb).sqrt())
}

/// EVE's resource list in the planet's scan panel: where its rows are (picture y of "No filter"
/// and the row spacing, from the pilot pointing them out once) and which one is lit up.
#[derive(Clone, Copy, Debug)]
pub struct ListSpot {
    /// Picture x over the rows' names, y of "No filter", y of the last resource row.
    pub x: f64,
    pub y0: f64,
    pub y5: f64,
}

/// Which row is lit up: 0 = No filter, 1 … 5 = the resources top to bottom. None when no row
/// stands out (the panel is closed or moved, or two rows are lit: one picked, one under the mouse).
///
/// EVE lights the picked row with a box the width of the list and 0.88 rows tall. Its top and
/// bottom edges are sharp steps, where a bright nebula behind the see-through list only changes
/// slowly: so each row is judged by those two steps, not by how bright it is (a bright patch of
/// sky behind one row once read as that row being picked).
pub fn picked_row(pic: &Pic, spot: &ListSpot) -> Option<usize> {
    let gap = (spot.y5 - spot.y0) / 5.0;
    if gap < 6.0 {
        return None;
    }
    let half = 0.44 * gap;
    let (xa, xb) = ((spot.x - gap * 2.2).max(0.0) as usize, ((spot.x + gap * 6.6).max(0.0) as usize).min(pic.w));
    if xb <= xa + 8 {
        return None;
    }
    // Each picture row's middle brightness across the list (text and bars are a small part of it).
    let top = (spot.y0 - half - 10.0).floor().max(0.0) as usize;
    let bottom = ((spot.y5 + half + 10.0).ceil() as usize).min(pic.h);
    if bottom <= top + 4 {
        return None;
    }
    let mut vals = Vec::with_capacity((xb - xa) / 2 + 1);
    let mut line = vec![None; bottom - top];
    for (i, y) in (top..bottom).enumerate() {
        vals.clear();
        vals.extend((xa..xb).step_by(2).map(|x| pic.lum(x, y)));
        vals.sort_unstable();
        line[i] = Some(vals[vals.len() / 2] as f64);
    }
    let at = |y: f64| -> Option<f64> {
        let y = y.round();
        if y < top as f64 {
            return None;
        }
        line.get(y as usize - top).copied().flatten()
    };
    let mut score = [f64::NEG_INFINITY; 6];
    for (row, s) in score.iter_mut().enumerate() {
        let yc = spot.y0 + gap * row as f64;
        for o in -6..=6 {
            let (yt, yb) = (yc + o as f64 - half, yc + o as f64 + half);
            // Each step across 3 pixels, so a box a pixel taller or shorter still shows both.
            if let (Some(t_in), Some(t_out), Some(b_in), Some(b_out)) = (at(yt + 1.0), at(yt - 2.0), at(yb - 1.0), at(yb + 2.0)) {
                *s = s.max((t_in - t_out).min(b_in - b_out));
            }
        }
    }
    let (best, &top_score) = score.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let second = score.iter().enumerate().filter(|(i, _)| *i != best).map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
    if top_score < 10.0 || top_score - second < 8.0 {
        return None;
    }
    Some(best)
}

/// EVE's colour range slider, just above "No filter" in the resource list: where its two small
/// triangle handles sit along the bar, as fractions 0 … 1 of the bar (lowest level shown, highest
/// level shown). (0, 1) = both right out, the full range: only then are two scans' colours the
/// same levels. None when the two handles aren't both seen (panel closed or moved, Build tab).
///
/// The bar's place comes from the list spot: its ends 1.93 rows left and 6.19 rows right of the
/// names' x (measured on Tsa's 1918 × 1024 window at 90 % UI scale), so a list spot clicked far
/// along the names shifts the reading; the handles themselves are found from the picture.
pub fn slider_handles(pic: &Pic, spot: &ListSpot) -> Option<(f64, f64)> {
    let found = slider_boxes(pic, spot)?;
    if found.contrast < 14.0 {
        return None;
    }
    let gap = (spot.y5 - spot.y0) / 5.0;
    let (t0, t1) = (spot.x - 1.93 * gap, spot.x + 6.19 * gap);
    // Within a pixel and a half of an end counts as right out.
    let snap = 1.5 / (t1 - t0);
    let at = |x: f64| {
        let v = ((x - t0) / (t1 - t0)).clamp(0.0, 1.0);
        if v < snap {
            0.0
        } else if v > 1.0 - snap {
            1.0
        } else {
            v
        }
    };
    // Each handle points at the bar with its upright side.
    Some((at((found.left + found.side) as f64), at(found.right as f64)))
}

/// The slider's two handles as found in the picture: each one's little box (left edge x, their
/// top y, the box's side) and how clearly the fainter of the two stands out (grey levels).
#[derive(Clone, Copy, Debug)]
pub struct SliderBoxes {
    pub left: usize,
    pub right: usize,
    pub top: usize,
    pub side: usize,
    pub contrast: f64,
}

/// The handles are light triangles about 0.28 rows across: the left one "◢", the right one "◣",
/// upright sides towards the bar. Each is found by its shape: the lit half of its box against the
/// other half, and against the strip just outside its slanted side (dark for a handle; lit for
/// the colour band's own edge), so neither the band nor a bright sky behind the see-through panel
/// reads as a handle. Both must sit on the same line.
pub fn slider_boxes(pic: &Pic, spot: &ListSpot) -> Option<SliderBoxes> {
    let gap = (spot.y5 - spot.y0) / 5.0;
    if gap < 12.0 {
        return None;
    }
    let n = ((0.28 * gap).round() as usize).max(4);
    let xa = (spot.x - 2.6 * gap).floor().max(4.0) as usize;
    let xb = ((spot.x + 6.9 * gap).ceil().max(0.0) as usize).min(pic.w.saturating_sub(4));
    let ya = (spot.y0 - 1.15 * gap).floor().max(0.0) as usize;
    let yb = ((spot.y0 - 0.45 * gap).ceil().max(0.0) as usize).min(pic.h);
    if xb < xa + 4 * n || yb < ya + n {
        return None;
    }
    let mut best: Option<SliderBoxes> = None;
    for top in ya..=yb - n {
        let mut scores: Vec<(f64, f64)> = Vec::with_capacity(xb - xa);
        for x in xa..=xb - n {
            // Row by row (the top and bottom rows left out, and the slant itself, which is half
            // lit): the lit run against the dark run. A handle shows the step in every row; the
            // colour band's edges and its white end, far brighter than a handle, only in some.
            // So the second-weakest row counts.
            let (mut rows_l, mut rows_r) = (Vec::with_capacity(n), Vec::with_capacity(n));
            let (mut lit_l, mut lit_r) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64));
            // The three columns just outside each side of the box (the brightest of them counts).
            let (mut out_l, mut out_r) = ([0.0f64; 3], [0.0f64; 3]);
            for j in 0..n {
                if j >= 1 && j + 1 < n {
                    let (mut ll, mut dl, mut lr, mut dr) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64), (0.0f64, 0.0f64), (0.0f64, 0.0f64));
                    for i in 0..n {
                        let v = pic.lum(x + i, top + j) as f64;
                        // "◢": lit right of its slant.
                        if i + j >= n {
                            ll = (ll.0 + v, ll.1 + 1.0);
                        } else if i + j + 1 < n {
                            dl = (dl.0 + v, dl.1 + 1.0);
                        }
                        // "◣": lit left of its slant.
                        if j > i {
                            lr = (lr.0 + v, lr.1 + 1.0);
                        } else if j < i {
                            dr = (dr.0 + v, dr.1 + 1.0);
                        }
                    }
                    rows_l.push(ll.0 / ll.1.max(1.0) - dl.0 / dl.1.max(1.0));
                    rows_r.push(lr.0 / lr.1.max(1.0) - dr.0 / dr.1.max(1.0));
                    lit_l = (lit_l.0 + ll.0, lit_l.1 + ll.1);
                    lit_r = (lit_r.0 + lr.0, lit_r.1 + lr.1);
                }
                for k in 0..3 {
                    out_l[k] += pic.lum(x - 2 - k, top + j) as f64;
                    out_r[k] += pic.lum(x + n + 1 + k, top + j) as f64;
                }
            }
            let (out_l, out_r) = (out_l.iter().cloned().fold(0.0, f64::max) / n as f64, out_r.iter().cloned().fold(0.0, f64::max) / n as f64);
            rows_l.sort_by(f64::total_cmp);
            rows_r.sort_by(f64::total_cmp);
            let (lit_l, lit_r) = (lit_l.0 / lit_l.1.max(1.0), lit_r.0 / lit_r.1.max(1.0));
            // Outside a handle's slanted side it's dark; past the left handle's upright side too
            // (the band starts with its darkest colour there; at the right handle it ends white).
            let left = rows_l[1.min(rows_l.len() - 1)].min(lit_l - out_l).min(lit_l - out_r);
            let right = rows_r[1.min(rows_r.len() - 1)].min(lit_r - out_r);
            scores.push((left, right));
        }
        let Some((li, l)) = scores.iter().map(|s| s.0).enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        // The right handle is right of the left one.
        let Some((ri, r)) = scores.iter().map(|s| s.1).enumerate().skip(li + 2 * n).max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        let contrast = l.min(r);
        if best.is_none_or(|b| contrast > b.contrast) {
            best = Some(SliderBoxes { left: xa + li, right: xa + ri, top, side: n, contrast });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A planet of one colour with EVE's band at 0.6 laid over it, painted on several cores:
    /// every cell seen reads 0.6, and the colours seen are kept.
    #[test]
    fn resource_painted_against_the_bare_planet() {
        use crate::planetscan::geom::{pose_q, Disc};
        let bare_rgb = [90.0f32, 80.0, 70.0];
        let bytes: Vec<u8> = (0..GRID_W * GRID_H).flat_map(|_| [90u8, 80, 70]).collect();
        let bare = Map::from_bare_bytes(&bytes).unwrap();
        let level = 0.6f32;
        let c = band_fine()[(level * (STEPS - 1) as f32).round() as usize];
        let seen: Vec<u8> = (0..3).map(|k| ((1.0 - OVERLAY) * bare_rgb[k] + c[k]).round() as u8).collect();
        let (w, h) = (640usize, 400usize);
        let disc = Disc::with_lens(320.0, 200.0, 100.0, 375.0);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                if (x as f64 - 320.0).hypot(y as f64 - 200.0) < 100.0 {
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[seen[2], seen[1], seen[0], 255]);
                }
            }
        }
        let mut m = Map::new(false);
        let painted = m.paint(&Pic { w, h, bgra: &px }, &disc, &pose_q(0.4, 0.3), false, 1, Some(&bare));
        assert!(painted > 100, "{painted}");
        for i in 0..GRID_W * GRID_H {
            if m.seen[i] {
                assert!((m.value[i] - level).abs() < 0.03, "cell {i}: {}", m.value[i]);
                assert!((m.raw[i * 3] - seen[0] as f32).abs() < 1.0, "raw {}", m.raw[i * 3]);
            }
        }
        let raw = m.raw_bytes();
        assert_eq!(raw.len(), 8 + GRID_W * GRID_H * 4);
        // Painting the same picture again changes nothing (each cell keeps its most face-on look).
        assert_eq!(m.paint(&Pic { w, h, bgra: &px }, &disc, &pose_q(0.4, 0.3), false, 1, Some(&bare)), 0);
    }

    /// Glowing lava on the planet map can't be read through: those cells aren't left to see, so a
    /// lava planet's scan reaches 99 % (planet II stopped at 98.9 % with 1.4 % of it glowing).
    #[test]
    fn glowing_lava_is_not_left_to_see() {
        let glows = |i: usize| (80..90).contains(&(i / GRID_W)) && (100..130).contains(&(i % GRID_W));
        let bytes: Vec<u8> = (0..GRID_W * GRID_H).flat_map(|i| if glows(i) { [240u8, 200, 60] } else { [90u8, 80, 70] }).collect();
        let bare = Map::from_bare_bytes(&bytes).unwrap();
        let mut m = Map::new(false);
        let mut plain = Map::new(false);
        for i in 0..GRID_W * GRID_H {
            if !glows(i) {
                m.seen[i] = true;
                plain.seen[i] = true;
            }
        }
        m.version += 1;
        plain.version += 1;
        assert!(plain.coverage() < 0.995, "{}", plain.coverage());
        let share = m.mark_glow(&bare);
        assert!(share > 0.005 && share < 0.01, "{share}");
        assert!(m.coverage() > 0.9999, "{}", m.coverage());
        assert!(!m.missing().iter().any(|&x| x));
        assert!((m.blind_share() - share).abs() < 1e-9);
        // Cells that glow get no level (SeAT gets nothing there, as for unseen).
        assert_eq!(m.cells_1deg()[85 * GRID_W + 110], 0);
    }

    /// Looks that can't be read (EVE's colours not matching the planet's) count only when they go
    /// on for a while: a moment of them (EVE's colours fading in) leaves the cells to see.
    #[test]
    fn unreadable_only_after_a_while() {
        use crate::planetscan::geom::{pose_q, Disc};
        let bytes: Vec<u8> = (0..GRID_W * GRID_H).flat_map(|_| [90u8, 80, 70]).collect();
        let bare = Map::from_bare_bytes(&bytes).unwrap();
        let (w, h) = (640usize, 400usize);
        let disc = Disc::with_lens(320.0, 200.0, 100.0, 375.0);
        // Pure white over the whole planet: no level of EVE's band reads as that.
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                if (x as f64 - 320.0).hypot(y as f64 - 200.0) < 100.0 {
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        let pic = Pic { w, h, bgra: &px };
        let (front, back) = (pose_q(0.4, 0.3), pose_q(0.4 + PI, 0.3));
        let mut m = Map::new(false);
        for _ in 0..6 {
            assert_eq!(m.paint(&pic, &disc, &front, false, 1, Some(&bare)), 0);
        }
        let middle = {
            // The cell straight in front.
            let v = crate::planetscan::geom::apply(&crate::planetscan::geom::transpose(&front), &[0.0, 0.0, 1.0]);
            let (lat, lon) = lat_lon(&v, false);
            ((lat / PI * GRID_H as f64) as usize).min(GRID_H - 1) * GRID_W + ((lon / (2.0 * PI) * GRID_W as f64) as usize).min(GRID_W - 1)
        };
        assert!(!m.known(middle), "six quick looks");
        for _ in 0..40 {
            m.paint(&pic, &disc, &back, false, 1, Some(&bare));
        }
        assert!(!m.known(middle), "no new looks there");
        m.paint(&pic, &disc, &front, false, 1, Some(&bare));
        assert!(m.known(middle) && !m.seen[middle], "unreadable looks spread over a while");
        assert!(m.blind_share() > 0.03, "{}", m.blind_share());
    }

    /// The quick unmixing gives the same level as trying every step, on real-looking colours:
    /// planet colours with the band laid over them at every level, plus a little noise.
    #[test]
    fn quick_unmix_agrees() {
        let (mut n, mut off) = (0, 0);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..20_000 {
            let bare = [20.0 + 180.0 * rnd(), 20.0 + 180.0 * rnd(), 20.0 + 180.0 * rnd()];
            let c = band_fine()[(rnd() * (STEPS - 1) as f32) as usize];
            let px: Vec<u8> = (0..3).map(|k| ((1.0 - OVERLAY) * bare[k] + c[k] + 8.0 * (rnd() - 0.5)).clamp(0.0, 255.0) as u8).collect();
            let (a, b) = (unmix(px[0], px[1], px[2], bare[0], bare[1], bare[2]), unmix_every_step(px[0], px[1], px[2], bare[0], bare[1], bare[2]));
            n += 1;
            if a != b {
                off += 1;
            }
        }
        assert!(off * 200 <= n, "{off} of {n} differ");
    }

    /// A resource on the 0.5° grid: SeAT's 1° cells are the mean of the four in each.
    #[test]
    fn half_degree_cells_average_to_one_degree() {
        let mut m = Map::with_size(false, RES_W, RES_H);
        for i in 0..RES_W * RES_H {
            m.seen[i] = true;
            m.value[i] = if (i % RES_W) % 2 == 0 { 0.2 } else { 0.6 };
        }
        let c = m.cells_1deg();
        assert_eq!(c.len(), GRID_W * GRID_H);
        assert!(c.iter().all(|&v| v == 1 + (0.4f32 * 250.0).round() as u8), "{:?}", &c[..4]);
        assert!((m.coverage() - 1.0).abs() < 1e-9);
        // A lone unseen cell among seen ones is a gap, not a target.
        m.seen[100 * RES_W + 50] = false;
        m.version += 1;
        assert!((m.coverage() - 1.0).abs() < 1e-9 && !m.missing()[100 * RES_W + 50]);
    }

    #[test]
    fn unmix_finds_the_band() {
        // A grey planet with the band's middle laid over it reads as the middle.
        let fine = band_fine();
        let (br, bg, bb) = (80.0, 80.0, 80.0);
        let c = fine[30];
        let k = 1.0 - OVERLAY;
        let level = unmix((k * br + c[0]) as u8, (k * bg + c[1]) as u8, (k * bb + c[2]) as u8, br, bg, bb).unwrap();
        assert!((level - 30.0 / 59.0).abs() < 0.04, "{level}");
        assert!(unmix(250, 250, 250, 230.0, 230.0, 230.0).is_none());
    }

    /// A list like EVE's: dark see-through panel, a name and a bar on each row, the picked row's
    /// box 0.88 rows tall.
    fn list_picture(w: usize, h: usize, sky: impl Fn(usize, usize) -> u8, lit: &[usize]) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut v = sky(x, y) as f32 * 0.6 + 12.0;
                for row in 0..6 {
                    let yc = 63.0 + 25.2 * row as f32;
                    let dy = y as f32 - yc;
                    if dy.abs() < 4.0 && (106..180).contains(&x) && (x / 3 + y) % 4 == 0 {
                        v = 200.0; // a name's letters
                    }
                    if row > 0 && dy.abs() < 4.0 && (208..208 + 20 * row).contains(&x) {
                        v = 240.0; // its bar
                    }
                    if lit.contains(&row) && dy.abs() <= 11.0 && (80..310).contains(&x) {
                        v += 0.35 * (255.0 - v);
                    }
                }
                let k = (y * w + x) * 4;
                let v = v.clamp(0.0, 255.0) as u8;
                px[k..k + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        px
    }

    #[test]
    fn picked_row_finds_the_lit_row() {
        let (w, h) = (420usize, 260usize);
        let spot = ListSpot { x: 140.0, y0: 60.0, y5: 186.0 };
        for row in 0..6 {
            let px = list_picture(w, h, |_, _| 20, &[row]);
            assert_eq!(picked_row(&Pic { w, h, bgra: &px }, &spot), Some(row), "row {row} on a dark sky");
        }
        // Nothing lit: none.
        let px = list_picture(w, h, |_, _| 20, &[]);
        assert_eq!(picked_row(&Pic { w, h, bgra: &px }, &spot), None);
        // One picked, another under the mouse: can't tell, so none.
        let px = list_picture(w, h, |_, _| 20, &[1, 2]);
        assert_eq!(picked_row(&Pic { w, h, bgra: &px }, &spot), None);
    }

    /// EVE's colour range slider as on Tsa's pictures, for a list at x 140, y0 60, y5 186: a ruler
    /// line, the colour band between the handles (dark at the low end, a white block at the high
    /// end, far brighter than the handles), the two see-through triangle handles, a few stars.
    fn slider_picture(w: usize, h: usize, sky: impl Fn(usize, usize) -> u8, handles: Option<(f64, f64)>) -> Vec<u8> {
        let (t0, t1) = (140.0 - 1.93 * 25.2, 140.0 + 6.19 * 25.2);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut c = [sky(x, y) as f32 * 0.6 + 12.0; 3];
                if (x * 7 + y * 13) % 211 == 0 {
                    c = [170.0; 3]; // a star
                }
                if y == 27 && (83..306).contains(&x) {
                    c = [150.0; 3]; // the ruler
                }
                if let Some((lo, hi)) = handles {
                    let (a, b) = ((t0 + lo * (t1 - t0)).round() as usize, (t0 + hi * (t1 - t0)).round() as usize);
                    let (i, j) = (x as i64, y as i64 - 37);
                    // The band: between the handles, rows 37 … 42.
                    if (0..6).contains(&j) && x > a && x + 1 < b {
                        let t = (x - a) as f32 / (b - a) as f32;
                        let band = if t > 0.94 { [200.0; 3] } else { [20.0 + 180.0 * t, 14.0 + 90.0 * (1.0 - (2.0 * t - 1.0).abs()), 25.0] };
                        for k in 0..3 {
                            c[k] = 0.3 * c[k] + band[k];
                        }
                    }
                    // The handles: 7 × 7, rows 37 … 43, the left one's upright side at a − 1, the
                    // right one's at b.
                    let lit = (0..7).contains(&j) && ((i >= a as i64 - 7 && i < a as i64 && (i - (a as i64 - 7)) + j >= 6) || (i >= b as i64 && i < b as i64 + 7 && j >= i - b as i64));
                    if lit {
                        for v in c.iter_mut() {
                            *v += 0.5 * (255.0 - *v);
                        }
                    }
                }
                let k = (y * w + x) * 4;
                px[k..k + 4].copy_from_slice(&[c[2].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[0].clamp(0.0, 255.0) as u8, 255]);
            }
        }
        px
    }

    /// The slider's handles are read where they are: right out reads (0, 1) exactly, pulled in
    /// reads how far, on a dark sky and with a bright nebula behind the panel; no handles, none.
    #[test]
    fn slider_handles_are_read() {
        let (w, h) = (420usize, 260usize);
        let spot = ListSpot { x: 140.0, y0: 60.0, y5: 186.0 };
        let dark = |_: usize, _: usize| 10u8;
        let nebula = |x: usize, y: usize| (120.0 + 80.0 * ((x as f64 / 60.0).sin() * (y as f64 / 45.0).cos())) as u8;
        for (name, sky) in [("dark sky", &dark as &dyn Fn(usize, usize) -> u8), ("nebula", &nebula)] {
            let px = slider_picture(w, h, sky, Some((0.0, 1.0)));
            assert_eq!(slider_handles(&Pic { w, h, bgra: &px }, &spot), Some((0.0, 1.0)), "right out, {name}");
            for want in [(0.25, 0.8), (0.0, 0.5), (0.4, 1.0), (0.3, 0.45)] {
                let px = slider_picture(w, h, sky, Some(want));
                let got = slider_handles(&Pic { w, h, bgra: &px }, &spot).unwrap_or((-1.0, -1.0));
                assert!((got.0 - want.0).abs() < 0.012 && (got.1 - want.1).abs() < 0.012, "{want:?} on {name}: read {got:?}");
                assert!(got != (0.0, 1.0));
            }
            let px = slider_picture(w, h, sky, None);
            assert_eq!(slider_handles(&Pic { w, h, bgra: &px }, &spot), None, "no handles, {name}");
        }
        // No room for a list: none.
        assert_eq!(slider_handles(&Pic { w: 40, h: 40, bgra: &vec![0u8; 40 * 40 * 4] }, &ListSpot { x: 10.0, y0: 5.0, y5: 30.0 }), None);
    }

    /// A bright patch of nebula behind the list (brightest round "No filter" and Heavy Metals)
    /// doesn't make those rows read as picked: the picked row is still the one with the box.
    #[test]
    fn picked_row_on_a_bright_nebula() {
        let (w, h) = (420usize, 260usize);
        let spot = ListSpot { x: 140.0, y0: 60.0, y5: 186.0 };
        let glow = |x: usize, y: usize| -> u8 {
            let d0 = ((x as f64 - 150.0).powi(2) + (y as f64 - 60.0).powi(2)).sqrt();
            let d3 = ((x as f64 - 260.0).powi(2) + (y as f64 - 140.0).powi(2)).sqrt();
            (250.0 * (-d0 / 90.0).exp() + 230.0 * (-d3 / 70.0).exp()).min(255.0) as u8
        };
        for row in [1, 5] {
            let px = list_picture(w, h, glow, &[row]);
            let got = picked_row(&Pic { w, h, bgra: &px }, &spot);
            assert!(got == Some(row) || got.is_none(), "row {row}: read {got:?}");
            assert_ne!(got, Some(3));
            assert_ne!(got, Some(0));
        }
        let px = list_picture(w, h, glow, &[1]);
        assert_eq!(picked_row(&Pic { w, h, bgra: &px }, &spot), Some(1));
    }
}
