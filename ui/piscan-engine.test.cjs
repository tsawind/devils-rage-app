// Run: node ui/piscan-engine.test.cjs — renders a made-up planet, turns it, checks the scan maths.
require("./piscan-engine.js");
const E = globalThis.PiScanEngine;

let fails = 0;
const check = (what, ok) => { console.log((ok ? "ok   " : "FAIL ") + what); if (!ok) fails++; };

// A planet surface: grey terrain with texture everywhere, and a resource hotspot (red → white) near lat 1.2, lon 2.0.
const hot = E.planetVector(1.2, 2.0, false);
function surface(p) {
  const t = 90 + 40 * Math.sin(p[0] * 23 + p[1] * 7) * Math.cos(p[2] * 19 - p[0] * 5) + 25 * Math.sin(p[1] * 41 + p[2] * 13);
  const d = E.angle(p, hot);
  if (d < 0.35) { const k = 1 - d / 0.35; return [255, Math.round(80 + 175 * k * k), Math.round(60 + 195 * k * k * k)]; }
  return [t, t * 0.95, t * 0.9];
}
const W = 640, H = 480, disc = { cx: 320, cy: 240, r: 200 };
let STRIPES = false;
function render(Q, mirrored) {
  const rgba = new Uint8ClampedArray(W * H * 4), Qt = E.transpose(Q);
  for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
    const v = E.toSphere(x, y, disc), k = (y * W + x) * 4;
    rgba[k + 3] = 255;
    if (!v) continue;
    const ll = E.latLon(E.apply(Qt, v), mirrored);
    const c = surface(E.planetVector(ll.lat, ll.lon, false));
    // EVE's scan overlay: fine diagonal stripes fixed on screen.
    const stripe = STRIPES && ((x + y) % 6 < 2) ? 0.55 : 1;
    rgba[k] = c[0] * stripe; rgba[k + 1] = c[1] * stripe; rgba[k + 2] = c[2] * stripe;
  }
  return rgba;
}

// fitRotation finds a known rotation.
const R0 = E.rotationAbout([0.3, 1, 0.2], 0.4);
const pts = [[1, 0, 0], [0, 1, 0], [0, 0, 1], [0.5, 0.5, 0.7]];
const fit = E.fitRotation(pts, pts.map((p) => E.apply(R0, p)));
check("fitRotation recovers a rotation", fit.every((v, i) => Math.abs(v - R0[i]) < 1e-6));

// Heat colours.
check("white is hottest, blue coolest, grey none", E.heat(255, 255, 255) === 1 && Math.abs(E.heat(40, 60, 230) - 0.2) < 0.05 && E.heat(120, 120, 120) === 0);
check("red hotter than green", E.heat(230, 30, 30) > E.heat(30, 220, 30));

// Anchors: three buildings pointed out on the picture give Q back (and say it isn't mirrored).
// Facing: the hotspot a little off the middle of the picture, north roughly up.
const east = (() => { const c = [-hot[1], hot[0], 0], l = Math.hypot(...c); return c.map((x) => x / l); })();
const Q = E.mul(E.rotationAbout([1, 1, 0], 0.25), E.fitRotation([hot, east], [[0, 0, 1], [1, 0, 0]]));
const buildings = [{ lat: 1.15, lon: 1.95 }, { lat: 1.3, lon: 2.1 }, { lat: 1.1, lon: 2.15 }];
const anchors = buildings.map((b) => { const p = E.toPicture(E.apply(Q, E.planetVector(b.lat, b.lon, false)), disc); return { px: p.x, py: p.y, ...b }; });
check("the buildings are on the visible side", anchors.every((a) => E.toSphere(a.px, a.py, disc)));
const solved = E.solveAnchorsEitherWay(anchors, disc);
check("anchors give the planet's facing back", solved && Q.every((v, i) => Math.abs(v - solved.Q[i]) < 1e-3));
check("and it isn't mirrored", solved && !solved.mirrored);
const hotM = E.planetVector(1.2, 2.0, true);
const Qm = E.fitRotation([hotM, [-hotM[1], hotM[0], 0]], [[0, 0, 1], [1, 0, 0]]);
const mirroredAnchors = buildings.map((b) => { const p = E.toPicture(E.apply(Qm, E.planetVector(b.lat, b.lon, true)), disc); return { px: p.x, py: p.y, ...b }; });
const solvedM = E.solveAnchorsEitherWay(mirroredAnchors, disc);
check("a mirrored planet is spotted", solvedM && solvedM.mirrored);
const two = E.solveAnchors(anchors.slice(0, 2), disc, false);
check("two anchors are enough", two && two.error < 1e-3);

// Drift fix: a slightly wrong Q is pulled so the clicked building lands on its click.
const off = E.mul(E.rotationAbout([0, 1, 0], 0.05), Q);
const fixedQ = E.correctWithAnchor(off, anchors[0], disc, false);
const landed = E.toPicture(E.apply(fixedQ, E.planetVector(anchors[0].lat, anchors[0].lon, false)), disc);
check("drift fix puts the building back", Math.hypot(landed.x - anchors[0].px, landed.y - anchors[0].py) < 0.5);

// Tracking: turn the planet a few degrees between frames; the tracker finds the turn.
const turns = [[[0, 1, 0], 0.06], [[1, 0.2, 0], -0.08], [[0.3, -1, 0.1], 0.12], [[0, 0, 1], 0.05]];
let ok = 0;
for (const [axis, rad] of turns) {
  const R = E.rotationAbout(axis, rad);
  const a = E.greyDisc(render(Q), W, H, disc), b = E.greyDisc(render(E.mul(R, Q)), W, H, disc);
  const t = E.trackRotation(a, b);
  const err = t ? E.angle(E.apply(t.R, [0.3, 0.4, 0.86]), E.apply(R, [0.3, 0.4, 0.86])) : 9;
  console.log(`     turn ${rad} rad about ${axis}: ${t ? `${t.used}/${t.of} patches, error ${(err * 180 / Math.PI).toFixed(2)}°` : "lost"}`);
  if (t && err < 0.01) ok++;
}
check("tracker follows each turn within ~0.6°", ok === turns.length);

// A whole scan: anchors, then turns, painting each frame; the hotspot lands where it is.
let q = solved.Q;
const map = E.newMap();
let frame = render(Q), grey = E.greyDisc(frame, W, H, disc), truth = Q;
E.paint(map, frame, W, H, disc, q, false, 2);
for (let i = 0; i < 40; i++) {
  const R = E.rotationAbout([Math.sin(i / 7), 1, 0.3 * Math.cos(i / 5)], 0.07);
  truth = E.mul(R, truth);
  frame = render(truth);
  const g = E.greyDisc(frame, W, H, disc), t = E.trackRotation(grey, g);
  if (t) q = E.mul(t.R, q);
  grey = g;
  E.paint(map, frame, W, H, disc, q, false, 3);
}
const drift = E.angle(E.apply(q, [0, 0, 1]), E.apply(truth, [0, 0, 1])) * 180 / Math.PI;
console.log(`     after 40 frames: drift ${drift.toFixed(2)}°, coverage ${(E.coverage(map) * 100).toFixed(0)}%`);
check("drift stays small over a long turn", drift < 3);
check("a long turn covers much of the planet", E.coverage(map) > 0.4);
const bytes = E.cells(map);
const at = (lat, lon) => bytes[Math.floor(lat / Math.PI * 180) * 360 + Math.floor(lon / (2 * Math.PI) * 360)];
console.log("     hotspot cell", at(1.2, 2.0), "near", at(1.25, 2.05), at(1.15,1.95));
check("the hotspot is hot on the map", at(1.2, 2.0) > 150);
check("far from it is cold", at(1.2, 2.0 + 1.2) > 0 ? at(1.2, 2.0 + 1.2) < 30 : true);
const back = E.fromCells(bytes);
check("a map comes back from its bytes", E.cells(back).every((v, i) => Math.abs(v - bytes[i]) <= 1));
check("a missing-side hint is given", typeof E.missingHint(map, q, false) === "string");

// Clicks that only say the kind of building: two look-alike storage facilities get sorted out.
const pins = [{ key: "cc", name: "Command Center", lat: 1.15, lon: 1.95 }, { key: "s1", name: "Storage", lat: 1.3, lon: 2.1 }, { key: "s2", name: "Storage", lat: 1.1, lon: 2.15 }, { key: "s3", name: "Storage", lat: 1.25, lon: 1.85 }];
const clicks = pins.map((pin) => { const p = E.toPicture(E.apply(Q, E.planetVector(pin.lat, pin.lon, false)), disc); return { px: p.x + 1.5, py: p.y - 1, candidates: pins.filter((x) => x.name === pin.name) }; });
const matchedSolve = E.solveMatching(clicks, disc);
check("look-alike buildings are matched by where they are", matchedSolve && matchedSolve.matched.join(",") === "cc,s1,s2,s3");
check("and the facing comes out right", matchedSolve && E.angle(E.apply(matchedSolve.Q, [0, 0, 1]), E.apply(Q, [0, 0, 1])) < 0.02);

// The rim: found on a rendered planet, followed through a zoom, and from three clicks.
const frameRim = render(Q);
const rim = E.findRim(frameRim, W, H, null);
check("finds the planet's edge", rim && Math.abs(rim.cx - 320) < 3 && Math.abs(rim.cy - 240) < 3 && Math.abs(rim.r - 200) < 4);
const zoomed = { cx: 320, cy: 240, r: 200 };
const rim2 = E.findRim(frameRim, W, H, { cx: 320, cy: 240, r: 180 });
check("follows a zoom from the last circle", rim2 && Math.abs(rim2.r - zoomed.r) < 4);
const three = E.circleFrom3([520, 240], [320, 40], [120, 240]);
check("three clicks on the rim make the circle", three && Math.abs(three.cx - 320) < 1e-6 && Math.abs(three.r - 200) < 1e-6);

// Turning fast and steadily: too far to find without looking ahead, found with the last step's turn.
{
  const R = E.rotationAbout([0.2, 1, 0], 0.30);
  const a = E.greyDisc(render(Q), W, H, disc), b = E.greyDisc(render(E.mul(R, Q)), W, H, disc);
  const blind = E.trackRotation(a, b), ahead = E.trackRotation(a, b, undefined, { guess: E.rotationAbout([0.2, 1, 0], 0.27) });
  const err = ahead ? E.angle(E.apply(ahead.R, [0.3, 0.4, 0.86]), E.apply(R, [0.3, 0.4, 0.86])) : 9;
  console.log(`     17° in one step: blind ${blind ? "found " + (blind.moved * 180 / Math.PI).toFixed(1) + "°" : "lost"}, looking ahead error ${(err * 180 / Math.PI).toFixed(2)}°`);
  check("a turn too far to see blind isn't passed off as a small one", !blind || Math.abs(blind.moved - 0.3) < 0.02);
  check("looking ahead with the last turn follows a fast spin", err < 0.01);
}
// Lost, then back over seen ground: a kept picture gives the facing back.
{
  const R = E.rotationAbout([1, 0.3, 0], 0.9);
  const a = E.greyDisc(render(Q), W, H, disc), far = E.greyDisc(render(E.mul(R, Q)), W, H, disc);
  check("a picture of the far side doesn't match (no false grip)", !E.trackRotation(a, far));
  const near = E.rotationAbout([0, 0.4, 1], 0.08), again = E.greyDisc(render(E.mul(near, Q)), W, H, disc);
  const t = E.trackRotation(a, again);
  check("back near a kept picture: the facing comes back", t && E.angle(E.apply(t.R, [0.3, 0.4, 0.86]), E.apply(near, [0.3, 0.4, 0.86])) < 0.01);
}

// One building is enough when the camera doesn't roll (EVE keeps the planet's axis upright).
{
  const d3 = { cx: 320, cy: 240, r: 200, D: 3 };
  let worst = 0, flips = 0;
  for (const [tilt, spin, lat, lon] of [[-0.8, 0.4, 1.1, 0.5], [-1.2, 2.0, 0.7, 2.3], [-0.3, -1.0, 1.6, -0.9], [0.5, 3.0, 2.0, 2.9]]) {
    const truthQ = E.mul(E.rotationAbout([1, 0, 0], tilt), E.rotationAbout([0, 0, 1], spin));
    const pic = E.toPicture(E.apply(truthQ, E.planetVector(lat, lon, false)), d3);
    if (!pic.front) continue;
    const got = E.lockOne({ px: pic.x, py: pic.y, lat, lon }, d3, false, E.northUp(truthQ));
    const err = got ? Math.max(E.angle(E.apply(got, [1, 0, 0]), E.apply(truthQ, [1, 0, 0])), E.angle(E.apply(got, [0, 0, 1]), E.apply(truthQ, [0, 0, 1]))) : 9;
    worst = Math.max(worst, err);
    const other = E.lockOne({ px: pic.x, py: pic.y, lat, lon }, d3, false, !E.northUp(truthQ));
    if (other && E.angle(E.apply(other, [0, 0, 1]), E.apply(truthQ, [0, 0, 1])) > 0.05) flips++;
  }
  console.log(`     one building + upright pole: worst error ${(worst * 180 / Math.PI).toFixed(3)}°`);
  check("one clicked building locks on when the pole stands upright", worst < 0.002);
  check("the other way up gives a different facing (for Flip)", flips > 0);
}

// With EVE's on-screen stripes over everything, a long turn still doesn't drift.
STRIPES = true;
{
  let q2 = Q, truth2 = Q, grey2 = E.greyDisc(render(Q), W, H, disc);
  for (let i = 0; i < 30; i++) {
    const R = E.rotationAbout([Math.sin(i / 7), 1, 0.3 * Math.cos(i / 5)], 0.07);
    truth2 = E.mul(R, truth2);
    const g = E.greyDisc(render(truth2), W, H, disc), t = E.trackRotation(grey2, g);
    if (t) q2 = E.mul(t.R, q2);
    grey2 = g;
  }
  const drift2 = E.angle(E.apply(q2, [0, 0, 1]), E.apply(truth2, [0, 0, 1])) * 180 / Math.PI;
  console.log(`     with on-screen stripes, after 30 frames: drift ${drift2.toFixed(2)}°`);
  check("on-screen stripes don't make it drift", drift2 < 3);
}
STRIPES = false;

// EVE's close camera: buildings clicked on a planet seen from 2.5 radii away.
const near = { cx: 320, cy: 240, r: 200, D: 2.5 };
const nearClicks = pins.map((pin) => { const p = E.toPicture(E.apply(Q, E.planetVector(pin.lat, pin.lon, false)), near); return { px: p.x, py: p.y, candidates: pins.filter((x) => x.name === pin.name) }; });
check("close-camera picture points come back on the planet", nearClicks.every((c) => E.toSphere(c.px, c.py, near)));
const nearSolve = E.solveMatching(nearClicks, { cx: 320, cy: 240, r: 200 });
console.log(`     close camera: distance ${nearSolve && nearSolve.D.toFixed(2)}, fit ${(nearSolve.error * 180 / Math.PI).toFixed(2)}°`);
check("works out the camera distance", nearSolve && Math.abs(nearSolve.D - 2.5) < 0.4);
check("and the buildings fit", nearSolve && nearSolve.error < 0.01);
const rt = E.toSphere(E.toPicture([0.3, 0.5, Math.sqrt(1 - 0.34)], near).x, E.toPicture([0.3, 0.5, Math.sqrt(1 - 0.34)], near).y, near);
check("picture ↔ planet round trip (close camera)", rt && Math.abs(rt[0] - 0.3) < 1e-9 && Math.abs(rt[1] - 0.5) < 1e-9);

// Unmixing: a resource level laid over a lava-red planet comes back out.
{
  const base = [150, 60, 30];
  const BAND = [[20, 14, 25], [21, 21, 40], [19, 36, 55], [18, 57, 49], [18, 75, 33], [37, 99, 7], [74, 112, 7], [118, 126, 6], [145, 138, 5], [161, 109, 12], [169, 58, 4], [179, 5, 3], [187, 43, 42], [195, 191, 190]];
  let worst = 0;
  for (const level of [0.2, 0.45, 0.7, 0.9]) {
    const t = level * (BAND.length - 1), a = Math.floor(t), b = Math.min(BAND.length - 1, a + 1), f = t - a;
    const c = [0, 1, 2].map((i) => 0.65 * base[i] + 0.35 * 1.6 * (BAND[a][i] * (1 - f) + BAND[b][i] * f));
    worst = Math.max(worst, Math.abs(E.unmix(c[0], c[1], c[2], base[0], base[1], base[2]) - level));
  }
  check("unmixing finds the level under a lava-red planet", worst < 0.03);
  check("glowing lava is skipped", E.unmix(250, 230, 120, 250, 220, 100) === null);
}

// A map is settled once seen: more turning, other light or another character changes nothing.
{
  const solid = (v) => { const a = new Uint8ClampedArray(W * H * 4); for (let i = 0; i < a.length; i += 4) { a[i] = v; a[i + 1] = v; a[i + 2] = v; a[i + 3] = 255; } return a; };
  const b = E.newBare();
  E.paint(b, solid(100), W, H, disc, Q, false, 3, "record");
  const first = E.bareToBytes(b);
  E.paint(b, solid(180), W, H, disc, Q, false, 3, "record"); // same view, brighter light
  const again = E.bareToBytes(b);
  check("the same view again (other light) leaves the bare map alone", first.every((x, i) => x === again[i]));
  const turned = E.mul(E.rotationAbout([0, 1, 0], 0.5), Q);
  E.paint(b, solid(180), W, H, disc, turned, false, 3, "record");
  const after = E.bareToBytes(b);
  const centre = (() => { const v = E.apply(E.transpose(Q), [0, 0, 1]); const ll = E.latLon(v, false); return Math.floor(ll.lat / Math.PI * 180) * 360 + Math.floor(ll.lon / (2 * Math.PI) * 360); })();
  check("a spot seen face-on keeps its look after turning", after[centre * 3] === 100);
  check("turning still fills in what wasn't seen", E.bareCoverage(b) > 0 && after.some((x, i) => i % 3 === 0 && x === 180 && first[i] === 0));
  const saved = E.bareFromBytes(first);
  E.paint(saved, solid(30), W, H, disc, Q, false, 3, "record");
  check("a saved bare map isn't repainted", E.bareToBytes(saved).every((x, i) => x === first[i]));
}

// A bare map recorded under a lock that was off sideways is a pure longitude shift: it turns back.
{
  const b = E.newBare();
  for (let i = 0; i < b.weight.length; i++) { const col = i % E.GRID_W; b.rgb[i * 3] = col % 256; b.rgb[i * 3 + 1] = 50; b.rgb[i * 3 + 2] = 50; b.weight[i] = 1; }
  const there = E.shiftBare(b, 8.75), back = E.shiftBare(there, -8.75);
  let worst = 0; for (let i = 0; i < b.weight.length; i++) { const col = i % E.GRID_W; if (col > 20 && col < 230) worst = Math.max(worst, Math.abs(back.rgb[i * 3] - b.rgb[i * 3])); }
  check("a bare map turned 8.75° and back comes back", worst < 1);
  check("turning moves it along the longitude", Math.abs(there.rgb[(10 * E.GRID_W + 100) * 3] - 91.25) < 0.01);
}

// Specks (lava shining through) go; the smooth field stays.
{
  const m = E.newMap();
  for (let i = 0; i < m.weight.length; i++) { const col = i % E.GRID_W; m.sum[i] = 0.3 + 0.2 * Math.sin(col / 20); m.weight[i] = 1; }
  const smooth = Float32Array.from(m.sum), specks = [5000, 20111, 33333, 47000];
  specks.forEach((i) => { m.sum[i] = 0.95; });
  E.despeckle(m);
  check("specks take the value round them", specks.every((i) => Math.abs(m.sum[i] - smooth[i]) < 0.05));
  check("the smooth field is left alone", m.sum.every((v, i) => specks.includes(i) || Math.abs(v - smooth[i]) < 1e-6));
}

console.log(fails ? `\n${fails} failed` : "\nall passed");
process.exit(fails ? 1 : 0);
