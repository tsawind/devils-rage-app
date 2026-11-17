//! The planet's own colours in detail (EVE on No filter): the base map. 2048 × 1024 cells
//! (0.18° each, about what a fully zoomed-out view shows at the middle of the planet), in the
//! sky's frame: north is the planet's axis, longitude 0 wherever the system's sky started.
//!
//! Each cell is the average of every look at it, weighted hard towards the most face-on looks
//! (sharper than the edge-on ones). EVE's planets don't turn and their sun stays put, so a place
//! looks the same from every side; its night side stays dark.

use super::geom::{apply, facing_of, pose_q, to_sphere, transpose, Disc, M3};
use super::rings::Rings;
use super::scan::{Map, GRID_H, GRID_W};
use super::sky::{axis_for_pitch, Pic, DEG};
use std::f64::consts::PI;

pub const BASE_W: usize = 2048;
pub const BASE_H: usize = 1024;
/// Less face-on than this isn't painted (the edge is smeared and stretched).
pub const BASE_FACING: f64 = 0.6;
/// A cell this face-on is as good as it gets: no more painting there.
const SETTLED: f32 = 0.985;

pub struct Base {
    pub w: usize,
    pub h: usize,
    sum: Vec<[f32; 3]>,
    weight: Vec<f32>,
    best: Vec<f32>,
    pub version: u64,
    /// The planet's pixels for the current edge and picture size: worked out once, not per picture.
    spots: Option<(SpotsKey, Vec<Spot>)>,
    /// The map's detail (each cell's brightness minus its surroundings), for placing a picture
    /// on the planet itself; made again once the map has moved on.
    detail: Option<(u64, Vec<f32>)>,
}

/// A picture placed on the planet map: the facing, how well it matched (correlation of the
/// detail), and the share of the planet's points that landed on mapped cells.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub yaw: f64,
    pub pitch: f64,
    pub score: f64,
    pub seen: f64,
}

/// A picture pixel on the planet, face-on enough to paint: where it is and which way it points
/// (camera space), and how face-on it is.
struct Spot {
    pixel: u32,
    v: [f32; 3],
    facing: f32,
}

#[derive(Clone, Copy, PartialEq)]
struct SpotsKey {
    w: usize,
    h: usize,
    disc: [u64; 4],
}

fn spots_for(w: usize, h: usize, disc: &Disc) -> Vec<Spot> {
    let (y0, y1) = ((disc.cy - disc.r).floor().max(0.0) as usize, ((disc.cy + disc.r).ceil() as usize).min(h - 1));
    let (x0, x1) = ((disc.cx - disc.r).floor().max(0.0) as usize, ((disc.cx + disc.r).ceil() as usize).min(w - 1));
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let Some(v) = to_sphere(x as f64 + 0.5, y as f64 + 0.5, disc) else { continue };
            let f = facing_of(&v, disc);
            if f >= BASE_FACING {
                out.push(Spot { pixel: (y * w + x) as u32, v: [v[0] as f32, v[1] as f32, v[2] as f32], facing: f as f32 });
            }
        }
    }
    out
}

/// atan2 to about 1e-5 rad (0.0006°), several times faster than the exact one: plenty for 0.18° cells.
#[inline]
fn fast_atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (x.abs(), y.abs());
    let big = ax.max(ay);
    if big == 0.0 {
        return 0.0;
    }
    let a = ax.min(ay) / big;
    let s = a * a;
    let mut r = a * (0.999_866_0 + s * (-0.330_299_5 + s * (0.180_141_0 + s * (-0.085_133_0 + s * 0.020_835_1))));
    if ay > ax {
        r = std::f32::consts::FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = std::f32::consts::PI - r;
    }
    if y < 0.0 {
        -r
    } else {
        r
    }
}

impl Default for Base {
    fn default() -> Self {
        Base::new()
    }
}

impl Base {
    pub fn new() -> Base {
        let n = BASE_W * BASE_H;
        Base { w: BASE_W, h: BASE_H, sum: vec![[0.0; 3]; n], weight: vec![0.0; n], best: vec![0.0; n], version: 0, spots: None, detail: None }
    }

    #[cfg(test)]
    fn cell(&self, lat: f64, lon: f64) -> usize {
        let row = ((lat / PI * self.h as f64) as usize).min(self.h - 1);
        let col = ((lon / (2.0 * PI) * self.w as f64) as usize).min(self.w - 1);
        row * self.w + col
    }

    fn spots(&mut self, w: usize, h: usize, disc: &Disc) {
        let key = SpotsKey { w, h, disc: [disc.cx.to_bits(), disc.cy.to_bits(), disc.r.to_bits(), disc.d.to_bits()] };
        if self.spots.as_ref().is_none_or(|(k, _)| *k != key) {
            self.spots = Some((key, spots_for(w, h, disc)));
        }
    }

    /// Paint one picture at facing Q. Returns the cells it touched. rings: a gas giant's rings and
    /// the pitch, so the parts of the planet behind them aren't painted. poor_only: only cells not
    /// seen face-on yet (a facing from the planet itself, not the sky, fills gaps but doesn't
    /// blur what the sky's facings painted).
    pub fn paint(&mut self, pic: &Pic, disc: &Disc, q: &M3, rings: Option<(&Rings, f64)>, poor_only: bool) -> usize {
        self.spots(pic.w, pic.h, disc);
        // Where the camera's ray to a planet point crosses the ring plane first (t in 0 … 1 along
        // camera → point): behind a ring there, it isn't painted.
        let ring = rings.filter(|(r, _)| r.any()).map(|(r, pitch)| (*r, axis_for_pitch(pitch)));
        let d = disc.d;
        let qt = transpose(q);
        let m: [f32; 9] = std::array::from_fn(|i| qt[i] as f32);
        let (wf, hf) = (self.w as f32, self.h as f32);
        let (w, h) = (self.w, self.h);
        let Some((_, spots)) = self.spots.as_ref() else { return 0 };
        let mut touched = 0;
        for sp in spots {
            let v = sp.v;
            if let Some((rg, n)) = &ring {
                let dir = [v[0] as f64, v[1] as f64, v[2] as f64 - d];
                let nd = n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2];
                if nd.abs() > 1e-9 {
                    let t = -d * n[2] / nd;
                    if t > 0.0 && t < 1.0 {
                        let x = [t * dir[0], t * dir[1], d + t * dir[2]];
                        if rg.cut_at((x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()) {
                            continue;
                        }
                    }
                }
            }
            let p = [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]];
            // As geom::lat_lon(p, false): latitude from the north pole, longitude 0..2π.
            let lat = fast_atan2((p[0] * p[0] + p[1] * p[1]).sqrt(), p[2]);
            let mut lon = fast_atan2(p[1], p[0]);
            if lon < 0.0 {
                lon += 2.0 * std::f32::consts::PI;
            }
            let row = ((lat / std::f32::consts::PI * hf) as usize).min(h - 1);
            let col = ((lon / (2.0 * std::f32::consts::PI) * wf) as usize).min(w - 1);
            let c = row * w + col;
            let f = sp.facing;
            if self.best[c] >= SETTLED && f < self.best[c] {
                continue;
            }
            if poor_only && self.best[c] >= 0.9 {
                continue;
            }
            let i = sp.pixel as usize * 4;
            let (b, g, r) = (pic.bgra[i] as f32, pic.bgra[i + 1] as f32, pic.bgra[i + 2] as f32);
            // Face-on looks count far more: 0.6 → 0.05, 0.8 → 0.26, 1.0 → 1.
            let f2 = f * f;
            let wgt = f2 * f2 * f2;
            let s = &mut self.sum[c];
            s[0] += r * wgt;
            s[1] += g * wgt;
            s[2] += b * wgt;
            self.weight[c] += wgt;
            if f > self.best[c] {
                self.best[c] = f;
            }
            touched += 1;
        }
        if touched > 0 {
            self.version += 1;
        }
        touched
    }

    /// The map's detail: each seen cell's brightness minus the mean of the seen cells round it
    /// (9 × 9; NaN where not seen). Made again when the map has had 40 more pictures.
    fn detail(&mut self) -> &Vec<f32> {
        if self.detail.as_ref().is_none_or(|(v, _)| self.version >= v + 40 || self.version < *v) {
            let (w, h) = (self.w, self.h);
            let n = w * h;
            let mut lum = vec![0f32; n];
            let mut seen = vec![0f32; n];
            for i in 0..n {
                let wt = self.weight[i];
                if wt > 0.0 {
                    let s = self.sum[i];
                    lum[i] = (0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2]) / wt;
                    seen[i] = 1.0;
                }
            }
            // Box sums, one direction at a time (longitude wraps round).
            let boxed = |src: &[f32], k: usize| -> Vec<f32> {
                let mut across = vec![0f32; n];
                for row in 0..h {
                    let r = &src[row * w..(row + 1) * w];
                    let mut acc: f32 = (0..=2 * k).map(|j| r[(j + w - k) % w]).sum();
                    for col in 0..w {
                        across[row * w + col] = acc;
                        acc += r[(col + k + 1) % w] - r[(col + w - k) % w];
                    }
                }
                let mut out = vec![0f32; n];
                for col in 0..w {
                    for row in 0..h {
                        let (a, b) = (row.saturating_sub(k), (row + k).min(h - 1));
                        let mut acc = 0.0;
                        for r in a..=b {
                            acc += across[r * w + col];
                        }
                        out[row * w + col] = acc;
                    }
                }
                out
            };
            // The detail: a 3 × 3 mean minus the 9 × 9 one round it (fine noise and broad shading out).
            let masked: Vec<f32> = (0..n).map(|i| lum[i] * seen[i]).collect();
            let (l1, n1, l4, n4) = (boxed(&masked, 1), boxed(&seen, 1), boxed(&masked, 4), boxed(&seen, 4));
            let out: Vec<f32> = (0..n).map(|i| if n1[i] >= 5.0 && n4[i] >= 40.0 { l1[i] / n1[i] - l4[i] / n4[i] } else { f32::NAN }).collect();
            self.detail = Some((self.version, out));
        }
        &self.detail.as_ref().unwrap().1
    }

    /// Place a picture on the planet map itself (when too little sky shows, as past a gas giant's
    /// broad rings): the detail of up to 3000 of the planet's points (face-on enough to paint)
    /// against the map's, searched round a guess (± span) and refined to 1/16 of a degree. The
    /// map has to be painted under at least 40 % of the points.
    pub fn place(&mut self, pic: &Pic, disc: &Disc, around: (f64, f64), span: f64) -> Option<Placed> {
        self.spots(pic.w, pic.h, disc);
        // The picture's detail at each point: its brightness minus the box round it, the box about
        // as wide on the planet as the map's (a pixel near the middle covers (D − 1) / f radians).
        let f = disc.r * (disc.d * disc.d - 1.0).sqrt();
        let px_deg = (disc.d - 1.0) / f / DEG;
        let cells = (180.0 / BASE_H as f64) / px_deg;
        let k = ((4.0 * cells).round() as i64).clamp(2, 12);
        let k1 = ((1.0 * cells).round() as i64).clamp(0, 3);
        let samples: Vec<([f32; 3], f32)> = {
            let (_, spots) = self.spots.as_ref()?;
            let every = (spots.len() / 3000).max(1);
            spots
                .iter()
                .step_by(every)
                .filter_map(|sp| {
                    let (x, y) = ((sp.pixel as usize % pic.w) as i64, (sp.pixel as usize / pic.w) as i64);
                    if x < k || y < k || x + k >= pic.w as i64 || y + k >= pic.h as i64 {
                        return None;
                    }
                    let (mut acc, mut near) = (0u32, 0u32);
                    for yy in y - k..=y + k {
                        for xx in x - k..=x + k {
                            let l = pic.lum(xx as usize, yy as usize);
                            acc += l;
                            if (xx - x).abs() <= k1 && (yy - y).abs() <= k1 {
                                near += l;
                            }
                        }
                    }
                    let mean = acc as f32 / ((2 * k + 1) * (2 * k + 1)) as f32;
                    Some((sp.v, near as f32 / ((2 * k1 + 1) * (2 * k1 + 1)) as f32 - mean))
                })
                .collect()
        };
        if samples.len() < 200 {
            return None;
        }
        let (w, h) = (self.w, self.h);
        let detail = self.detail();
        let score = |yaw: f64, pitch: f64| -> (f64, f64) {
            let qt = transpose(&pose_q(yaw, pitch));
            let m: [f32; 9] = std::array::from_fn(|i| qt[i] as f32);
            let (mut c, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for (v, fv) in &samples {
                let p = [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]];
                let lat = fast_atan2((p[0] * p[0] + p[1] * p[1]).sqrt(), p[2]);
                let mut lon = fast_atan2(p[1], p[0]);
                if lon < 0.0 {
                    lon += 2.0 * std::f32::consts::PI;
                }
                let row = ((lat / std::f32::consts::PI * h as f32) as usize).min(h - 1);
                let col = ((lon / (2.0 * std::f32::consts::PI) * w as f32) as usize).min(w - 1);
                let mv = detail[row * w + col];
                if mv.is_nan() {
                    continue;
                }
                let (a, b) = (*fv as f64, mv as f64);
                c += 1.0;
                sa += a;
                sb += b;
                saa += a * a;
                sbb += b * b;
                sab += a * b;
            }
            let seen = c / samples.len() as f64;
            if c < 100.0 {
                return (f64::NAN, seen);
            }
            let (va, vb) = (saa - sa * sa / c, sbb - sb * sb / c);
            if va <= 0.0 || vb <= 0.0 {
                return (f64::NAN, seen);
            }
            ((sab - sa * sb / c) / (va * vb).sqrt(), seen)
        };
        let mut best: Option<Placed> = None;
        let grid = |cy: f64, cp: f64, half: i64, st: f64, best: &mut Option<Placed>| {
            for j in -half..=half {
                for i in -half..=half {
                    let (yaw, pitch) = (cy + i as f64 * st, cp + j as f64 * st);
                    let (sc, seen) = score(yaw, pitch);
                    if sc.is_finite() && seen >= 0.4 && best.is_none_or(|b| sc > b.score) {
                        *best = Some(Placed { yaw, pitch, score: sc, seen });
                    }
                }
            }
        };
        let step = 0.5 * DEG;
        grid(around.0, around.1, (span / step).round().max(1.0) as i64, step, &mut best);
        let mut st = step / 2.0;
        for _ in 0..3 {
            if let Some(b) = best {
                grid(b.yaw, b.pitch, 1, st, &mut best);
            }
            st /= 2.0;
        }
        best
    }

    /// A cell no picture pixel landed on, but most of its neighbours are seen: a gap between
    /// pixels, not a part of the planet still to see.
    fn gap(&self, i: usize) -> bool {
        let (row, col) = (i / self.w, i % self.w);
        if row == 0 || row + 1 == self.h {
            return false;
        }
        let mut seen = 0;
        for dr in [self.w as isize * -1, 0, self.w as isize] {
            for dc in [-1isize, 0, 1] {
                let c = ((col as isize + dc).rem_euclid(self.w as isize)) as usize;
                let k = (i as isize - col as isize + dr) as usize + c;
                if k != i && self.weight[k] > 0.0 {
                    seen += 1;
                }
            }
        }
        seen >= 5
    }

    /// Share of the planet's surface seen (area-weighted; gaps between pixels count as seen).
    /// Every other row and column: a quarter of the work, the same answer within a fraction of a %.
    pub fn coverage(&self) -> f64 {
        let (mut seen, mut all) = (0.0, 0.0);
        for row in (1..self.h).step_by(2) {
            let wgt = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            let start = row * self.w;
            let n = (start..start + self.w).step_by(2).filter(|&i| self.weight[i] > 0.0 || self.gap(i)).count();
            seen += wgt * n as f64;
            all += wgt * self.w.div_ceil(2) as f64;
        }
        seen / all
    }

    /// r, g, b a cell (0, 0, 0 = not seen; a seen cell's red is at least 1).
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h * 3];
        for (i, (s, &w)) in self.sum.iter().zip(&self.weight).enumerate() {
            if w > 0.0 {
                out[i * 3] = (s[0] / w).round().clamp(1.0, 255.0) as u8;
                out[i * 3 + 1] = (s[1] / w).round().clamp(0.0, 255.0) as u8;
                out[i * 3 + 2] = (s[2] / w).round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    /// As `rgb`, with the gaps between pixels filled from their neighbours (for showing).
    pub fn rgb_filled(&self) -> Vec<u8> {
        let mut out = self.rgb();
        fill_gaps(&mut out, self.w, self.h);
        out
    }

    /// "BAS1", width, height (u16 LE), then r, g, b a cell (0, 0, 0 = not seen).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"BAS1".to_vec();
        out.extend_from_slice(&(self.w as u16).to_le_bytes());
        out.extend_from_slice(&(self.h as u16).to_le_bytes());
        out.extend(self.rgb());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Base> {
        if bytes.len() < 8 || &bytes[0..4] != b"BAS1" {
            return None;
        }
        let w = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let h = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if (w, h) != (BASE_W, BASE_H) || bytes.len() != 8 + w * h * 3 {
            return None;
        }
        let mut base = Base::new();
        let rgb = &bytes[8..];
        for i in 0..w * h {
            if rgb[i * 3] == 0 && rgb[i * 3 + 1] == 0 && rgb[i * 3 + 2] == 0 {
                continue;
            }
            // As if seen once, fairly face-on (later looks still blend in).
            let b = 0.9f32;
            let wgt = b.powi(6);
            base.sum[i] = [rgb[i * 3] as f32 * wgt, rgb[i * 3 + 1] as f32 * wgt, rgb[i * 3 + 2] as f32 * wgt];
            base.weight[i] = wgt;
            base.best[i] = b;
        }
        base.version = 1;
        Some(base)
    }

    /// The planet's own colours on the resource scans' 1° grid (each 1° cell the mean of the map
    /// cells in it), to unmix EVE's scan colours against. In the same frame as this map.
    pub fn bare_map(&self) -> Map {
        let n = GRID_W * GRID_H;
        let mut sum = vec![[0f64; 4]; n];
        for row in 0..self.h {
            let gr = (row * GRID_H / self.h).min(GRID_H - 1);
            for col in 0..self.w {
                let i = row * self.w + col;
                let wt = self.weight[i];
                if wt <= 0.0 {
                    continue;
                }
                let gc = (col * GRID_W / self.w).min(GRID_W - 1);
                let s = &mut sum[gr * GRID_W + gc];
                let c = self.sum[i];
                s[0] += (c[0] / wt) as f64;
                s[1] += (c[1] / wt) as f64;
                s[2] += (c[2] / wt) as f64;
                s[3] += 1.0;
            }
        }
        let mut bytes = vec![0u8; n * 3];
        for (k, s) in sum.iter().enumerate() {
            if s[3] > 0.0 {
                bytes[k * 3] = (s[0] / s[3]).round().clamp(1.0, 255.0) as u8;
                bytes[k * 3 + 1] = (s[1] / s[3]).round().clamp(0.0, 255.0) as u8;
                bytes[k * 3 + 2] = (s[2] / s[3]).round().clamp(0.0, 255.0) as u8;
            }
        }
        Map::from_bare_bytes(&bytes).unwrap_or_else(|| Map::new(true))
    }

    /// The patches still to see (purple on the map), for the green targets: on a 256 × 128 grid,
    /// a grid cell is missing when a quarter of the map cells sampled in it aren't seen.
    pub fn targets(&self, max: usize) -> Vec<Target> {
        let (gw, gh) = (256usize, 128usize);
        let (sx, sy) = (self.w / gw, self.h / gh);
        let mut missing = vec![false; gw * gh];
        for gr in 0..gh {
            for gc in 0..gw {
                let (mut n, mut gone) = (0, 0);
                for y in (gr * sy..(gr + 1) * sy).step_by(2) {
                    for x in (gc * sx..(gc + 1) * sx).step_by(2) {
                        let i = y * self.w + x;
                        n += 1;
                        if self.weight[i] == 0.0 && !self.gap(i) {
                            gone += 1;
                        }
                    }
                }
                missing[gr * gw + gc] = gone * 4 >= n;
            }
        }
        patches(gw, gh, &missing, max)
    }

    /// Where the biggest part not seen yet is, from the camera's side: a few words for the pilot.
    pub fn missing_hint(&self, q: &M3) -> Option<String> {
        let (mut sx, mut sy, mut sz, mut n) = (0.0, 0.0, 0.0, 0.0);
        for row in (0..self.h).step_by(8) {
            let lat = (row as f64 + 0.5) / self.h as f64 * PI;
            let wgt = lat.sin();
            for col in (0..self.w).step_by(8) {
                if self.weight[row * self.w + col] > 0.0 {
                    continue;
                }
                let lon = (col as f64 + 0.5) / self.w as f64 * 2.0 * PI;
                let v = apply(q, &[lat.sin() * lon.cos(), lat.sin() * lon.sin(), lat.cos()]);
                sx += v[0] * wgt;
                sy += v[1] * wgt;
                sz += v[2] * wgt;
                n += wgt;
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

/// A patch still to see, for the green targets: its middle (latitude from the north pole and
/// longitude, degrees, as the maps are drawn) and its size (the radius of a circle as big, degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub lat: f64,
    pub lon: f64,
    pub radius: f64,
}

/// The patches on a coarse grid (gw × gh, rows from the north pole, longitude wrapping round):
/// touching missing cells make one patch. The biggest `max` of them, biggest first.
pub fn patches(gw: usize, gh: usize, missing: &[bool], max: usize) -> Vec<Target> {
    let mut seen = vec![false; gw * gh];
    let mut out: Vec<(f64, Target)> = Vec::new();
    let cell_area = |row: usize| (((row as f64 + 0.5) / gh as f64) * PI).sin() * (PI / gh as f64) * (2.0 * PI / gw as f64);
    for start in 0..gw * gh {
        if !missing[start] || seen[start] {
            continue;
        }
        let (mut area, mut v) = (0.0, [0.0f64; 3]);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(k) = stack.pop() {
            let (row, col) = (k / gw, k % gw);
            let a = cell_area(row);
            let (lat, lon) = ((row as f64 + 0.5) / gh as f64 * PI, (col as f64 + 0.5) / gw as f64 * 2.0 * PI);
            area += a;
            v[0] += a * lat.sin() * lon.cos();
            v[1] += a * lat.sin() * lon.sin();
            v[2] += a * lat.cos();
            let mut next = vec![row * gw + (col + 1) % gw, row * gw + (col + gw - 1) % gw];
            if row > 0 {
                next.push((row - 1) * gw + col);
            }
            if row + 1 < gh {
                next.push((row + 1) * gw + col);
            }
            for n in next {
                if missing[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        let lat = (v[2] / l).clamp(-1.0, 1.0).acos();
        let mut lon = v[1].atan2(v[0]);
        if lon < 0.0 {
            lon += 2.0 * PI;
        }
        // A cap of the same area: 2π(1 − cos r).
        let radius = (1.0 - area / (2.0 * PI)).clamp(-1.0, 1.0).acos();
        out.push((area, Target { lat: lat / PI * 180.0, lon: lon / PI * 180.0, radius: radius / PI * 180.0 }));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().take(max).map(|(_, t)| t).collect()
}

/// Fill the gaps between pixels in r, g, b bytes (0, 0, 0 = not seen): an unseen cell with at
/// least 5 of its 8 neighbours seen takes their average. For showing only.
pub fn fill_gaps(rgb: &mut [u8], w: usize, h: usize) {
    let seen = |rgb: &[u8], k: usize| rgb[k * 3] != 0 || rgb[k * 3 + 1] != 0 || rgb[k * 3 + 2] != 0;
    let src = rgb.to_vec();
    for row in 1..h.saturating_sub(1) {
        for col in 0..w {
            let i = row * w + col;
            if seen(&src, i) {
                continue;
            }
            let (mut acc, mut n) = ([0u32; 3], 0u32);
            for dr in [-1isize, 0, 1] {
                for dc in [-1isize, 0, 1] {
                    if dr == 0 && dc == 0 {
                        continue;
                    }
                    let c = (col as isize + dc).rem_euclid(w as isize) as usize;
                    let k = (row as isize + dr) as usize * w + c;
                    if seen(&src, k) {
                        for ch in 0..3 {
                            acc[ch] += src[k * 3 + ch] as u32;
                        }
                        n += 1;
                    }
                }
            }
            if n >= 5 {
                for ch in 0..3 {
                    rgb[i * 3 + ch] = (acc[ch] / n).clamp(if ch == 0 { 1 } else { 0 }, 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planetscan::geom::lat_lon;

    /// Two patches still to see, one across longitude 0: each found where it is and about its size.
    #[test]
    fn targets_where_the_gaps_are() {
        let (gw, gh) = (360usize, 180usize);
        let mut missing = vec![false; gw * gh];
        // 10° × 10° at latitude 40–50 from the north pole, longitude 100–110.
        for r in 40..50 {
            for c in 100..110 {
                missing[r * gw + c] = true;
            }
        }
        // A smaller one round longitude 0 (355–5), latitude 120–126.
        for r in 120..126 {
            for c in (355..360).chain(0..5) {
                missing[r * gw + c] = true;
            }
        }
        let t = patches(gw, gh, &missing, 6);
        assert_eq!(t.len(), 2, "{t:?}");
        assert!((t[0].lat - 45.0).abs() < 0.5 && (t[0].lon - 105.0).abs() < 0.5, "{t:?}");
        assert!(t[0].radius > 4.0 && t[0].radius < 5.0, "{t:?}");
        assert!((t[1].lat - 123.0).abs() < 0.5 && (t[1].lon < 0.5 || t[1].lon > 359.5), "{t:?}");
        // Nothing seen: one patch, all of it.
        let b = Base::new();
        let all = b.targets(6);
        assert_eq!(all.len(), 1);
        assert!(all[0].radius > 179.0, "{all:?}");
    }

    #[test]
    fn fast_atan2_is_close() {
        for k in 0..20000 {
            let a = k as f32 * 0.000_314_159 * 7.0;
            let (y, x) = (a.sin() * (1.0 + (k % 7) as f32), a.cos() * (1.0 + (k % 5) as f32));
            assert!((fast_atan2(y, x) - y.atan2(x)).abs() < 2e-5, "{y} {x}");
        }
        assert_eq!(fast_atan2(0.0, 0.0), 0.0);
        assert!((fast_atan2(1.0, 0.0) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((fast_atan2(0.0, -1.0) - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn painted_where_the_exact_maths_puts_it() {
        // One bright pixel on the planet lands in the cell geom::lat_lon gives it.
        let (w, h) = (640usize, 400usize);
        let disc = Disc::with_lens(320.0, 200.0, 100.0, 375.0);
        for &(px, py, yaw, pitch) in &[(330usize, 190usize, 0.3, 0.2), (280, 240, -2.0, 1.1), (350, 170, 3.0, -0.7)] {
            let mut pic = vec![0u8; w * h * 4];
            pic[(py * w + px) * 4..(py * w + px) * 4 + 4].copy_from_slice(&[0, 0, 200, 255]);
            let mut b = Base::new();
            let q = pose_q(yaw, pitch);
            b.paint(&Pic { w, h, bgra: &pic }, &disc, &q, None, false);
            let v = to_sphere(px as f64 + 0.5, py as f64 + 0.5, &disc).unwrap();
            let (lat, lon) = lat_lon(&apply(&transpose(&q), &v), false);
            let want = b.cell(lat, lon);
            let rgb = b.rgb();
            assert!(rgb[want * 3] > 150, "pixel ({px}, {py}) not in its cell");
        }
    }

    #[test]
    fn bytes_round_trip() {
        let mut b = Base::new();
        let (w, h) = (640usize, 400usize);
        let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 200) as u8 + 20, 90, 40, 255]).collect();
        let disc = Disc::with_lens(320.0, 200.0, 100.0, 375.0);
        assert!(b.paint(&Pic { w, h, bgra: &px }, &disc, &pose_q(0.3, 0.2), None, false) > 1000);
        let back = Base::from_bytes(&b.to_bytes()).unwrap();
        assert_eq!(back.rgb(), b.rgb());
        assert!((back.coverage() - b.coverage()).abs() < 1e-9);
        assert!(b.coverage() > 0.01 && b.coverage() < 0.2);
    }
}
