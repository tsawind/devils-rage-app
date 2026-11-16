//! The planet's own colours in detail (EVE on No filter): the base map. 2048 × 1024 cells
//! (0.18° each, about what a fully zoomed-out view shows at the middle of the planet), in the
//! sky's frame: north is the planet's axis, longitude 0 wherever the system's sky started.
//!
//! Each cell is the average of every look at it, weighted hard towards the most face-on looks
//! (sharper than the edge-on ones). EVE's planets don't turn and their sun stays put, so a place
//! looks the same from every side; its night side stays dark.

use super::geom::{apply, facing_of, lat_lon, to_sphere, transpose, Disc, M3};
use super::sky::Pic;
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
        Base { w: BASE_W, h: BASE_H, sum: vec![[0.0; 3]; n], weight: vec![0.0; n], best: vec![0.0; n], version: 0, spots: None }
    }

    #[inline]
    fn cell(&self, lat: f64, lon: f64) -> usize {
        let row = ((lat / PI * self.h as f64) as usize).min(self.h - 1);
        let col = ((lon / (2.0 * PI) * self.w as f64) as usize).min(self.w - 1);
        row * self.w + col
    }

    /// Paint one picture at facing Q. Returns the cells it touched.
    pub fn paint(&mut self, pic: &Pic, disc: &Disc, q: &M3) -> usize {
        let key = SpotsKey { w: pic.w, h: pic.h, disc: [disc.cx.to_bits(), disc.cy.to_bits(), disc.r.to_bits(), disc.d.to_bits()] };
        if self.spots.as_ref().is_none_or(|(k, _)| *k != key) {
            self.spots = Some((key, spots_for(pic.w, pic.h, disc)));
        }
        let qt = transpose(q);
        let m: [f32; 9] = std::array::from_fn(|i| qt[i] as f32);
        let (wf, hf) = (self.w as f32, self.h as f32);
        let (w, h) = (self.w, self.h);
        let Some((_, spots)) = self.spots.as_ref() else { return 0 };
        let mut touched = 0;
        for sp in spots {
            let v = sp.v;
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
    use crate::planetscan::geom::pose_q;

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
            b.paint(&Pic { w, h, bgra: &pic }, &disc, &q);
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
        assert!(b.paint(&Pic { w, h, bgra: &px }, &disc, &pose_q(0.3, 0.2)) > 1000);
        let back = Base::from_bytes(&b.to_bytes()).unwrap();
        assert_eq!(back.rgb(), b.rgb());
        assert!((back.coverage() - b.coverage()).abs() < 1e-9);
        assert!(b.coverage() > 0.01 && b.coverage() < 0.2);
    }
}
