//! Reading EVE's scan colours onto a whole-planet map (one cell per degree, the size SeAT keeps),
//! the bare planet's own colours (for unmixing), and EVE's resource list (which row is picked).

use super::geom::{apply, facing_of, lat_lon, planet_vector, to_sphere, transpose, Disc, M3};
use super::sky::Pic;
use std::f64::consts::PI;

pub const GRID_W: usize = 360;
pub const GRID_H: usize = 180;

/// Paint from within about 37° of the disc's middle (facing ≥ 0.8); a cell keeps its most
/// face-on look, and is settled once seen at 0.97.
pub const PAINT_FACING: f64 = 0.8;
const SETTLED: f32 = 0.97;

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

/// Resource level 0 … 1 at a pixel, knowing the bare planet's colour there; None where the
/// planet is too bright to see through (glowing lava).
pub fn unmix(r: u8, g: u8, b: u8, br: f32, bg: f32, bb: f32) -> Option<f32> {
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
    /// Resource: level; bare: r, g, b.
    pub value: Vec<f32>,
    pub seen: Vec<bool>,
    pub best: Vec<f32>,
    pub version: u64,
}

impl Map {
    pub fn new(bare: bool) -> Map {
        let n = GRID_W * GRID_H;
        Map { bare, value: vec![0.0; if bare { n * 3 } else { n }], seen: vec![false; n], best: vec![0.0; n], version: 0 }
    }
    pub fn coverage(&self) -> f64 {
        let (mut seen, mut all) = (0.0, 0.0);
        for row in 0..GRID_H {
            let w = ((row as f64 + 0.5) / GRID_H as f64 * PI).sin();
            for col in 0..GRID_W {
                all += w;
                if self.seen[row * GRID_W + col] {
                    seen += w;
                }
            }
        }
        seen / all
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
    pub fn paint(&mut self, pic: &Pic, disc: &Disc, q: &M3, mirrored: bool, every: usize, bare: Option<&Map>) -> usize {
        let qt = transpose(q);
        let n = GRID_W * GRID_H;
        // This picture's own look at each cell: sums and how face-on.
        let mut sum = vec![[0f32; 4]; n];
        let mut square = vec![0f32; n];
        let mut touched: Vec<usize> = Vec::new();
        let (y0, y1) = ((disc.cy - disc.r).floor().max(0.0) as usize, ((disc.cy + disc.r).ceil() as usize).min(pic.h - 1));
        let (x0, x1) = ((disc.cx - disc.r).floor().max(0.0) as usize, ((disc.cx + disc.r).ceil() as usize).min(pic.w - 1));
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            while x <= x1 {
                if let Some(v) = to_sphere(x as f64, y as f64, disc) {
                    let f = facing_of(&v, disc);
                    if f >= PAINT_FACING {
                        let (lat, lon) = lat_lon(&apply(&qt, &v), mirrored);
                        let row = ((lat / PI * GRID_H as f64).floor() as usize).min(GRID_H - 1);
                        let col = ((lon / (2.0 * PI) * GRID_W as f64).floor() as usize).min(GRID_W - 1);
                        let cell = row * GRID_W + col;
                        let (r, g, b) = pic.rgb(x, y);
                        let level = if self.bare {
                            Some(0.0)
                        } else if let Some((br, bg, bb)) = bare.and_then(|m| m.bare_rgb(cell)) {
                            unmix(r, g, b, br, bg, bb)
                        } else {
                            Some(heat(r, g, b))
                        };
                        if let Some(level) = level {
                            let s = &mut sum[cell];
                            if s[3] == 0.0 {
                                touched.push(cell);
                            }
                            s[0] += r as f32;
                            s[1] += g as f32;
                            s[2] += b as f32;
                            s[3] += 1.0;
                            if !self.bare {
                                s[0] = s[0] - r as f32 + level;
                            }
                            square[cell] = square[cell].max(f as f32);
                        }
                    }
                }
                x += every;
            }
            y += every;
        }
        let mut painted = 0;
        for cell in touched {
            if self.best[cell] >= SETTLED || square[cell] <= self.best[cell] + 0.02 {
                continue;
            }
            self.best[cell] = square[cell];
            let s = sum[cell];
            if self.bare {
                self.value[cell * 3] = s[0] / s[3];
                self.value[cell * 3 + 1] = s[1] / s[3];
                self.value[cell * 3 + 2] = s[2] / s[3];
            } else {
                self.value[cell] = s[0] / s[3];
            }
            self.seen[cell] = true;
            painted += 1;
        }
        if painted > 0 {
            self.version += 1;
        }
        painted
    }
    /// EVE's resource fields are smooth: a lone cell far from the cells round it takes their median.
    pub fn despeckle(&mut self, limit: f32) {
        if self.bare {
            return;
        }
        for _ in 0..2 {
            let mut fix = Vec::new();
            for row in 0..GRID_H {
                for col in 0..GRID_W {
                    let i = row * GRID_W + col;
                    if !self.seen[i] {
                        continue;
                    }
                    let mut near = Vec::with_capacity(25);
                    for dr in -2i64..=2 {
                        let r = row as i64 + dr;
                        if r < 0 || r >= GRID_H as i64 {
                            continue;
                        }
                        for dc in -2i64..=2 {
                            let j = r as usize * GRID_W + (col as i64 + dc).rem_euclid(GRID_W as i64) as usize;
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
        (0..GRID_W * GRID_H).map(|i| if self.seen[i] { 1 + (self.value[i].clamp(0.0, 1.0) * 250.0).round() as u8 } else { 0 }).collect()
    }
    /// Bare map bytes: r, g, b a cell (0, 0, 0 = not seen).
    pub fn bare_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; GRID_W * GRID_H * 3];
        for i in 0..GRID_W * GRID_H {
            if self.seen[i] {
                for c in 0..3 {
                    out[i * 3 + c] = self.value[i * 3 + c].round().clamp(if c == 0 { 1.0 } else { 0.0 }, 255.0) as u8;
                }
            }
        }
        out
    }
    pub fn from_bare_bytes(bytes: &[u8]) -> Option<Map> {
        if bytes.len() != GRID_W * GRID_H * 3 {
            return None;
        }
        let mut m = Map::new(true);
        for i in 0..GRID_W * GRID_H {
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
        for row in (0..GRID_H).step_by(3) {
            let lat = (row as f64 + 0.5) / GRID_H as f64 * PI;
            let w = lat.sin();
            for col in (0..GRID_W).step_by(3) {
                if self.seen[row * GRID_W + col] {
                    continue;
                }
                let v = apply(q, &planet_vector(lat, (col as f64 + 0.5) / GRID_W as f64 * 2.0 * PI, mirrored));
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
                        let cell = ((lat / PI * GRID_H as f64) as usize).min(GRID_H - 1) * GRID_W + ((lon / (2.0 * PI) * GRID_W as f64) as usize).min(GRID_W - 1);
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
/// stands out (the panel is closed or moved).
pub fn picked_row(pic: &Pic, spot: &ListSpot) -> Option<usize> {
    let gap = (spot.y5 - spot.y0) / 5.0;
    if gap < 6.0 {
        return None;
    }
    let half_h = (gap * 0.2).max(1.0);
    let mut levels = Vec::with_capacity(6);
    for row in 0..6 {
        let yc = spot.y0 + gap * row as f64;
        let mut vals = Vec::new();
        let mut y = yc - half_h;
        while y <= yc + half_h {
            let mut x = spot.x - gap * 2.4;
            while x <= spot.x + gap * 1.6 {
                if x >= 0.0 && y >= 0.0 && (x as usize) < pic.w && (y as usize) < pic.h {
                    vals.push(pic.lum(x as usize, y as usize));
                }
                x += 2.0;
            }
            y += 1.0;
        }
        if vals.is_empty() {
            return None;
        }
        vals.sort_unstable();
        levels.push(vals[vals.len() / 2] as f64);
    }
    let (best, &top) = levels.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let mut rest: Vec<f64> = levels.iter().enumerate().filter(|(i, _)| *i != best).map(|(_, v)| *v).collect();
    rest.sort_by(f64::total_cmp);
    let second = rest[rest.len() - 1];
    if top - second < 18.0 || top < 45.0 {
        return None;
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn picked_row_finds_the_lit_row() {
        let (w, h) = (400usize, 300usize);
        let mut px = vec![20u8; w * h * 4];
        // Row 3 lit up (spacing 25, No filter at y 60).
        for y in 130..140 {
            for x in 60..300 {
                let k = (y * w + x) * 4;
                px[k] = 110;
                px[k + 1] = 110;
                px[k + 2] = 110;
            }
        }
        let pic = Pic { w, h, bgra: &px };
        assert_eq!(picked_row(&pic, &ListSpot { x: 150.0, y0: 60.0, y5: 185.0 }), Some(3));
        let dark = vec![20u8; w * h * 4];
        assert_eq!(picked_row(&Pic { w, h, bgra: &dark }, &ListSpot { x: 150.0, y0: 60.0, y5: 185.0 }), None);
    }
}
