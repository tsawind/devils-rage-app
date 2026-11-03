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

  // Picture point (pixels) → camera-space point on the near side, or null outside the disc.
  function toSphere(px, py, disc) {
    const x = (px - disc.cx) / disc.r, y = -(py - disc.cy) / disc.r, d = x * x + y * y;
    if (d >= 1) return null;
    return [x, y, Math.sqrt(1 - d)];
  }
  function toPicture(v, disc) {
    return { x: disc.cx + v[0] * disc.r, y: disc.cy - v[1] * disc.r, front: v[2] > 0 };
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

  // ---- tracking ----
  // A frame for tracking: the disc cropped and scaled to SIZE × SIZE grey values.
  const SIZE = 256;

  function greyDisc(rgba, width, height, disc) {
    const out = new Float32Array(SIZE * SIZE);
    const scale = (2 * disc.r) / SIZE, x0 = disc.cx - disc.r, y0 = disc.cy - disc.r;
    for (let j = 0; j < SIZE; j++) {
      const y = Math.min(height - 1, Math.max(0, Math.round(y0 + (j + 0.5) * scale)));
      for (let i = 0; i < SIZE; i++) {
        const x = Math.min(width - 1, Math.max(0, Math.round(x0 + (i + 0.5) * scale)));
        const k = (y * width + x) * 4;
        out[j * SIZE + i] = 0.3 * rgba[k] + 0.59 * rgba[k + 1] + 0.11 * rgba[k + 2];
      }
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
  const unit = (x, y, size) => { const u = (x + 0.5) / (size / 2) - 1, v = 1 - (y + 0.5) / (size / 2), d = u * u + v * v; return d >= 1 ? null : [u, v, Math.sqrt(1 - d)]; };

  /**
   * How the planet turned between two grey discs: R with cur ≈ R·prev (camera space), from
   * matched patches (coarse at half size, then refined). Returns { R, used, spread (radians), moved (radians) }
   * or null when too few patches agree (moved too fast, or nothing to grip on).
   */
  function trackRotation(prev, cur) {
    const P = 4, step = 18, inner = 0.72;
    const prevHalf = half(prev, SIZE), curHalf = half(cur, SIZE);
    const as = [], bs = [];
    for (let y = step; y < SIZE - step; y += step) for (let x = step; x < SIZE - step; x += step) {
      const a = unit(x, y, SIZE);
      if (!a || Math.hypot(a[0], a[1]) > inner) continue;
      if (textured(prev, SIZE, x, y, P) < 12) continue;
      const hx = x >> 1, hy = y >> 1;
      if (hx - 3 < 0 || hy - 3 < 0) continue;
      const coarse = search(prevHalf, curHalf, SIZE / 2, hx, hy, hx, hy, 14, 2);
      const fine = search(prev, cur, SIZE, x, y, coarse.x * 2, coarse.y * 2, 3, P);
      const b = unit(fine.x, fine.y, SIZE);
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
    const moved = Math.acos(Math.max(-1, Math.min(1, (R[0] + R[4] + R[8] - 1) / 2)));
    return { R, used, of: as.length, spread: miss[Math.floor(miss.length / 2)], moved };
  }

  // ---- painting ----
  function newMap() { return { sum: new Float32Array(GRID_W * GRID_H), weight: new Float32Array(GRID_W * GRID_H) }; }

  /** Paint one picture onto a resource's map: every disc pixel facing the camera well enough. */
  function paint(map, rgba, width, height, disc, Q, mirrored, every) {
    const Qt = transpose(Q), step = every || 2;
    let painted = 0;
    const y0 = Math.max(0, Math.floor(disc.cy - disc.r)), y1 = Math.min(height - 1, Math.ceil(disc.cy + disc.r));
    const x0 = Math.max(0, Math.floor(disc.cx - disc.r)), x1 = Math.min(width - 1, Math.ceil(disc.cx + disc.r));
    for (let y = y0; y <= y1; y += step) for (let x = x0; x <= x1; x += step) {
      const v = toSphere(x, y, disc);
      if (!v || v[2] < 0.35) continue; // near the edge the picture is too squashed to trust
      const { lat, lon } = latLon(apply(Qt, v), mirrored);
      const row = Math.min(GRID_H - 1, Math.floor(lat / Math.PI * GRID_H)), col = Math.min(GRID_W - 1, Math.floor(lon / (2 * Math.PI) * GRID_W));
      const k = (y * width + x) * 4, w = v[2] * v[2];
      map.sum[row * GRID_W + col] += heat(rgba[k], rgba[k + 1], rgba[k + 2]) * w;
      map.weight[row * GRID_W + col] += w;
      painted++;
    }
    return painted;
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
    const from = around ? around.r * 0.75 : 20, to = around ? around.r * 1.35 : Math.min(width, height) * 0.6;
    // How dark space is here: the darkest tenth of a ring well outside the guess.
    const points = [];
    for (let i = 0; i < 72; i++) {
      const a = (i / 72) * Math.PI * 2, dx = Math.cos(a), dy = Math.sin(a);
      let dark = 0, edge = null, inside = false;
      for (let r = from; r < to; r += 2) {
        const x = cx + dx * r, y = cy + dy * r;
        if (x < 1 || y < 1 || x >= width - 1 || y >= height - 1) { edge = null; break; }
        if (lum(x, y) < 18) { dark++; if (dark === 1) edge = r; if (dark >= 5 && inside) break; } else { dark = 0; edge = null; inside = true; }
      }
      if (edge && dark >= 5 && inside) points.push([cx + dx * edge, cy + dy * edge]);
    }
    if (points.length < 10) return null;
    const circle = fitCircle(points);
    if (!circle || circle.r < 40) return null;
    if (around && (Math.abs(circle.r - around.r) > around.r * 0.3 || Math.hypot(circle.cx - around.cx, circle.cy - around.cy) > around.r * 0.15)) return null;
    return circle;
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
        if (!best || error < best.error - 1e-9) best = { Q, mirrored, error, matched: matched.map((m) => (m ? m.key : null)), misses };
      }
    }
    return best;
  }

  const api = { GRID_W, GRID_H, SIZE, heat, fitRotation, rotationAbout, mul, apply, transpose, identity, angle, planetVector, latLon, toSphere, toPicture,
    solveAnchors, solveAnchorsEitherWay, solveMatching, fitCircle, findRim, circleFrom3, correctWithAnchor, greyDisc, trackRotation, newMap, paint, cells, fromCells, coverage, missingHint };
  if (typeof module !== "undefined" && module.exports) module.exports = api; else root.PiScanEngine = api;
})(typeof window !== "undefined" ? window : globalThis);
