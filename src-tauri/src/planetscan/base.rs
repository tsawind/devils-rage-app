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
}

impl Default for Base {
    fn default() -> Self {
        Base::new()
    }
}

impl Base {
    pub fn new() -> Base {
        let n = BASE_W * BASE_H;
        Base { w: BASE_W, h: BASE_H, sum: vec![[0.0; 3]; n], weight: vec![0.0; n], best: vec![0.0; n], version: 0 }
    }

    #[inline]
    fn cell(&self, lat: f64, lon: f64) -> usize {
        let row = ((lat / PI * self.h as f64) as usize).min(self.h - 1);
        let col = ((lon / (2.0 * PI) * self.w as f64) as usize).min(self.w - 1);
        row * self.w + col
    }

    /// Paint one picture at facing Q. Returns the cells it touched.
    pub fn paint(&mut self, pic: &Pic, disc: &Disc, q: &M3) -> usize {
        let qt = transpose(q);
        let (y0, y1) = ((disc.cy - disc.r).floor().max(0.0) as usize, ((disc.cy + disc.r).ceil() as usize).min(pic.h - 1));
        let (x0, x1) = ((disc.cx - disc.r).floor().max(0.0) as usize, ((disc.cx + disc.r).ceil() as usize).min(pic.w - 1));
        let mut touched = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Some(v) = to_sphere(x as f64 + 0.5, y as f64 + 0.5, disc) else { continue };
                let f = facing_of(&v, disc);
                if f < BASE_FACING {
                    continue;
                }
                let (lat, lon) = lat_lon(&apply(&qt, &v), false);
                let c = self.cell(lat, lon);
                if self.best[c] >= SETTLED && (f as f32) < self.best[c] {
                    continue;
                }
                let (r, g, b) = pic.rgb(x, y);
                // Face-on looks count far more: 0.6 → 0.05, 0.8 → 0.26, 1.0 → 1.
                let wgt = (f * f * f * f * f * f) as f32;
                let s = &mut self.sum[c];
                s[0] += r as f32 * wgt;
                s[1] += g as f32 * wgt;
                s[2] += b as f32 * wgt;
                self.weight[c] += wgt;
                if f as f32 > self.best[c] {
                    self.best[c] = f as f32;
                }
                touched += 1;
            }
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
    pub fn coverage(&self) -> f64 {
        let (mut seen, mut all) = (0.0, 0.0);
        for row in 0..self.h {
            let wgt = ((row as f64 + 0.5) / self.h as f64 * PI).sin();
            let start = row * self.w;
            let n = (start..start + self.w).filter(|&i| self.weight[i] > 0.0 || self.gap(i)).count();
            seen += wgt * n as f64;
            all += wgt * self.w as f64;
        }
        seen / all
    }

    /// As `rgb`, with the gaps between pixels filled from their neighbours (for showing).
    pub fn rgb_filled(&self) -> Vec<u8> {
        let mut out = self.rgb();
        for i in 0..self.w * self.h {
            if self.weight[i] > 0.0 || !self.gap(i) {
                continue;
            }
            let (row, col) = (i / self.w, i % self.w);
            let (mut acc, mut n) = ([0u32; 3], 0u32);
            for dr in [-1isize, 0, 1] {
                for dc in [-1isize, 0, 1] {
                    let r = row as isize + dr;
                    let c = (col as isize + dc).rem_euclid(self.w as isize);
                    let k = r as usize * self.w + c as usize;
                    if self.weight[k] > 0.0 {
                        for ch in 0..3 {
                            acc[ch] += (self.sum[k][ch] / self.weight[k]) as u32;
                        }
                        n += 1;
                    }
                }
            }
            if n > 0 {
                for ch in 0..3 {
                    out[i * 3 + ch] = (acc[ch] / n).clamp(if ch == 0 { 1 } else { 0 }, 255) as u8;
                }
            }
        }
        out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planetscan::geom::pose_q;

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
