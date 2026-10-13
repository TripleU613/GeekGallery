// Face detection with YuNet, as pure functions over typed arrays.
//
// Nothing in here touches the DOM, a canvas or the network, so the same file
// runs in the browser worker and under `node --test`. The caller owns the
// ONNX Runtime session and hands in pixels and raw output tensors.
//
// The model is opencv_zoo's `face_detection_yunet_2026may.onnx` (MIT): the
// 2023mar network re-exported with symbolic height/width, so it runs on any
// input whose sides are multiples of 32 without letterboxing to 640x640.
//
// YuNet was trained on faces of roughly 10 to 300 pixels. A selfie at full
// resolution is far outside that, so `scalesFor` runs the image at several
// sizes and `nms` keeps the best box per face across all of them.

export const STRIDES = [8, 16, 32];

/** Longest sides the image is detected at. 640 finds small faces in group
 * shots; 320 and 160 bring a frame-filling face back under ~300px. */
export const DETECT_SIDES = [640, 320, 160];

/** Below this a candidate is dropped before NMS. Low on purpose: the UI
 * decides what to apply by default (see `DEFAULT_ON`), and showing a weak
 * detection as an unticked box beats silently missing a face. */
export const MIN_SCORE = 0.5;

/** Detections at or above this are applied without asking. */
export const DEFAULT_ON = 0.75;

const NMS_IOU = 0.3;

export function padTo32(n) {
  return Math.max(32, Math.ceil(n / 32) * 32);
}

/** The scaled sizes to run, largest first, skipping repeats. A small image is
 * enlarged to 640 rather than run at its own size: YuNet's smallest anchors
 * are 8px, and a 400px group photo has faces below that. */
export function scalesFor(width, height, sides = DETECT_SIDES) {
  const long = Math.max(width, height);
  const out = [];
  const seen = new Set();
  for (const side of sides) {
    const scale = side / long;
    const w = Math.max(1, Math.round(width * scale));
    const h = Math.max(1, Math.round(height * scale));
    const key = `${w}x${h}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ scale, w, h, padW: padTo32(w), padH: padTo32(h) });
  }
  return out;
}

/** RGBA pixels (canvas order) to the model's input: NCHW float32, BGR,
 * unnormalised 0..255, zero-padded on the right and bottom. */
export function toInput(rgba, w, h, padW, padH) {
  const plane = padW * padH;
  const out = new Float32Array(3 * plane);
  for (let y = 0; y < h; y++) {
    let i = y * w * 4;
    let o = y * padW;
    for (let x = 0; x < w; x++, i += 4, o++) {
      out[o] = rgba[i + 2];
      out[plane + o] = rgba[i + 1];
      out[2 * plane + o] = rgba[i];
    }
  }
  return out;
}

const clamp01 = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);

/**
 * Raw outputs (keyed by name, each a Float32Array) to face candidates in the
 * coordinates of the padded input. Same arithmetic as OpenCV's
 * FaceDetectorYN for this model: per anchor, score = sqrt(cls * obj), box
 * centre offset in cells, size as exp() in strides, five landmarks offset
 * from the cell's corner.
 *
 * Landmarks are [rightEye, leftEye, noseTip, rightMouth, leftMouth] in the
 * subject's own sense, i.e. rightEye is usually on the image's left.
 */
export function decode(outputs, padW, padH, minScore = MIN_SCORE) {
  const faces = [];
  for (const s of STRIDES) {
    const cols = padW / s;
    const rows = padH / s;
    const cls = outputs[`cls_${s}`];
    const obj = outputs[`obj_${s}`];
    const box = outputs[`bbox_${s}`];
    const kps = outputs[`kps_${s}`];
    const n = rows * cols;
    for (let idx = 0; idx < n; idx++) {
      const score = Math.sqrt(clamp01(cls[idx]) * clamp01(obj[idx]));
      if (score < minScore) continue;
      const r = Math.floor(idx / cols);
      const c = idx % cols;
      const cx = (c + box[idx * 4]) * s;
      const cy = (r + box[idx * 4 + 1]) * s;
      const w = Math.exp(box[idx * 4 + 2]) * s;
      const h = Math.exp(box[idx * 4 + 3]) * s;
      const landmarks = [];
      for (let k = 0; k < 5; k++) {
        landmarks.push([(kps[idx * 10 + 2 * k] + c) * s, (kps[idx * 10 + 2 * k + 1] + r) * s]);
      }
      faces.push({ score, box: [cx - w / 2, cy - h / 2, w, h], landmarks });
    }
  }
  return faces;
}

/** A candidate in scaled-input pixels back to source-image pixels. */
export function unscale(face, scale) {
  const [x, y, w, h] = face.box;
  return {
    score: face.score,
    box: [x / scale, y / scale, w / scale, h / scale],
    landmarks: face.landmarks.map(([lx, ly]) => [lx / scale, ly / scale]),
  };
}

export function iou(a, b) {
  const [ax, ay, aw, ah] = a;
  const [bx, by, bw, bh] = b;
  const ix = Math.max(0, Math.min(ax + aw, bx + bw) - Math.max(ax, bx));
  const iy = Math.max(0, Math.min(ay + ah, by + bh) - Math.max(ay, by));
  const inter = ix * iy;
  const union = aw * ah + bw * bh - inter;
  return union > 0 ? inter / union : 0;
}

/** How much of the smaller box lies inside the larger one. */
export function containment(a, b) {
  const [ax, ay, aw, ah] = a;
  const [bx, by, bw, bh] = b;
  const ix = Math.max(0, Math.min(ax + aw, bx + bw) - Math.max(ax, bx));
  const iy = Math.max(0, Math.min(ay + ah, by + bh) - Math.max(ay, by));
  const small = Math.min(aw * ah, bw * bh);
  return small > 0 ? (ix * iy) / small : 0;
}

/** Greedy non-maximum suppression, best score first.
 *
 * IoU alone is not enough across scales: the same tilted face found at 640
 * and at 160 can come back as a small box inside a large one, whose IoU is
 * low because the areas differ. A box mostly inside a kept one is the same
 * face. */
export function nms(faces, threshold = NMS_IOU) {
  const sorted = [...faces].sort((a, b) => b.score - a.score);
  const kept = [];
  for (const f of sorted) {
    if (kept.every((k) => iou(k.box, f.box) <= threshold && containment(k.box, f.box) < 0.7)) {
      kept.push(f);
    }
  }
  return kept;
}

/**
 * The whole detector. `run(input, padW, padH)` executes the model and resolves
 * to its outputs by name; `pixelsAt(w, h)` returns RGBA for the image scaled
 * to w x h. Both are supplied by the environment (worker or test), which is
 * what keeps this file free of browser APIs.
 */
export async function detect(width, height, pixelsAt, run) {
  const all = [];
  for (const { scale, w, h, padW, padH } of scalesFor(width, height)) {
    const input = toInput(pixelsAt(w, h), w, h, padW, padH);
    const outputs = await run(input, padW, padH);
    for (const f of decode(outputs, padW, padH)) all.push(unscale(f, scale));
  }
  // Left to right, so "face 1" in the UI is the leftmost one.
  return nms(all).sort((a, b) => a.box[0] - b.box[0]);
}
