// Everything heavy, off the page's main thread: the ONNX runtime, face
// detection, and the warp. The page talks to it in messages:
//
//   -> { type: "init" }                                  <- { type: "ready", ... }
//   -> { type: "open", bitmap }                           <- { type: "faces", ... }
//   -> { type: "warp", id, size, reach, faces }           <- { type: "warped", id, ... }
//   any failure                                           <- { type: "error", message }
//
// The photo arrives as an ImageBitmap the page already decoded and leaves as
// pixels for the page's canvas. Nothing here can reach the network except the
// two fetches `init` makes for the runtime and the model, both same-origin.

import * as ort from "../vendor/ort.wasm.min.mjs";
import { detect } from "./yunet.js";
import { faceFrame, warpAll } from "./warp.js";

const MODEL_URL = new URL("../models/face_detection_yunet_2026may.onnx", import.meta.url);

let session = null;
let original = null; // { width, height, pixels } of the open photo

function post(msg, transfer = []) {
  self.postMessage(msg, transfer);
}

async function init() {
  const t0 = performance.now();
  // CPU only: the plain WebAssembly backend, never WebGL or WebGPU. Threads
  // only when the page is cross-origin isolated (SharedArrayBuffer needs it);
  // one thread otherwise, which is the default deployment.
  ort.env.wasm.wasmPaths = new URL("../vendor/", import.meta.url).href;
  ort.env.wasm.proxy = false;
  const threads = self.crossOriginIsolated ? Math.max(1, Math.min(4, navigator.hardwareConcurrency || 1)) : 1;
  ort.env.wasm.numThreads = threads;
  const res = await fetch(MODEL_URL);
  if (!res.ok) throw new Error(`the face model did not load (${res.status})`);
  const model = new Uint8Array(await res.arrayBuffer());
  session = await ort.InferenceSession.create(model, {
    executionProviders: ["wasm"],
    graphOptimizationLevel: "all",
  });
  post({ type: "ready", ms: performance.now() - t0, threads, isolated: self.crossOriginIsolated });
}

async function run(input, padW, padH) {
  const out = await session.run({ input: new ort.Tensor("float32", input, [1, 3, padH, padW]) });
  return Object.fromEntries(Object.entries(out).map(([k, v]) => [k, v.data]));
}

async function open(bitmap) {
  const { width, height } = bitmap;
  const full = new OffscreenCanvas(width, height);
  const fctx = full.getContext("2d", { willReadFrequently: true });
  fctx.drawImage(bitmap, 0, 0);
  original = { width, height, pixels: fctx.getImageData(0, 0, width, height).data };

  const t0 = performance.now();
  const pixelsAt = (w, h) => {
    const c = new OffscreenCanvas(w, h);
    const ctx = c.getContext("2d", { willReadFrequently: true });
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(bitmap, 0, 0, w, h);
    return ctx.getImageData(0, 0, w, h).data;
  };
  const faces = await detect(width, height, pixelsAt, run);
  bitmap.close();
  post({ type: "faces", faces, ms: performance.now() - t0 });
}

function warp({ id, size, reach, faces }) {
  if (!original) throw new Error("no photo is open");
  const t0 = performance.now();
  const { width, height } = original;
  const pixels = original.pixels.slice();
  const frames = faces.map((f) => faceFrame(f.landmarks, reach));
  warpAll(pixels, width, height, frames, size);
  post({ type: "warped", id, width, height, pixels, ms: performance.now() - t0 }, [pixels.buffer]);
}

self.onmessage = async (e) => {
  const msg = e.data;
  try {
    if (msg.type === "init") await init();
    else if (msg.type === "open") await open(msg.bitmap);
    else if (msg.type === "warp") warp(msg);
  } catch (err) {
    const message =
      err instanceof RangeError
        ? "this device ran out of memory for a photo this size; try a smaller one"
        : String(err?.message || err);
    post({ type: "error", stage: msg.type, message });
  }
};
