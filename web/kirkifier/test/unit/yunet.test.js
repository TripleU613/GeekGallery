import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

import * as ort from "onnxruntime-web/wasm";
import { decode, detect, iou, nms, padTo32, scalesFor, STRIDES, toInput } from "../../src/yunet.js";

test("sides are padded up to multiples of 32", () => {
  assert.equal(padTo32(1), 32);
  assert.equal(padTo32(32), 32);
  assert.equal(padTo32(33), 64);
  assert.equal(padTo32(427), 448);
});

test("scales keep the aspect ratio and skip repeats", () => {
  const s = scalesFor(1920, 1280);
  assert.deepEqual(
    s.map((x) => [x.w, x.h]),
    [
      [640, 427],
      [320, 213],
      [160, 107],
    ],
  );
  // A tiny image collapses repeated sizes rather than running them twice.
  const tiny = scalesFor(1, 1);
  assert.equal(new Set(tiny.map((x) => `${x.w}x${x.h}`)).size, tiny.length);
});

test("input is planar BGR with zero padding", () => {
  // 2x1 image: red, then blue.
  const rgba = new Uint8ClampedArray([255, 0, 0, 255, 0, 0, 255, 255]);
  const out = toInput(rgba, 2, 1, 32, 32);
  const plane = 32 * 32;
  assert.equal(out.length, 3 * plane);
  assert.deepEqual([out[0], out[plane], out[2 * plane]], [0, 0, 255]); // red pixel as B,G,R
  assert.deepEqual([out[1], out[plane + 1], out[2 * plane + 1]], [255, 0, 0]); // blue pixel
  assert.equal(out[2], 0);
  assert.equal(out[32], 0);
});

function emptyOutputs(padW, padH) {
  const o = {};
  for (const s of STRIDES) {
    const n = (padW / s) * (padH / s);
    o[`cls_${s}`] = new Float32Array(n);
    o[`obj_${s}`] = new Float32Array(n);
    o[`bbox_${s}`] = new Float32Array(n * 4);
    o[`kps_${s}`] = new Float32Array(n * 10);
  }
  return o;
}

test("decode places a box and landmarks the way OpenCV's FaceDetectorYN does", () => {
  const o = emptyOutputs(64, 64);
  // Stride 16 grid is 4x4; light up cell (row 1, col 2).
  const idx = 1 * 4 + 2;
  o.cls_16[idx] = 0.9;
  o.obj_16[idx] = 0.9;
  o.bbox_16.set([0.5, 0.5, Math.log(2), Math.log(3)], idx * 4);
  o.kps_16.set([0.25, 0.25, 0.75, 0.25, 0.5, 0.5, 0.25, 0.75, 0.75, 0.75], idx * 10);
  const [f, ...rest] = decode(o, 64, 64);
  assert.equal(rest.length, 0);
  assert.ok(Math.abs(f.score - 0.9) < 1e-6);
  const [x, y, w, h] = f.box;
  assert.ok(Math.abs(w - 32) < 1e-4 && Math.abs(h - 48) < 1e-4);
  assert.ok(Math.abs(x + w / 2 - 40) < 1e-4 && Math.abs(y + h / 2 - 24) < 1e-4);
  assert.deepEqual(f.landmarks[0], [36, 20]);
  assert.deepEqual(f.landmarks[4], [44, 28]);
});

test("nms drops overlaps and boxes nested inside a better one", () => {
  const faces = [
    { score: 0.9, box: [0, 0, 100, 100] },
    { score: 0.8, box: [5, 5, 100, 100] },
    { score: 0.7, box: [30, 30, 20, 20] }, // inside the first, low IoU
    { score: 0.6, box: [300, 0, 50, 50] },
  ];
  assert.ok(iou(faces[0].box, faces[2].box) < 0.3);
  const kept = nms(faces);
  assert.deepEqual(
    kept.map((f) => f.score),
    [0.9, 0.6],
  );
});

test("the real model loads on the CPU backend and finds no face in a flat image", async () => {
  ort.env.wasm.numThreads = 1;
  const model = new Uint8Array(fs.readFileSync(new URL("../../models/face_detection_yunet_2026may.onnx", import.meta.url)));
  const session = await ort.InferenceSession.create(model, { executionProviders: ["wasm"] });
  const run = async (input, padW, padH) => {
    const out = await session.run({ input: new ort.Tensor("float32", input, [1, 3, padH, padW]) });
    return Object.fromEntries(Object.entries(out).map(([k, v]) => [k, v.data]));
  };
  const faces = await detect(300, 200, (dw, dh) => new Uint8ClampedArray(dw * dh * 4).fill(128), run);
  assert.equal(faces.length, 0);
  await session.release();
});
