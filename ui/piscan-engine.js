// PI video scan engine (pure maths, no page): used by piscan.html, tested by piscan-engine.test.mjs.
//
// Camera space: x right, y up, z towards the viewer; a picture point inside the planet's disc is
// a point on the unit sphere's near side. Planet space follows EVE's building positions:
// latitude θ 0 … π from the north pole, longitude φ 0 … 2π → (sinθ cosφ, sinθ sinφ, cosθ),
// optionally mirrored (φ → −φ) if EVE turns out to count longitude the other way (the anchors
// decide). Q turns planet space into camera space; rotating the planet in EVE changes Q, which
// the tracker follows frame by frame.
(function (root) {
  "use strict";

  const GRID_W = 360, GRID_H = 180;

  // EVE's scan colours to heat 0..1 (same as SeAT's scan combiner): blue .2 → green .4 → yellow .6 → red .8 → white 1.
  function heat(r, g, b) {
    const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 510;
    const s = max === min ? 0 : (max - min) / (255 - Math.abs(max + min - 255));
    if (l > 0.82 && s < 0.35) return 1;
    if (s < 0.3 || max < 60) return 0;
    const d = max - min;
    let h;
    if (max === r) h = ((g - b) / d) % 6; else if (max === g) h = (b - r) / d + 2; else h = (r - g) / d + 4;
    h = (h * 60 + 360) % 360;
    if (h > 300) h = 0;
    if (h > 240) h = 240;
    return 0.2 + 0.6 * (1 - h / 240) * Math.min(1, max / 200);
  }

  // ---- small 3 × 3 maths (row-major arrays of 9) ----
  const mul = (a, b) => {
    const o = new Array(9);
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) o[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
    return o;
  };
  const apply = (m, v) => [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]];
  const transpose = (m) => [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]];
  const identity = () => [1, 0, 0, 0, 1, 0, 0, 0, 1];
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const norm = (a) => { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };
  const angle = (a, b) => Math.acos(Math.max(-1, Math.min(1, dot(norm(a), norm(b)))));

  function rotationAbout(axis, rad) {
    const [x, y, z] = norm(axis), c = Math.cos(rad), s = Math.sin(rad), t = 1 - c;
    return [t * x * x + c, t * x * y - s * z, t * x * z + s * y, t * x * y + s * z, t * y * y + c, t * y * z - s * x, t * x * z - s * y, t * y * z + s * x, t * z * z + c];
  }

  // Symmetric 4 × 4 eigenvector of the largest eigenvalue (Jacobi sweeps: exact and stable).
  function largestEigenvector(input) {
    const a = input.map((row) => row.slice()), v = [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]];
    for (let sweep = 0; sweep < 50; sweep++) {
      let off = 0;
      for (let p = 0; p < 3; p++) for (let q = p + 1; q < 4; q++) off += a[p][q] * a[p][q];
      if (off < 1e-24) break;
      for (let p = 0; p < 3; p++) for (let q = p + 1; q < 4; q++) {
        if (Math.abs(a[p][q]) < 1e-300) continue;
        const theta = (a[q][q] - a[p][p]) / (2 * a[p][q]);
        const t = Math.sign(theta || 1) / (Math.abs(theta) + Math.sqrt(theta * theta + 1));
        const c = 1 / Math.sqrt(t * t + 1), s = t * c;
        for (let k = 0; k < 4; k++) { const akp = a[k][p], akq = a[k][q]; a[k][p] = c * akp - s * akq; a[k][q] = s * akp + c * akq; }
        for (let k = 0; k < 4; k++) { const apk = a[p][k], aqk = a[q][k]; a[p][k] = c * apk - s * aqk; a[q][k] = s * apk + c * aqk; }
        for (let k = 0; k < 4; k++) { const vkp = v[k][p], vkq = v[k][q]; v[k][p] = c * vkp - s * vkq; v[k][q] = s * vkp + c * vkq; }
      }
    }
    let best = 0;
    for (let i = 1; i < 4; i++) if (a[i][i] > a[best][best]) best = i;
    const q = [v[0][best], v[1][best], v[2][best], v[3][best]], l = Math.hypot(...q) || 1;
    return q.map((x) => x / l);
  }

  // Best rotation R with R·a ≈ b for pairs of vectors (Horn's quaternion method).
  function fitRotation(as, bs, weights) {
    const S = [0, 0, 0, 0, 0, 0, 0, 0, 0];
    for (let k = 0; k < as.length; k++) {
      const w = weights ? weights[k] : 1, a = as[k], b = bs[k];
      for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) S[i * 3 + j] += w * a[i] * b[j];
    }
    const [Sxx, Sxy, Sxz, Syx, Syy, Syz, Szx, Szy, Szz] = S;
    const N = [
      [Sxx + Syy + Szz, Syz - Szy, Szx - Sxz, Sxy - Syx],
      [Syz - Szy, Sxx - Syy - Szz, Sxy + Syx, Szx + Sxz],
      [Szx - Sxz, Sxy + Syx, -Sxx + Syy - Szz, Syz + Szy],
      [Sxy - Syx, Szx + Sxz, Syz + Szy, -Sxx - Syy + Szz],
    ];
    const q = largestEigenvector(N);
    const [w, x, y, z] = q;
    return [
      w * w + x * x - y * y - z * z, 2 * (x * y - w * z), 2 * (x * z + w * y),
      2 * (x * y + w * z), w * w - x * x + y * y - z * z, 2 * (y * z - w * x),
      2 * (x * z - w * y), 2 * (y * z + w * x), w * w - x * x - y * y + z * z,
    ];
  }

  // ---- planet positions ----
  function planetVector(lat, lon, mirrored) {
    const phi = mirrored ? -lon : lon;
    return [Math.sin(lat) * Math.cos(phi), Math.sin(lat) * Math.sin(phi), Math.cos(lat)];
  }
  function latLon(v, mirrored) {
    const [x, y, z] = norm(v);
    const lat = Math.acos(Math.max(-1, Math.min(1, z)));
    let lon = Math.atan2(mirrored ? -y : y, x);
    if (lon < 0) lon += 2 * Math.PI;
    return { lat, lon };
  }

  // EVE's planet camera is close: disc.D is its distance from the planet's centre in planet
  // radii (missing or Infinity = seen from far away). The rim on screen is where sight lines
  // just touch the planet.
  const far = (disc) => !disc.D || !isFinite(disc.D);
  // Picture point (pixels) → camera-space point on the near side, or null outside the disc.
  function toSphere(px, py, disc) {
    if (far(disc)) {
      const x = (px - disc.cx) / disc.r, y = -(py - disc.cy) / disc.r, d = x * x + y * y;
      if (d >= 1) return null;
      return [x, y, Math.sqrt(1 - d)];
    }
    const D = disc.D, f = disc.r * Math.sqrt(D * D - 1);
    const u = (px - disc.cx) / f, v = -(py - disc.cy) / f, l = Math.hypot(u, v, 1);
    const dir = [u / l, v / l, -1 / l];
    const b = D * dir[2], disc2 = b * b - (D * D - 1);
    if (disc2 < 0) return null;
    const t = -b - Math.sqrt(disc2);
    return [t * dir[0], t * dir[1], D + t * dir[2]];
  }
  function toPicture(v, disc) {
    if (far(disc)) return { x: disc.cx + v[0] * disc.r, y: disc.cy - v[1] * disc.r, front: v[2] > 0 };
    const D = disc.D, f = disc.r * Math.sqrt(D * D - 1), depth = D - v[2];
    return { x: disc.cx + f * v[0] / depth, y: disc.cy - f * v[1] / depth, front: v[2] > 1 / D };
  }
  /** How square-on the camera sees a surface point (1 = straight at it, 0 = edge-on). */
  function facingOf(v, disc) {
    if (far(disc)) return v[2];
    const to = [-v[0], -v[1], disc.D - v[2]], l = Math.hypot(...to);
    return (v[0] * to[0] + v[1] * to[1] + v[2] * to[2]) / l;
  }

  /**
   * Which way the planet faces, from buildings pointed out on the picture:
   * anchors = [{ px, py, lat, lon }]. Two or more; with three or more spread out, also tells
   * whether EVE counts longitude the other way round (mirrored).
   * Returns { Q, mirrored, error (radians, average miss) } or null.
   */
  function solveAnchors(anchors, disc, mirrored) {
    const cams = [], planets = [];
    for (const a of anchors) {
      const v = toSphere(a.px, a.py, disc);
      if (!v) continue;
      cams.push(v);
      planets.push(planetVector(a.lat, a.lon, mirrored));
    }
    if (cams.length < 2) return null;
    // With two points, add their cross product so the fit is fully pinned down.
    const as = planets.slice(), bs = cams.slice();
    if (as.length === 2) { as.push(norm(cross(as[0], as[1]))); bs.push(norm(cross(bs[0], bs[1]))); }
    const Q = fitRotation(as, bs);
    let error = 0;
    planets.forEach((p, i) => { error += angle(apply(Q, p), cams[i]); });
    return { Q, mirrored: !!mirrored, error: error / planets.length };
  }
  function solveAnchorsEitherWay(anchors, disc) {
    const a = solveAnchors(anchors, disc, false), b = solveAnchors(anchors, disc, true);
    if (!a) return b;
    if (!b) return a;
    return b.error < a.error * 0.7 ? b : a;
  }

  /** Nudge Q so one building lands where it was clicked (drift fix), keeping the rest. */
  function correctWithAnchor(Q, anchor, disc, mirrored) {
    const cam = toSphere(anchor.px, anchor.py, disc);
    if (!cam) return Q;
    const now = apply(Q, planetVector(anchor.lat, anchor.lon, mirrored));
    const axis = cross(now, cam), s = Math.hypot(...axis);
    if (s < 1e-9) return Q;
    return mul(rotationAbout(axis, Math.atan2(s, dot(now, cam))), Q);
  }

  /**
   * Lock from ONE building. EVE's planet camera never rolls: the planet's axis always stands
   * upright on screen (checked on a recording: within 0.6° over a 150° turn). So one building
   * where it was clicked plus "north straight up" (or down) gives the whole facing.
   */
  function lockOne(anchor, disc, mirrored, up) {
    const v = toSphere(anchor.px, anchor.py, disc);
    if (!v) return null;
    const a = planetVector(anchor.lat, anchor.lon, mirrored), n = [0, 0, 1];
    // The pole on screen: p = (0, y, z), unit, at the same angle from v as the pole is from a.
    const c = dot(a, n), r = Math.hypot(v[1], v[2]);
    if (r < 1e-9 || Math.abs(c) > r) return null;
    const phi = Math.atan2(v[2], v[1]), delta = Math.acos(c / r);
    const options = [phi + delta, phi - delta].map((t) => [0, Math.cos(t), Math.sin(t)]);
    // Upright: the pole's picture points up (or down) from the planet's middle.
    const p = options.sort((m, k) => (up ? k[1] - m[1] : m[1] - k[1]))[0];
    const frame = (x, y) => { const e2 = norm([y[0] - dot(y, x) * x[0], y[1] - dot(y, x) * x[1], y[2] - dot(y, x) * x[2]]); return [x, e2, cross(x, e2)]; };
    const E1 = frame(a, n), F = frame(v, p);
    const R = new Array(9).fill(0);
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) for (let k = 0; k < 3; k++) R[i * 3 + j] += F[k][i] * E1[k][j];
    return R;
  }
  /** Which way up the pole stands for a facing (true = north up on screen). */
  function northUp(Q) { return apply(Q, [0, 0, 1])[1] >= 0; }

  // ---- EVE's camera: two numbers ----
  // The camera never rolls, so where the planet faces is just a spin about its own axis (yaw)
  // and how far that axis tips toward you (pitch; 0 = axis straight up the screen, ±90° = a
  // pole facing you, beyond = upside down). Q(yaw, pitch) = Rx(pitch) · B · Rz(yaw), where B
  // stands the planet's axis up the screen.
  const STAND = rotationAbout([1, 0, 0], -Math.PI / 2);
  function poseQ(yaw, pitch) { return mul(rotationAbout([1, 0, 0], pitch), mul(STAND, rotationAbout([0, 0, 1], yaw))); }
  function poseOf(Q) {
    const pitch = Math.atan2(Q[8], Q[5]);
    const M = mul(transpose(mul(rotationAbout([1, 0, 0], pitch), STAND)), Q);
    return { yaw: Math.atan2(M[3], M[0]), pitch };
  }
  /** The nearest facing EVE's camera can actually have (no roll). */
  function upright(Q) { const p = poseOf(Q); return poseQ(p.yaw, p.pitch); }

  // ---- finding the facing on the planet map ----
  // The planet doesn't change: once its bare colours are recorded, any picture of it can be
  // placed by trying facings and keeping the one whose map colours match the picture best.
  // No piling-up error, and a lost grip is found again by searching the whole planet.
  /** Points to compare: camera-space directions with their picture brightness. */
  function samplePoints(rgba, width, height, disc, count, coarse) {
    const out = [], area = Math.PI * disc.r * disc.r, gap = Math.max(2, Math.sqrt(area / (count || 700)));
    for (let y = disc.cy - disc.r; y < disc.cy + disc.r; y += gap) for (let x = disc.cx - disc.r; x < disc.cx + disc.r; x += gap) {
      const v = toSphere(x, y, disc);
      if (!v || facingOf(v, disc) < 0.4) continue;
      const xi = Math.round(x), yi = Math.round(y);
      if (xi < 1 || yi < 1 || xi >= width - 1 || yi >= height - 1) continue;
      const lum = (px, py) => { const k = (Math.min(height - 1, Math.max(0, py)) * width + Math.min(width - 1, Math.max(0, px))) * 4; return 0.3 * rgba[k] + 0.59 * rgba[k + 1] + 0.11 * rgba[k + 2]; };
      // Coarse (for searching the whole planet): broader spots and surroundings, a wider match.
      const inner = coarse ? Math.max(2, Math.round(disc.r * 0.03)) : 1, istep = Math.max(1, inner >> 1);
      let g = 0, gn = 0;
      for (let dy = -inner; dy <= inner; dy += istep) for (let dx = -inner; dx <= inner; dx += istep) { g += lum(xi + dx, yi + dy); gn++; }
      g /= gn;
      // Fine detail only (the spot minus its surroundings, ~2.5° across): EVE's scan colours are
      // smooth, the rock and lava pattern isn't, so this matches with or without a filter on.
      const ring = Math.max(4, Math.round(disc.r * (coarse ? 0.13 : 0.045)));
      let around = 0, n = 0;
      for (let dy = -ring; dy <= ring; dy += Math.max(1, ring >> 2)) for (let dx = -ring; dx <= ring; dx += Math.max(1, ring >> 2)) { around += lum(xi + dx, yi + dy); n++; }
      out.push({ v, g: g - around / n });
    }
    return out;
  }
  /** The bare map as brightness per cell (NaN where not recorded). */
  function bareGrey(bare, coarse) {
    const lum = new Float32Array(GRID_W * GRID_H), out = new Float32Array(GRID_W * GRID_H);
    for (let i = 0; i < GRID_W * GRID_H; i++) {
      const w = bare.weight[i];
      lum[i] = w > 0 ? (0.3 * bare.rgb[i * 3] + 0.59 * bare.rgb[i * 3 + 1] + 0.11 * bare.rgb[i * 3 + 2]) / w : NaN;
    }
    // Same fine detail as the picture: each cell minus the cells round it (±2, wrapping east-west).
    for (let row = 0; row < GRID_H; row++) for (let col = 0; col < GRID_W; col++) {
      const i = row * GRID_W + col;
      if (lum[i] !== lum[i]) { out[i] = NaN; continue; }
      let s = 0, n = 0;
      for (let dr = -2; dr <= 2; dr++) { const r = row + dr; if (r < 0 || r >= GRID_H) continue; for (let dc = -2; dc <= 2; dc++) { const v = lum[r * GRID_W + (col + dc + GRID_W) % GRID_W]; if (v === v) { s += v; n++; } } }
      out[i] = lum[i] - s / n;
    }
    if (!coarse) return out;
    // Coarse: each cell's ±1 average minus its ±6 surroundings.
    const box = (src, rad, row, col) => { let s2 = 0, n2 = 0; for (let dr = -rad; dr <= rad; dr++) { const r = row + dr; if (r < 0 || r >= GRID_H) continue; for (let dc = -rad; dc <= rad; dc++) { const v = src[r * GRID_W + (col + dc + GRID_W) % GRID_W]; if (v === v) { s2 += v; n2++; } } } return n2 ? s2 / n2 : NaN; };
    const wide = new Float32Array(GRID_W * GRID_H);
    for (let row = 0; row < GRID_H; row++) for (let col = 0; col < GRID_W; col++) {
      const i = row * GRID_W + col;
      wide[i] = lum[i] !== lum[i] ? NaN : box(lum, 1, row, col) - box(lum, 6, row, col);
    }
    return wide;
  }
  /** How well a facing explains the picture: correlation of brightness, -1 … 1 (null: too little of the map seen). */
  function poseScore(points, grey, Q, mirrored) {
    const Qt = transpose(Q);
    let n = 0, sa = 0, sb = 0, saa = 0, sbb = 0, sab = 0;
    for (const p of points) {
      const { lat, lon } = latLon(apply(Qt, p.v), mirrored);
      const row = Math.min(GRID_H - 1, Math.floor(lat / Math.PI * GRID_H)), col = Math.min(GRID_W - 1, Math.floor(lon / (2 * Math.PI) * GRID_W));
      const m = grey[row * GRID_W + col];
      if (m !== m) continue;
      n++; sa += p.g; sb += m; saa += p.g * p.g; sbb += m * m; sab += p.g * m;
    }
    if (n < points.length * 0.5 || n < 40) return null;
    const va = saa - sa * sa / n, vb = sbb - sb * sb / n;
    return va > 0 && vb > 0 ? (sab - sa * sb / n) / Math.sqrt(va * vb) : null;
  }
  /** Best facing near a guess: a grid of yaw/pitch tries, then finer around the best. */
  function alignToMap(points, grey, mirrored, around, span, step) {
    let best = null;
    const tryAt = (yaw, pitch) => { const s = poseScore(points, grey, poseQ(yaw, pitch), mirrored); if (s !== null && (!best || s > best.score)) best = { yaw, pitch, score: s }; };
    for (let dy = -span; dy <= span + 1e-9; dy += step) for (let dp = -span; dp <= span + 1e-9; dp += step) tryAt(around.yaw + dy, around.pitch + dp);
    if (!best) return null;
    const fine = step / 4, c = { ...best };
    for (let dy = -step; dy <= step + 1e-9; dy += fine) for (let dp = -step; dp <= step + 1e-9; dp += fine) tryAt(c.yaw + dy, c.pitch + dp);
    return best;
  }
  /** Lost: search the whole planet (coarse), then refine. Returns the best and how clearly it beat the rest. */
  function findOnMap(points, grey, mirrored) {
    const step = 4 * Math.PI / 180, found = [];
    for (let yaw = -Math.PI; yaw < Math.PI; yaw += step) for (let pitch = -Math.PI; pitch < Math.PI; pitch += step) {
      const s = poseScore(points, grey, poseQ(yaw, pitch), mirrored);
      if (s !== null) found.push({ yaw, pitch, score: s });
    }
    if (!found.length) return null;
    found.sort((a, b) => b.score - a.score);
    const top = found[0];
    // The runner-up that isn't just next door to the best: how sure the find is.
    const other = found.find((f) => angle(apply(poseQ(f.yaw, f.pitch), [0, 0, 1]), apply(poseQ(top.yaw, top.pitch), [0, 0, 1])) > 0.25);
    const best = alignToMap(points, grey, mirrored, top, step, step / 2) || top;
    return { ...best, margin: best.score - (other ? other.score : 0) };
  }

  // ---- tracking ----
  // A frame for tracking: the disc cropped and scaled to SIZE × SIZE grey values.
  const SIZE = 256;

  // Each cell is the average of the screen pixels it covers, then softened once more: EVE draws
  // fine diagonal stripes over the scan colours that stay put on screen while the planet turns,
  // and a tracker that sees them follows the stripes instead of the planet.
  function greyDisc(rgba, width, height, disc) {
    const raw = new Float32Array(SIZE * SIZE);
    const scale = (2 * disc.r) / SIZE, x0 = disc.cx - disc.r, y0 = disc.cy - disc.r;
    const span = Math.max(1, Math.round(scale));
    for (let j = 0; j < SIZE; j++) {
      const yStart = Math.floor(y0 + j * scale);
      for (let i = 0; i < SIZE; i++) {
        const xStart = Math.floor(x0 + i * scale);
        let sum = 0, n = 0;
        for (let dy = 0; dy < span; dy++) {
          const y = Math.min(height - 1, Math.max(0, yStart + dy));
          for (let dx = 0; dx < span; dx++) {
            const x = Math.min(width - 1, Math.max(0, xStart + dx));
            const k = (y * width + x) * 4;
            // Brightness plus colour: EVE's scan colours (blue → green → yellow → red) give plain planets something to grip.
            sum += 0.3 * rgba[k] + 0.59 * rgba[k + 1] + 0.11 * rgba[k + 2] + 0.5 * (rgba[k] - rgba[k + 2]) + 0.25 * (rgba[k + 1] - (rgba[k] + rgba[k + 2]) / 2);
            n++;
          }
        }
        raw[j * SIZE + i] = sum / n;
      }
    }
    const out = new Float32Array(SIZE * SIZE);
    for (let j = 0; j < SIZE; j++) for (let i = 0; i < SIZE; i++) {
      let sum = 0, n = 0;
      for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
        const y = j + dy, x = i + dx;
        if (y < 0 || x < 0 || y >= SIZE || x >= SIZE) continue;
        sum += raw[y * SIZE + x]; n++;
      }
      out[j * SIZE + i] = sum / n;
    }
    return out;
  }
  function half(img, size) {
    const h = size / 2, out = new Float32Array(h * h);
    for (let j = 0; j < h; j++) for (let i = 0; i < h; i++) {
      const k = 2 * j * size + 2 * i;
      out[j * h + i] = (img[k] + img[k + 1] + img[k + size] + img[k + size + 1]) / 4;
    }
    return out;
  }
  function sad(a, b, size, ax, ay, bx, by, p, cap) {
    let s = 0;
    for (let j = -p; j <= p; j++) {
      const ra = (ay + j) * size, rb = (by + j) * size;
      for (let i = -p; i <= p; i++) { s += Math.abs(a[ra + ax + i] - b[rb + bx + i]); }
      if (s > cap) return s;
    }
    return s;
  }
  function textured(img, size, x, y, p) {
    let sum = 0, sq = 0, n = 0;
    for (let j = -p; j <= p; j++) for (let i = -p; i <= p; i++) { const v = img[(y + j) * size + x + i]; sum += v; sq += v * v; n++; }
    return sq / n - (sum / n) ** 2;
  }
  function search(a, b, size, x, y, gx, gy, range, p) {
    let best = Infinity, bx = gx, by = gy;
    for (let dy = -range; dy <= range; dy++) for (let dx = -range; dx <= range; dx++) {
      const tx = gx + dx, ty = gy + dy;
      if (tx - p < 0 || ty - p < 0 || tx + p >= size || ty + p >= size) continue;
      const s = sad(a, b, size, x, y, tx, ty, p, best);
      if (s < best) { best = s; bx = tx; by = ty; }
    }
    return { x: bx, y: by, cost: best };
  }

  /**
   * How the planet turned between two grey discs: R with cur ≈ R·prev (camera space), from
   * matched patches (coarse at half size, then refined). Returns { R, used, spread (radians), moved (radians) }
   * or null when too few patches agree (moved too fast, or nothing to grip on).
   */
  function trackRotation(prev, cur, D, opts) {
    const o = opts || {}, P = o.P || 4, step = o.step || 18, inner = o.inner || 0.72, CP = o.CP || 2;
    const crop = { cx: SIZE / 2, cy: SIZE / 2, r: SIZE / 2, D };
    const unit = (x, y) => toSphere(x + 0.5, y + 0.5, crop);
    const prevHalf = half(prev, SIZE), curHalf = half(cur, SIZE);
    const as = [], bs = [];
    for (let y = step; y < SIZE - step; y += step) for (let x = step; x < SIZE - step; x += step) {
      const a = unit(x, y);
      if (!a || Math.hypot((x - SIZE / 2) / (SIZE / 2), (y - SIZE / 2) / (SIZE / 2)) > inner) continue;
      if (textured(prev, SIZE, x, y, P) < 12) continue;
      const hx = x >> 1, hy = y >> 1;
      if (hx - 3 < 0 || hy - 3 < 0) continue;
      // Turning steadily: look first where the last step's turn would put this patch.
      let gx = hx, gy = hy;
      if (o.guess) {
        const g = toPicture(apply(o.guess, a), crop);
        if (!g.front) continue;
        gx = Math.round(g.x - 0.5) >> 1; gy = Math.round(g.y - 0.5) >> 1;
      }
      const coarse = search(prevHalf, curHalf, SIZE / 2, hx, hy, gx, gy, o.range || 14, CP);
      const fine = search(prev, cur, SIZE, x, y, coarse.x * 2, coarse.y * 2, 3, P);
      const b = unit(fine.x, fine.y);
      if (!b) continue;
      as.push(a); bs.push(b);
    }
    if (as.length < 8) return null;
    let keep = as.map(() => true), R = identity();
    for (let round = 0; round < 4; round++) {
      const A = as.filter((_, i) => keep[i]), B = bs.filter((_, i) => keep[i]);
      if (A.length < 8) return null;
      R = fitRotation(A, B);
      const miss = as.map((a, i) => angle(apply(R, a), bs[i]));
      const sorted = miss.filter((_, i) => keep[i]).sort((m, n) => m - n);
      const median = sorted[Math.floor(sorted.length / 2)];
      const limit = Math.max(0.012, median * 3);
      keep = miss.map((m) => m <= limit);
    }
    const used = keep.filter(Boolean).length;
    if (used < Math.max(8, as.length * 0.4)) return null;
    const miss = as.map((a, i) => angle(apply(R, a), bs[i])).filter((_, i) => keep[i]).sort((m, n) => m - n);
    // Patches that agree only loosely mean the matches were guesses (turned past the search): lost.
    if (miss[Math.floor(miss.length / 2)] > (o.maxSpread || 0.012)) return null;
    const moved = Math.acos(Math.max(-1, Math.min(1, (R[0] + R[4] + R[8] - 1) / 2)));
    return { R, used, of: as.length, spread: miss[Math.floor(miss.length / 2)], moved, ...(o.debug ? { as, bs, keep } : {}) };
  }

  // ---- painting ----
  function newMap() { return { sum: new Float32Array(GRID_W * GRID_H), weight: new Float32Array(GRID_W * GRID_H), best: new Float32Array(GRID_W * GRID_H) }; }

  /** Paint one picture onto a resource's map: every disc pixel facing the camera well enough. */
  // EVE's scan colour band (sampled from the Scan panel), low → high, and how it's laid over
  // the planet: what you see = (1 − OVERLAY) × planet + OVERLAY × BOOST × band colour.
  const BAND = [[20, 14, 25], [21, 21, 40], [19, 36, 55], [18, 57, 49], [18, 75, 33], [37, 99, 7], [74, 112, 7], [118, 126, 6], [145, 138, 5], [161, 109, 12], [169, 58, 4], [179, 5, 3], [187, 43, 42], [195, 191, 190]];
  const OVERLAY = 0.35, BOOST = 1.6, STEPS = 60;
  const BAND_FINE = (() => {
    const out = [];
    for (let i = 0; i < STEPS; i++) {
      const t = i / (STEPS - 1) * (BAND.length - 1), a = Math.floor(t), b = Math.min(BAND.length - 1, a + 1), f = t - a;
      out.push([0, 1, 2].map((c) => OVERLAY * BOOST * (BAND[a][c] * (1 - f) + BAND[b][c] * f)));
    }
    return out;
  })();
  /**
   * Resource level 0 … 1 at a pixel, knowing the bare planet's colour there: the band colour
   * that, laid over the planet, gives what's on screen. null where the planet is too bright to
   * see through (glowing lava).
   */
  function unmix(r, g, b, br, bg, bb) {
    if (Math.max(br, bg, bb) > 215) return null;
    const k = 1 - OVERLAY;
    let best = Infinity, at = 0;
    for (let i = 0; i < STEPS; i++) {
      const c = BAND_FINE[i];
      const e = (k * br + c[0] - r) ** 2 + (k * bg + c[1] - g) ** 2 + (k * bb + c[2] - b) ** 2;
      if (e < best) { best = e; at = i; }
    }
    return best > 4000 ? null : at / (STEPS - 1);
  }

  /** The bare planet's colours (EVE on "No filter"), one cell per degree, for unmixing. */
  function newBare() { return { rgb: new Float32Array(GRID_W * GRID_H * 3), weight: new Float32Array(GRID_W * GRID_H), best: new Float32Array(GRID_W * GRID_H) }; }
  function bareCoverage(bare) { return coverage({ weight: bare.weight }); }
  function bareToBytes(bare) {
    const out = new Uint8Array(GRID_W * GRID_H * 3);
    for (let i = 0; i < GRID_W * GRID_H; i++) if (bare.weight[i] > 0) for (let c = 0; c < 3; c++) out[i * 3 + c] = Math.round(bare.rgb[i * 3 + c] / bare.weight[i]);
    return out;
  }
  function bareFromBytes(bytes) {
    const bare = newBare();
    for (let i = 0; i < GRID_W * GRID_H; i++) {
      if (!bytes[i * 3] && !bytes[i * 3 + 1] && !bytes[i * 3 + 2]) continue;
      bare.weight[i] = 1; bare.best[i] = 1; // a saved map is settled; only "Record bare map again" redoes it
      for (let c = 0; c < 3; c++) bare.rgb[i * 3 + c] = bytes[i * 3 + c];
    }
    return bare;
  }

  /**
   * Paint one picture onto a map: every disc pixel facing the camera well enough. With `bare`
   * (a bare-planet map) it's a resource map, unmixed against the planet's own colours; with
   * `bare === "record"`, `map` is a bare map being recorded.
   */
  function paint(map, rgba, width, height, disc, Q, mirrored, every, bare) {
    const Qt = transpose(Q), step = every || 2;
    const recording = bare === "record";
    if (!map.best) map.best = new Float32Array(map.weight.length);
    // This picture's own look at each cell (its colour or level, and how face-on it was).
    const look = new Map();
    const y0 = Math.max(0, Math.floor(disc.cy - disc.r)), y1 = Math.min(height - 1, Math.ceil(disc.cy + disc.r));
    const x0 = Math.max(0, Math.floor(disc.cx - disc.r)), x1 = Math.min(width - 1, Math.ceil(disc.cx + disc.r));
    for (let y = y0; y <= y1; y += step) for (let x = x0; x <= x1; x += step) {
      const v = toSphere(x, y, disc);
      if (!v) continue;
      const square = facingOf(v, disc);
      if (square < 0.35) continue; // near the edge the picture is too squashed to trust
      const { lat, lon } = latLon(apply(Qt, v), mirrored);
      const row = Math.min(GRID_H - 1, Math.floor(lat / Math.PI * GRID_H)), col = Math.min(GRID_W - 1, Math.floor(lon / (2 * Math.PI) * GRID_W));
      const cell = row * GRID_W + col, k = (y * width + x) * 4;
      let level = 0;
      if (!recording) {
        if (bare && bare.weight[cell] > 0) {
          const bw = bare.weight[cell];
          level = unmix(rgba[k], rgba[k + 1], rgba[k + 2], bare.rgb[cell * 3] / bw, bare.rgb[cell * 3 + 1] / bw, bare.rgb[cell * 3 + 2] / bw);
          if (level === null) continue;
        } else {
          level = heat(rgba[k], rgba[k + 1], rgba[k + 2]);
        }
      }
      let l = look.get(cell);
      if (!l) look.set(cell, l = { r: 0, g: 0, b: 0, level: 0, n: 0, square: 0 });
      l.r += rgba[k]; l.g += rgba[k + 1]; l.b += rgba[k + 2]; l.level += level; l.n++;
      l.square = Math.max(l.square, square);
    }
    // Each cell keeps the most face-on look it has had, and nothing else: once a spot has been
    // seen near the middle of the disc it's settled, and more turning (or another character,
    // other light, moving lava) leaves it alone. Only a clearly better look replaces it.
    let painted = 0;
    for (const [cell, l] of look) {
      if (l.square <= map.best[cell] + 0.02) continue;
      map.best[cell] = l.square;
      if (recording) {
        map.rgb[cell * 3] = l.r / l.n; map.rgb[cell * 3 + 1] = l.g / l.n; map.rgb[cell * 3 + 2] = l.b / l.n;
      } else {
        map.sum[cell] = l.level / l.n;
      }
      map.weight[cell] = 1;
      painted++;
    }
    return painted;
  }

  /** Fill small holes (glowing lava, a missed patch) from the cells round them. */
  function fillHoles(map, rounds) {
    for (let round = 0; round < (rounds || 3); round++) {
      const add = [];
      for (let row = 0; row < GRID_H; row++) for (let col = 0; col < GRID_W; col++) {
        const i = row * GRID_W + col;
        if (map.weight[i] > 0) continue;
        let s = 0, n = 0;
        for (const [dr, dc] of [[-1, 0], [1, 0], [0, -1], [0, 1]]) {
          const r = row + dr;
          if (r < 0 || r >= GRID_H) continue;
          const j = r * GRID_W + ((col + dc + GRID_W) % GRID_W);
          if (map.weight[j] > 0) { s += map.sum[j] / map.weight[j]; n++; }
        }
        if (n >= 3) add.push([i, s / n]);
      }
      for (const [i, v] of add) { map.sum[i] = v * 0.1; map.weight[i] = 0.1; }
    }
  }

  /** The map as SeAT's bytes: 0 = not seen, 1 … 251 = heat 0 … 1. */
  function cells(map) {
    const out = new Uint8Array(GRID_W * GRID_H);
    for (let i = 0; i < out.length; i++) if (map.weight[i] > 0) out[i] = 1 + Math.round(Math.max(0, Math.min(1, map.sum[i] / map.weight[i])) * 250);
    return out;
  }
  /** Start a map from bytes SeAT already has (they count as one light look). */
  function fromCells(bytes) {
    const map = newMap();
    for (let i = 0; i < bytes.length && i < map.sum.length; i++) if (bytes[i]) { map.weight[i] = 0.25; map.sum[i] = ((bytes[i] - 1) / 250) * 0.25; }
    return map;
  }
  function coverage(map) {
    let seen = 0, all = 0;
    for (let row = 0; row < GRID_H; row++) {
      const w = Math.sin((row + 0.5) / GRID_H * Math.PI);
      for (let col = 0; col < GRID_W; col++) { all += w; if (map.weight[row * GRID_W + col] > 0) seen += w; }
    }
    return seen / all;
  }

  /** Where the biggest unseen part is, in words relative to the picture now ("past the top-left edge", "behind"). */
  function missingHint(map, Q, mirrored) {
    let sx = 0, sy = 0, sz = 0, n = 0;
    for (let row = 0; row < GRID_H; row += 3) {
      const lat = (row + 0.5) / GRID_H * Math.PI, w = Math.sin(lat);
      for (let col = 0; col < GRID_W; col += 3) {
        if (map.weight[row * GRID_W + col] > 0) continue;
        const v = apply(Q, planetVector(lat, (col + 0.5) / GRID_W * 2 * Math.PI, mirrored));
        sx += v[0] * w; sy += v[1] * w; sz += v[2] * w; n += w;
      }
    }
    if (!n) return null;
    const v = norm([sx, sy, sz]);
    if (v[2] > 0.6) return "in front of you: hold still a moment over it";
    const parts = [];
    if (v[1] > 0.3) parts.push("top"); else if (v[1] < -0.3) parts.push("bottom");
    if (v[0] > 0.3) parts.push("right"); else if (v[0] < -0.3) parts.push("left");
    if (!parts.length) return "on the far side: turn the planet right round";
    return `past the ${parts.join("-")} edge: turn the planet to bring that side round to the front`;
  }


  // ---- the planet's edge ----
  /** Circle through edge points (Kåsa fit), then again with the points that agree. */
  function fitCircle(points) {
    const fit = (list) => {
      let sx = 0, sy = 0, sxx = 0, syy = 0, sxy = 0, sxz = 0, syz = 0, sz = 0;
      const n = list.length;
      list.forEach(([x, y]) => { const z = x * x + y * y; sx += x; sy += y; sxx += x * x; syy += y * y; sxy += x * y; sxz += x * z; syz += y * z; sz += z; });
      const A = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]], B = [sxz, syz, sz];
      const det = (m) => m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
      const D = det(A);
      if (!D) return null;
      const col = (i) => A.map((row, r) => row.map((v, c) => (c === i ? B[r] : v)));
      const a = det(col(0)) / D, b = det(col(1)) / D, c = det(col(2)) / D;
      return { cx: a / 2, cy: b / 2, r: Math.sqrt(Math.max(1, c + a * a / 4 + b * b / 4)) };
    };
    if (points.length < 3) return null;
    let circle = fit(points);
    for (let round = 0; circle && round < 2; round++) {
      const near = points.filter(([x, y]) => Math.abs(Math.hypot(x - circle.cx, y - circle.cy) - circle.r) < circle.r * 0.04);
      if (near.length < Math.max(3, points.length * 0.5)) break;
      circle = fit(near) || circle;
    }
    return circle;
  }

  /**
   * The planet's rim: rays out from a guess, each stops where the picture turns to space (dark
   * for a few pixels). With `around` (the circle last frame), only looks near that radius, so a
   * zoom in EVE is followed. Rays that leave the picture (zoomed right in) are skipped.
   */
  function findRim(rgba, width, height, around) {
    const lum = (x, y) => { const k = ((y | 0) * width + (x | 0)) * 4; return 0.3 * rgba[k] + 0.59 * rgba[k + 1] + 0.11 * rgba[k + 2]; };
    const cx = around ? around.cx : width / 2, cy = around ? around.cy : height / 2;
    const from = around ? around.r * 0.75 : 20, to = around ? around.r * 1.35 : Math.hypot(width, height) / 2;
    // How dark "space" is here (nebulae make it brownish, not black): between the darkest picture
    // corners and the planet's middle.
    const sample = (x, y) => lum(Math.max(0, Math.min(width - 1, x)), Math.max(0, Math.min(height - 1, y)));
    const corners = [];
    for (let i = 0; i < 40; i++) { corners.push(sample(i * 5, i * 3), sample(width - 1 - i * 5, i * 3), sample(i * 5, height - 1 - i * 3), sample(width - 1 - i * 5, height - 1 - i * 3)); }
    corners.sort((a, b) => a - b);
    const spaceLevel = corners[Math.floor(corners.length * 0.5)];
    let middle = 0, count = 0;
    for (let dy = -30; dy <= 30; dy += 6) for (let dx = -30; dx <= 30; dx += 6) { middle += sample(cx + dx, cy + dy); count++; }
    middle /= count;
    const darkBelow = Math.max(18, spaceLevel + 0.22 * Math.max(0, middle - spaceLevel));
    // How dark space is here: the darkest tenth of a ring well outside the guess.
    const points = [];
    for (let i = 0; i < 72; i++) {
      const a = (i / 72) * Math.PI * 2, dx = Math.cos(a), dy = Math.sin(a);
      let dark = 0, edge = null, inside = false;
      for (let r = from; r < to; r += 2) {
        const x = cx + dx * r, y = cy + dy * r;
        if (x < 1 || y < 1 || x >= width - 1 || y >= height - 1) { edge = null; break; }
        if (lum(x, y) < darkBelow) { dark++; if (dark === 1) edge = r; if (dark >= 5 && inside) break; } else { dark = 0; edge = null; inside = true; }
      }
      if (edge && dark >= 5 && inside) points.push([cx + dx * edge, cy + dy * edge]);
    }
    if (points.length < 10) return null;
    const circle = fitCircle(points);
    if (!circle || circle.r < 40) return null;
    if (around && (Math.abs(circle.r - around.r) > around.r * 0.3 || Math.hypot(circle.cx - around.cx, circle.cy - around.cy) > around.r * 0.15)) return null;
    return circle;
  }

  /**
   * EVE always puts the planet in the middle of the client window, so only the radius is
   * unknown: along rays from the middle, where the picture drops from planet to space and
   * stays dark. Median over the rays (a lava glow or a window on top spoils only a few).
   * With `near`, only looks within ±20 % of that radius (following a zoom).
   */
  function rimRadius(rgba, width, height, near) {
    const cx = width / 2, cy = height / 2;
    const lum = (x, y) => { const k = ((y | 0) * width + (x | 0)) * 4; return 0.3 * rgba[k] + 0.59 * rgba[k + 1] + 0.11 * rgba[k + 2]; };
    const maxR = Math.hypot(width, height) / 2;
    const from = near ? near * 0.8 : 30, to = near ? near * 1.2 : maxR;
    const found = [];
    for (let i = 0; i < 120; i++) {
      const a = (i / 120) * Math.PI * 2, dx = Math.cos(a), dy = Math.sin(a);
      const prof = [];
      for (let r = from - 8; r < to + 12; r += 2) {
        const x = cx + dx * r, y = cy + dy * r;
        if (x < 0 || y < 0 || x >= width || y >= height) break;
        prof.push([r, lum(x, y)]);
      }
      if (prof.length < 12) continue;
      // The outermost place where it drops and then stays as dark as space: dark lava rock and
      // shadows inside the planet drop too, but brighten again further out.
      const tail = prof.slice(-6).reduce((m, p) => m + p[1], 0) / 6;
      let at = null;
      for (let k = prof.length - 12; k >= 4; k--) {
        if (prof[k][0] < from || prof[k][0] > to) continue;
        const inside = (prof[k - 4][1] + prof[k - 3][1] + prof[k - 2][1]) / 3;
        let outside = 0;
        for (let j = k + 2; j < Math.min(prof.length, k + 12); j++) outside = Math.max(outside, prof[j][1]);
        if (inside - outside > 12 && outside < tail + 25) { at = prof[k][0]; break; }
      }
      if (at !== null) found.push(at);
    }
    if (found.length < 15) return null;
    found.sort((a, b) => a - b);
    // The outer edge of EVE's atmosphere / lava glow sits about 3 % outside the surface.
    return { cx, cy, r: found[Math.floor(found.length / 2)] * 0.97 };
  }

  /** The circle through three clicks on the rim. */
  function circleFrom3(a, b, c) {
    const d = 2 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if (Math.abs(d) < 1e-9) return null;
    const sq = (p) => p[0] * p[0] + p[1] * p[1];
    const cx = (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d;
    const cy = (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d;
    return { cx, cy, r: Math.hypot(a[0] - cx, a[1] - cy) };
  }

  /**
   * Lock on from clicks where each click says only what *kind* of building it is (several
   * storage facilities look alike): tries the likely matches and keeps the one where every
   * click lands on a building of its kind. clicks = [{ px, py, candidates: [{ key, lat, lon }] }].
   * Returns { Q, mirrored, error, matched: [key per click], misses: [radians per click] } or null.
   */
  function solveMatching(clicks, disc) {
    let best = null;
    for (const D of [Infinity, 12, 8, 6, 5, 4, 3.5, 3, 2.6, 2.3, 2, 1.8, 1.6, 1.45, 1.3]) {
      const got = solveMatchingAt(clicks, { ...disc, D });
      if (got && (!best || got.error < best.error - 1e-9)) best = { ...got, D };
    }
    // Refine the distance between its neighbours.
    if (best) {
      for (let step = 0; step < 12; step++) {
        const span = isFinite(best.D) ? best.D * 0.08 : 0;
        if (!span) break;
        for (const D of [best.D - span, best.D + span]) {
          if (D <= 1.05) continue;
          const got = solveMatchingAt(clicks, { ...disc, D });
          if (got && got.error < best.error - 1e-9) best = { ...got, D };
        }
      }
    }
    return best;
  }
  function solveMatchingAt(clicks, disc) {
    const usable = clicks.map((c) => ({ ...c, cam: toSphere(c.px, c.py, disc) })).filter((c) => c.cam && c.candidates.length);
    if (usable.length < 2) return null;
    // Seed from the two clicks farthest apart (best pinned), every pairing of their candidates.
    let si = 0, sj = 1, far = -1;
    for (let i = 0; i < usable.length; i++) for (let j = i + 1; j < usable.length; j++) {
      const d = angle(usable[i].cam, usable[j].cam);
      if (d > far) { far = d; si = i; sj = j; }
    }
    let best = null;
    for (const mirrored of [false, true]) {
      for (const ca of usable[si].candidates.slice(0, 12)) for (const cb of usable[sj].candidates.slice(0, 12)) {
        if (ca.key === cb.key) continue;
        const seed = solveAnchors([{ px: usable[si].px, py: usable[si].py, lat: ca.lat, lon: ca.lon }, { px: usable[sj].px, py: usable[sj].py, lat: cb.lat, lon: cb.lon }], disc, mirrored);
        if (!seed) continue;
        let Q = seed.Q, matched = [];
        for (let round = 0; round < 3; round++) {
          const used = new Set();
          matched = usable.map((c) => {
            let pick = null, miss = Infinity;
            for (const cand of c.candidates) {
              if (used.has(cand.key)) continue;
              const m = angle(apply(Q, planetVector(cand.lat, cand.lon, mirrored)), c.cam);
              if (m < miss) { miss = m; pick = cand; }
            }
            if (pick) used.add(pick.key);
            return pick;
          });
          const pairs = usable.map((c, i) => matched[i] && { px: c.px, py: c.py, lat: matched[i].lat, lon: matched[i].lon }).filter(Boolean);
          const solved = solveAnchors(pairs, disc, mirrored);
          if (!solved) break;
          Q = solved.Q;
        }
        const misses = usable.map((c, i) => (matched[i] ? angle(apply(Q, planetVector(matched[i].lat, matched[i].lon, mirrored)), c.cam) : Math.PI));
        const error = misses.reduce((a, b) => a + b, 0) / misses.length;
        let spread = 0;
        for (let a = 0; a < matched.length; a++) for (let b = a + 1; b < matched.length; b++) {
          if (matched[a] && matched[b]) spread = Math.max(spread, angle(planetVector(matched[a].lat, matched[a].lon, mirrored), planetVector(matched[b].lat, matched[b].lon, mirrored)));
        }
        if (!best || error < best.error - 1e-9) best = { Q, mirrored, error, matched: matched.map((m) => (m ? m.key : null)), misses, spread };
      }
    }
    return best;
  }

  // ---- automatic lock-on from EVE's building markers ----
  /**
   * EVE's building markers in the picture: small dark dots (buildings, the dot in each head's
   * disc) on a brighter planet. Returns [{x, y, strength}], close ones merged, inside the disc
   * away from the rim.
   */
  function findMarkers(rgba, width, height, disc) {
    // Dark specks on the planet: pixels well below their neighbourhood's average, grouped into
    // small blobs (a building's dot, or the hollow ring EVE draws for one seen from afar).
    const L = new Float32Array(width * height);
    for (let i = 0; i < L.length; i++) L[i] = 0.3 * rgba[i * 4] + 0.59 * rgba[i * 4 + 1] + 0.11 * rgba[i * 4 + 2];
    const W1 = width + 1, sum = new Float64Array(W1 * (height + 1));
    for (let y = 0; y < height; y++) {
      let row = 0;
      for (let x = 0; x < width; x++) { row += L[y * width + x]; sum[(y + 1) * W1 + x + 1] = sum[y * W1 + x + 1] + row; }
    }
    const R = 7;
    const mean = (x, y) => {
      const xa = Math.max(0, x - R), xb = Math.min(width, x + R + 1), ya = Math.max(0, y - R), yb = Math.min(height, y + R + 1);
      return (sum[yb * W1 + xb] - sum[ya * W1 + xb] - sum[yb * W1 + xa] + sum[ya * W1 + xa]) / ((xb - xa) * (yb - ya));
    };
    const r2 = (disc.r * 0.93) ** 2;
    const dark = new Uint8Array(width * height);
    const x0 = Math.max(1, Math.floor(disc.cx - disc.r)), x1 = Math.min(width - 2, Math.ceil(disc.cx + disc.r));
    const y0 = Math.max(1, Math.floor(disc.cy - disc.r)), y1 = Math.min(height - 2, Math.ceil(disc.cy + disc.r));
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) {
      if ((x - disc.cx) ** 2 + (y - disc.cy) ** 2 > r2) continue;
      const m = mean(x, y);
      if (m > 45 && L[y * width + x] < m - Math.max(18, m * 0.22)) dark[y * width + x] = 1;
    }
    const found = [];
    const stack = [];
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) {
      if (!dark[y * width + x]) continue;
      let n = 0, sx = 0, sy = 0, minX = x, maxX = x, minY = y, maxY = y, depth = 0;
      stack.push(y * width + x); dark[y * width + x] = 0;
      while (stack.length) {
        const k = stack.pop(), px = k % width, py = (k - px) / width;
        n++; sx += px; sy += py; depth += mean(px, py) - L[k];
        if (px < minX) minX = px; if (px > maxX) maxX = px; if (py < minY) minY = py; if (py > maxY) maxY = py;
        for (const nk of [k - 1, k + 1, k - width, k + width]) if (dark[nk]) { dark[nk] = 0; stack.push(nk); }
        if (n > 400) break;
      }
      const w = maxX - minX + 1, h = maxY - minY + 1;
      // A speck: a few pixels to a small ring, about as wide as tall (not a line or a big patch).
      if (n < 3 || n > 120 || w > 16 || h > 16 || w > 3 * h || h > 3 * w) continue;
      found.push({ x: sx / n, y: sy / n, strength: depth / n, n });
    }
    found.sort((a, b) => b.strength - a.strength);
    return found.slice(0, 200);
  }

  /**
   * Which way the planet faces, from markers alone: tries each colony's layout (buildings and
   * heads) against the dots, two points at a time, and keeps the pose that lines up the most.
   * colonies = [{ owner, points: [{ lat, lon }] }]. Returns { Q, D, mirrored, owner, inliers, of } or null.
   */
  function autoLock(markers, colonies, disc, options) {
    const opts = options || {};
    const Ds = opts.Ds || [Infinity, 6, 4, 3, 2.5, 2.1, 1.8, 1.6];
    const mirrors = opts.mirrors || [true, false];
    const tolPx = opts.tolPx || 6;
    let best = null;
    for (const D of Ds) {
      const view = { ...disc, D };
      const cams = markers.map((m) => toSphere(m.x, m.y, view)).map((v, i) => v && { v, m: markers[i] }).filter(Boolean);
      if (cams.length < 3) continue;
      // Pairs of dots far enough apart to pin a direction.
      const dotPairs = [];
      for (let i = 0; i < cams.length; i++) for (let j = i + 1; j < cams.length; j++) {
        const d = Math.hypot(cams[i].m.x - cams[j].m.x, cams[i].m.y - cams[j].m.y);
        if (d > 25) dotPairs.push([i, j, angle(cams[i].v, cams[j].v)]);
      }
      dotPairs.sort((a, b) => b[2] - a[2]);
      const usePairs = dotPairs.slice(0, 400);
      const tolAngle = (tolPx * 1.5) / disc.r;
      for (const mirrored of mirrors) for (const colony of colonies) {
        const pts = colony.points.map((p) => planetVector(p.lat, p.lon, mirrored));
        if (pts.length < 3) continue;
        const pinPairs = [];
        for (let a = 0; a < pts.length; a++) for (let b = a + 1; b < pts.length; b++) pinPairs.push([a, b, angle(pts[a], pts[b])]);
        pinPairs.sort((p, q) => p[2] - q[2]);
        for (const [i, j, dAng] of usePairs) {
          // Pin pairs about as far apart as these two dots.
          let lo = 0, hi = pinPairs.length;
          while (lo < hi) { const mid = (lo + hi) >> 1; if (pinPairs[mid][2] < dAng - tolAngle) lo = mid + 1; else hi = mid; }
          for (let k = lo; k < pinPairs.length && pinPairs[k][2] <= dAng + tolAngle; k++) {
            for (const [a, b] of [[pinPairs[k][0], pinPairs[k][1]], [pinPairs[k][1], pinPairs[k][0]]]) {
              const ci = cams[i].v, cj = cams[j].v;
              const Q = fitRotation([pts[a], pts[b], norm(cross(pts[a], pts[b]))], [ci, cj, norm(cross(ci, cj))]);
              // Each dot counts once (a tight colony can't pile all its buildings on one dot).
              let inliers = 0, err = 0;
              const used = new Set();
              for (const p of pts) {
                const c = apply(Q, p);
                const pic = toPicture(c, view);
                if (!pic.front) continue;
                let nearest = Infinity, which = -1;
                for (let n = 0; n < markers.length; n++) {
                  if (used.has(n)) continue;
                  const m = markers[n], d = (m.x - pic.x) ** 2 + (m.y - pic.y) ** 2;
                  if (d < nearest) { nearest = d; which = n; }
                }
                if (nearest < tolPx * tolPx) { inliers++; err += Math.sqrt(nearest); used.add(which); }
              }
              if (inliers >= 5 && (!best || inliers > best.inliers || (inliers === best.inliers && err < best.err))) {
                best = { Q, D, mirrored, owner: colony.owner, inliers, of: pts.length, err };
              }
            }
          }
        }
      }
    }
    if (!best) return null;
    return refineWithMarkers(markers, colonies.find((c) => c.owner === best.owner).points, best, disc) || best;
  }

  /** Pull a pose onto the markers near where its buildings should be (every frame, no clicks). */
  function refineWithMarkers(markers, points, pose, disc, tolPx) {
    const tol = tolPx || 10;
    const view = { ...disc, D: pose.D };
    const pairs = [];
    for (const p of points) {
      const pv = planetVector(p.lat, p.lon, pose.mirrored), pic = toPicture(apply(pose.Q, pv), view);
      if (!pic.front) continue;
      let nearest = null, dist = tol * tol;
      for (const m of markers) { const d = (m.x - pic.x) ** 2 + (m.y - pic.y) ** 2; if (d < dist) { dist = d; nearest = m; } }
      if (nearest) { const cam = toSphere(nearest.x, nearest.y, view); if (cam) pairs.push([pv, cam]); }
    }
    if (pairs.length < 4) return null;
    const Q = fitRotation(pairs.map((p) => p[0]), pairs.map((p) => p[1]));
    return { ...pose, Q, inliers: pairs.length };
  }

  const api = { GRID_W, GRID_H, SIZE, heat, fitRotation, rotationAbout, mul, apply, transpose, identity, angle, planetVector, latLon, toSphere, toPicture, facingOf,
    solveAnchors, solveAnchorsEitherWay, lockOne, northUp, poseQ, poseOf, upright, samplePoints, bareGrey, poseScore, alignToMap, findOnMap, solveMatching, findMarkers, autoLock, refineWithMarkers, fitCircle, findRim, rimRadius, circleFrom3, correctWithAnchor, greyDisc, trackRotation, newMap, paint, cells, fromCells, coverage, missingHint, unmix, newBare, bareCoverage, bareToBytes, bareFromBytes, fillHoles };
  if (typeof module !== "undefined" && module.exports) module.exports = api; else root.PiScanEngine = api;
})(typeof window !== "undefined" ? window : globalThis);
