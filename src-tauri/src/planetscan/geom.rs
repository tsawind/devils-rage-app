//! Small 3 × 3 maths and EVE's planet camera (the same conventions as ui/piscan-engine.js).
//!
//! Camera space: x right, y up, z towards the viewer; the planet's centre is the origin and the
//! camera sits at (0, 0, D), D in planet radii. Planet space follows EVE's building positions:
//! latitude θ 0 … π from the north pole, longitude φ 0 … 2π → (sinθ cosφ, sinθ sinφ, cosθ),
//! optionally mirrored (φ → −φ). Q turns planet space into camera space.

pub type M3 = [f64; 9];
pub type V3 = [f64; 3];

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            o[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
        }
    }
    o
}

#[inline]
pub fn apply(m: &M3, v: &V3) -> V3 {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

pub fn transpose(m: &M3) -> M3 {
    [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]]
}

pub fn norm(v: &V3) -> V3 {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l == 0.0 {
        return *v;
    }
    [v[0] / l, v[1] / l, v[2] / l]
}

pub fn dot(a: &V3, b: &V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Rotation about an axis (Rodrigues), as rotationAbout in the JS engine.
pub fn rotation_about(axis: &V3, rad: f64) -> M3 {
    let [x, y, z] = norm(axis);
    let (c, s) = (rad.cos(), rad.sin());
    let t = 1.0 - c;
    [
        t * x * x + c,
        t * x * y - s * z,
        t * x * z + s * y,
        t * x * y + s * z,
        t * y * y + c,
        t * y * z - s * x,
        t * x * z - s * y,
        t * y * z + s * x,
        t * z * z + c,
    ]
}

/// The planet's axis stood up the screen.
pub fn stand() -> M3 {
    rotation_about(&[1.0, 0.0, 0.0], -std::f64::consts::FRAC_PI_2)
}

/// EVE's camera never rolls: a facing is a spin about the planet's axis (yaw) and how far the
/// axis tips toward you (pitch). Q = Rx(pitch) · B · Rz(yaw).
pub fn pose_q(yaw: f64, pitch: f64) -> M3 {
    mul(&rotation_about(&[1.0, 0.0, 0.0], pitch), &mul(&stand(), &rotation_about(&[0.0, 0.0, 1.0], yaw)))
}

/// (yaw, pitch) of a facing.
pub fn pose_of(q: &M3) -> (f64, f64) {
    let pitch = q[8].atan2(q[5]);
    let m = mul(&transpose(&mul(&rotation_about(&[1.0, 0.0, 0.0], pitch), &stand())), q);
    (m[3].atan2(m[0]), pitch)
}

/// Angle between two facings (radians).
pub fn apart(a: (f64, f64), b: (f64, f64)) -> f64 {
    let m = mul(&pose_q(a.0, a.1), &transpose(&pose_q(b.0, b.1)));
    (((m[0] + m[4] + m[8] - 1.0) / 2.0).clamp(-1.0, 1.0)).acos()
}

pub fn planet_vector(lat: f64, lon: f64, mirrored: bool) -> V3 {
    let phi = if mirrored { -lon } else { lon };
    [lat.sin() * phi.cos(), lat.sin() * phi.sin(), lat.cos()]
}

/// (lat, lon) of a planet-space direction (lon 0 … 2π).
pub fn lat_lon(v: &V3, mirrored: bool) -> (f64, f64) {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    let lat = (v[2] / l).clamp(-1.0, 1.0).acos();
    let mut lon = (if mirrored { -v[1] } else { v[1] }).atan2(v[0]);
    if lon < 0.0 {
        lon += 2.0 * std::f64::consts::PI;
    }
    (lat, lon)
}

/// The planet on the picture: middle, radius (pixels) and the camera's distance D (planet radii).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Disc {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    pub d: f64,
}

impl Disc {
    /// From the edge and the lens (focal length in pixels at this picture's size).
    pub fn with_lens(cx: f64, cy: f64, r: f64, lens: f64) -> Disc {
        Disc { cx, cy, r, d: ((lens / r).powi(2) + 1.0).sqrt() }
    }
    fn f(&self) -> f64 {
        self.r * (self.d * self.d - 1.0).sqrt()
    }
}

/// Picture point → camera-space point on the near side of the planet, or None outside the disc.
#[inline]
pub fn to_sphere(px: f64, py: f64, disc: &Disc) -> Option<V3> {
    let d = disc.d;
    let f = disc.f();
    let u = (px - disc.cx) / f;
    let v = -(py - disc.cy) / f;
    let l = (u * u + v * v + 1.0).sqrt();
    let dir = [u / l, v / l, -1.0 / l];
    let b = d * dir[2];
    let disc2 = b * b - (d * d - 1.0);
    if disc2 < 0.0 {
        return None;
    }
    let t = -b - disc2.sqrt();
    Some([t * dir[0], t * dir[1], d + t * dir[2]])
}

/// Camera-space point → picture (x, y, on the near side).
pub fn to_picture(v: &V3, disc: &Disc) -> (f64, f64, bool) {
    let f = disc.f();
    let depth = disc.d - v[2];
    (disc.cx + f * v[0] / depth, disc.cy - f * v[1] / depth, v[2] > 1.0 / disc.d)
}

/// How square-on the camera sees a surface point (1 = straight at it, 0 = edge-on).
#[inline]
pub fn facing_of(v: &V3, disc: &Disc) -> f64 {
    let to = [-v[0], -v[1], disc.d - v[2]];
    let l = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    (v[0] * to[0] + v[1] * to[1] + v[2] * to[2]) / l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_round_trip() {
        for &(y, p) in &[(0.3, 0.2), (-2.0, 1.1), (3.0, -0.7), (1.0, 2.5)] {
            let (yy, pp) = pose_of(&pose_q(y, p));
            assert!(apart((y, p), (yy, pp)) < 1e-6, "{y} {p} -> {yy} {pp}");
        }
    }

    #[test]
    fn sphere_round_trip() {
        let disc = Disc::with_lens(640.0, 340.0, 200.0, 750.0);
        let v = to_sphere(700.0, 300.0, &disc).unwrap();
        let (x, y, front) = to_picture(&v, &disc);
        assert!((x - 700.0).abs() < 1e-6 && (y - 300.0).abs() < 1e-6 && front);
        assert!(to_sphere(640.0 + 210.0, 340.0, &disc).is_none());
        let (lat, lon) = lat_lon(&planet_vector(1.0, 2.0, true), true);
        assert!((lat - 1.0).abs() < 1e-12 && (lon - 2.0).abs() < 1e-12);
    }
}
