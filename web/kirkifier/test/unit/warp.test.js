import { test } from "node:test";
import assert from "node:assert/strict";

import {
  CORE,
  MIN_SIZE,
  faceFrame,
  forward,
  frameBounds,
  inverseAt,
  inverseTable,
  warpAll,
  warpFace,
} from "../../src/warp.js";

const SIZES = [MIN_SIZE, 0.4, 0.55, 0.8, 0.99];

test("forward is strictly increasing, fixes the edge, and is a uniform shrink in the core", () => {
  for (const size of SIZES) {
    let prev = -1;
    for (let i = 0; i <= 2000; i++) {
      const r = i / 2000;
      const f = forward(r, size);
      assert.ok(f > prev, `not increasing at r=${r}, size=${size}`);
      prev = f;
    }
    assert.equal(forward(1, size), 1);
    assert.ok(Math.abs(forward(CORE / 2, size) - (CORE / 2) * size) < 1e-12);
    // Continuous into the band.
    const below = forward(CORE - 1e-9, size);
    const above = forward(CORE + 1e-9, size);
    assert.ok(Math.abs(above - below) < 1e-6);
  }
});

test("the slope at the edge is 1, so the warp meets the photo without a kink", () => {
  for (const size of SIZES) {
    const h = 1e-6;
    const slope = (forward(1, size) - forward(1 - h, size)) / h;
    assert.ok(Math.abs(slope - 1) < 1e-3, `slope ${slope} at size ${size}`);
  }
});

test("the inverse table undoes forward and never leaves the unit disc", () => {
  for (const size of SIZES) {
    const table = inverseTable(size);
    assert.equal(inverseAt(table, 0), 0);
    assert.equal(inverseAt(table, 1), 1);
    for (let i = 1; i < 500; i++) {
      const t = i / 500;
      const r = inverseAt(table, t);
      assert.ok(r >= 0 && r <= 1);
      assert.ok(Math.abs(forward(r, size) - t) < 2e-3, `size ${size}, t ${t}`);
    }
  }
});

// An upright face: eyes level, nose below, mouth below that.
const UPRIGHT = [
  [80, 100],
  [120, 100],
  [100, 125],
  [85, 145],
  [115, 145],
];

function rotate(points, angle, [ox, oy]) {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  return points.map(([x, y]) => [ox + (x - ox) * c - (y - oy) * s, oy + (x - ox) * s + (y - oy) * c]);
}

test("an upright face gets an unrotated ellipse centred above its landmarks", () => {
  const f = faceFrame(UPRIGHT);
  assert.ok(Math.abs(f.angle) < 1e-12);
  assert.ok(Math.abs(f.cx - 100) < 1e-9);
  const centroidY = UPRIGHT.reduce((a, p) => a + p[1], 0) / 5;
  assert.ok(f.cy < centroidY, "centre should sit above the centroid");
  assert.ok(f.b > f.a, "a face is taller than it is wide");
});

test("a rotated face gets the same ellipse, rotated with it", () => {
  const base = faceFrame(UPRIGHT);
  for (const angle of [Math.PI / 6, Math.PI / 2, Math.PI]) {
    const f = faceFrame(rotate(UPRIGHT, angle, [100, 120]));
    const d = Math.atan2(Math.sin(f.angle - angle), Math.cos(f.angle - angle));
    assert.ok(Math.abs(d) < 1e-9, `angle ${f.angle} vs ${angle}`);
    assert.ok(Math.abs(f.a - base.a) < 1e-9);
    assert.ok(Math.abs(f.b - base.b) < 1e-9);
  }
});

test("a face turned away keeps its size from the eye-to-mouth drop", () => {
  // Same face with the eyes foreshortened to half their distance.
  const turned = [[90, 100], [110, 100], [96, 125], [90, 145], [108, 145]];
  const f = faceFrame(turned);
  const g = faceFrame(UPRIGHT);
  assert.ok(f.a > g.a * 0.8, "foreshortened eyes must not halve the ellipse");
});

function image(w, h, fill) {
  const px = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const [r, g, b, a] = fill(x, y);
      const i = (y * w + x) * 4;
      px[i] = r;
      px[i + 1] = g;
      px[i + 2] = b;
      px[i + 3] = a;
    }
  }
  return px;
}

const noise = (x, y) => {
  const v = ((x * 73856093) ^ (y * 19349663)) >>> 0;
  return [v & 255, (v >> 8) & 255, (v >> 16) & 255, 255];
};

test("nothing outside the ellipse changes, byte for byte", () => {
  const w = 200;
  const h = 240;
  const before = image(w, h, noise);
  const after = before.slice();
  const frame = faceFrame(UPRIGHT);
  warpFace(after, w, h, frame, inverseTable(0.5));
  const cos = Math.cos(frame.angle);
  const sin = Math.sin(frame.angle);
  let changedInside = 0;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const dx = x + 0.5 - frame.cx;
      const dy = y + 0.5 - frame.cy;
      const t = Math.hypot((dx * cos + dy * sin) / frame.a, (-dx * sin + dy * cos) / frame.b);
      const i = (y * w + x) * 4;
      const same = before[i] === after[i] && before[i + 1] === after[i + 1] && before[i + 2] === after[i + 2];
      if (t >= 1) assert.ok(same, `pixel ${x},${y} outside the ellipse changed`);
      else if (!same) changedInside++;
    }
  }
  assert.ok(changedInside > 1000, "the warp should change the face");
});

test("a flat colour stays that colour: the supersampling weights sum to one", () => {
  const w = 200;
  const h = 240;
  const px = image(w, h, () => [120, 64, 200, 255]);
  warpFace(px, w, h, faceFrame(UPRIGHT), inverseTable(MIN_SIZE));
  for (let i = 0; i < px.length; i += 4) {
    assert.ok(Math.abs(px[i] - 120) <= 1 && Math.abs(px[i + 1] - 64) <= 1 && Math.abs(px[i + 2] - 200) <= 1);
    assert.equal(px[i + 3], 255);
  }
});

test("size 1 is the identity", () => {
  const w = 200;
  const h = 240;
  const before = image(w, h, noise);
  const after = before.slice();
  warpAll(after, w, h, [faceFrame(UPRIGHT)], 1);
  assert.deepEqual(after, before);
});

test("a face at the edge of the image warps without reading outside it", () => {
  const w = 120;
  const h = 130;
  const px = image(w, h, noise);
  const edge = UPRIGHT.map(([x, y]) => [x - 70, y - 90]);
  const frame = faceFrame(edge);
  const b = frameBounds(frame, w, h);
  assert.equal(b.x0, 0);
  assert.equal(b.y0, 0);
  warpFace(px, w, h, frame, inverseTable(0.5));
  assert.ok(px.every((v) => Number.isFinite(v)));
});
