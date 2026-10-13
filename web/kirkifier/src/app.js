// The page: pick a photo, hand it to the worker, show the result, save it.
//
// The photo goes from the file picker to `createImageBitmap` to the worker
// and back to a canvas. No step of that sends it anywhere: the only network
// requests this page makes are for its own files, and its Content Security
// Policy would refuse anything else.

import { REFUSE_PIXELS, sniff } from "./sniff.js";
import { NOTE, addJpegComment, addPngText } from "./provenance.js";
import { DEFAULT_SIZE } from "./warp.js";
import { DEFAULT_ON } from "./yunet.js";

const MAX_BYTES = 40 * 1024 * 1024;
/** Bigger photos are worked on scaled down to this many pixels, which keeps
 * a full copy plus a warped copy inside a phone's memory. */
const WORK_PIXELS = 24e6;

const $ = (id) => document.getElementById(id);
const ui = {
  file: $("file"),
  pick: $("pick"),
  status: $("status"),
  result: $("result"),
  stage: $("stage"),
  before: $("before"),
  after: $("after"),
  boxes: $("boxes"),
  compare: $("compare"),
  size: $("size"),
  sizeOut: $("size-out"),
  reach: $("reach"),
  reachOut: $("reach-out"),
  faceList: $("face-list"),
  savePng: $("save-png"),
  saveJpg: $("save-jpg"),
  again: $("again"),
  metrics: $("metrics"),
};

const metrics = {};
function record(key, value) {
  metrics[key] = value;
  ui.metrics.value = JSON.stringify(metrics);
}

function setStage(stage, message = "", isError = false) {
  document.body.dataset.stage = stage;
  ui.status.textContent = message;
  ui.status.classList.toggle("error", isError);
}

// -- the worker ------------------------------------------------------------

let worker = null;
let ready = null;
let busy = false;
let waiting = null; // resolve for the reply in flight
let warpSeq = 0;
let warpDirty = false;

function startWorker() {
  worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  ready = new Promise((resolve, reject) => {
    worker.onmessage = (e) => {
      const msg = e.data;
      if (msg.type === "ready") {
        record("modelLoadMs", Math.round(msg.ms));
        record("threads", msg.threads);
        resolve();
      } else if (msg.type === "error" && msg.stage === "init") {
        reject(new Error(msg.message));
      } else if (waiting) {
        const w = waiting;
        waiting = null;
        w(msg);
      }
    };
    worker.onerror = (e) => {
      e.preventDefault();
      const err = new Error(e.message || "the image worker stopped");
      if (waiting) {
        const w = waiting;
        waiting = null;
        w({ type: "error", message: err.message });
      } else reject(err);
    };
  });
  worker.postMessage({ type: "init" });
  return ready;
}

function ask(msg, transfer = []) {
  return new Promise((resolve) => {
    waiting = resolve;
    busy = true;
    worker.postMessage(msg, transfer);
  }).finally(() => {
    busy = false;
  });
}

// -- state -------------------------------------------------------------------

let faces = [];
let enabled = [];
let fileName = "photo";

function percent(v) {
  return `${Math.round(v)}%`;
}

ui.size.value = String(Math.round(DEFAULT_SIZE * 100));
ui.sizeOut.value = percent(ui.size.value);
ui.reachOut.value = percent(ui.reach.value);

// -- opening a photo ---------------------------------------------------------

async function decode(file, info) {
  let bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  const px = bitmap.width * bitmap.height;
  if (px > WORK_PIXELS) {
    const k = Math.sqrt(WORK_PIXELS / px);
    const w = Math.round(bitmap.width * k);
    const h = Math.round(bitmap.height * k);
    // Resize options where the browser has them, a canvas where it doesn't.
    let small = await createImageBitmap(bitmap, { resizeWidth: w, resizeHeight: h, resizeQuality: "high" });
    if (small.width !== w || small.height !== h) {
      small.close();
      const c = new OffscreenCanvas(w, h);
      c.getContext("2d").drawImage(bitmap, 0, 0, w, h);
      small = await createImageBitmap(c);
    }
    bitmap.close();
    bitmap = small;
    record("scaledFrom", `${info.width}x${info.height}`);
  }
  return bitmap;
}

async function open(file) {
  if (!file) return;
  if (busy) restartWorker();
  record("file", { bytes: file.size });
  if (file.size > MAX_BYTES) {
    return setStage("refused", "That file is over 40 MB. Try a smaller copy of the photo.", true);
  }
  const head = new Uint8Array(await file.slice(0, 1 << 20).arrayBuffer());
  const info = sniff(head);
  if (info?.type === "gif") {
    return setStage("refused", "GIFs aren't supported: the warp would lose the animation. Save a frame as PNG first.", true);
  }
  if (!info) return setStage("refused", "That doesn't look like a JPEG, PNG or WebP image.", true);
  if (info.width * info.height > REFUSE_PIXELS) {
    return setStage("refused", `That photo is ${info.width}×${info.height}, too large to open safely here.`, true);
  }
  fileName = (file.name || "photo").replace(/\.[^.]+$/, "") || "photo";

  setStage("decoding", "Reading your photo…");
  let bitmap;
  try {
    bitmap = await decode(file, info);
  } catch {
    return setStage("refused", "That image couldn't be read. It may be damaged or in a format this browser can't open.", true);
  }
  const { width, height } = bitmap;
  record("size", `${width}x${height}`);
  for (const c of [ui.before, ui.after]) {
    c.width = width;
    c.height = height;
    c.getContext("2d").drawImage(bitmap, 0, 0);
  }
  ui.boxes.setAttribute("viewBox", `0 0 ${width} ${height}`);
  ui.boxes.replaceChildren();
  ui.faceList.replaceChildren();
  ui.pick.hidden = true;
  ui.result.hidden = false;

  try {
    setStage("detecting", ready ? "Finding faces…" : "Loading the face finder…");
    await ready;
    setStage("detecting", "Finding faces…");
    const reply = await ask({ type: "open", bitmap }, [bitmap]);
    if (reply.type === "error") throw new Error(reply.message);
    record("detectMs", Math.round(reply.ms));
    faces = reply.faces;
    enabled = faces.map((f) => f.score >= DEFAULT_ON);
    record("faces", faces.length);
    // For the browser tests' pixel check; the page itself never sends this.
    record("detections", faces.map((f) => ({ score: f.score, landmarks: f.landmarks, on: f.score >= DEFAULT_ON })));
  } catch (err) {
    return setStage("error", `Something went wrong finding faces: ${err.message}`, true);
  }
  drawFaces();
  if (!faces.length) {
    setStage("result", "No face found in this photo, so there's nothing to warp. Faces that are tiny, far turned away or in deep shadow can be missed.");
  } else if (!enabled.some(Boolean)) {
    setStage("result", "Only unsure detections in this one. Tick a face below if it's right.");
  } else {
    setStage("result", "");
  }
  await requestWarp();
}

// -- faces -----------------------------------------------------------------

const SVG = "http://www.w3.org/2000/svg";

function drawFaces() {
  ui.boxes.replaceChildren();
  ui.faceList.replaceChildren();
  faces.forEach((f, i) => {
    const [x, y, w, h] = f.box;
    const rect = document.createElementNS(SVG, "rect");
    rect.setAttribute("x", x);
    rect.setAttribute("y", y);
    rect.setAttribute("width", w);
    rect.setAttribute("height", h);
    rect.setAttribute("rx", Math.min(w, h) * 0.08);
    rect.classList.toggle("off", !enabled[i]);
    const label = document.createElementNS(SVG, "text");
    label.setAttribute("x", x + 4);
    label.setAttribute("y", y - 6);
    label.setAttribute("font-size", Math.max(14, h * 0.18));
    label.textContent = String(i + 1);
    ui.boxes.append(rect, label);

    const item = document.createElement("label");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = enabled[i];
    box.addEventListener("change", () => {
      enabled[i] = box.checked;
      rect.classList.toggle("off", !box.checked);
      requestWarp();
    });
    const text = document.createElement("span");
    text.textContent = `Face ${i + 1}`;
    const score = document.createElement("span");
    score.className = f.score < DEFAULT_ON ? "weak" : "";
    score.textContent = f.score < DEFAULT_ON ? "unsure" : `${Math.round(f.score * 100)}%`;
    item.append(box, text, score);
    ui.faceList.append(item);
  });
  if (!faces.length) {
    const none = document.createElement("p");
    none.className = "face-note";
    none.textContent = "None found.";
    ui.faceList.append(none);
  }
}

// -- warping -----------------------------------------------------------------

/** Latest settings win: while one warp runs, further changes only mark the
 * result stale, and one more warp runs when it finishes. */
async function requestWarp() {
  if (busy) {
    warpDirty = true;
    return;
  }
  const id = ++warpSeq;
  const chosen = faces.filter((_, i) => enabled[i]);
  const reply = await ask({
    type: "warp",
    id,
    size: Number(ui.size.value) / 100,
    reach: Number(ui.reach.value) / 100,
    faces: chosen,
  });
  if (reply.type === "error") {
    setStage("error", `Something went wrong warping: ${reply.message}`, true);
    return;
  }
  if (reply.id === warpSeq) {
    const img = new ImageData(new Uint8ClampedArray(reply.pixels.buffer), reply.width, reply.height);
    ui.after.getContext("2d").putImageData(img, 0, 0);
    record("warpMs", Math.round(reply.ms));
    record("warpedFaces", chosen.length);
    document.body.dataset.warps = String(id);
  }
  if (warpDirty) {
    warpDirty = false;
    await requestWarp();
  }
}

// -- saving ------------------------------------------------------------------

async function save(type) {
  const blob = await new Promise((r) => ui.after.toBlob(r, type, 0.92));
  if (!blob) return setStage("result", "This browser couldn't encode the image.", true);
  let bytes = new Uint8Array(await blob.arrayBuffer());
  bytes = type === "image/png" ? addPngText(bytes, "Comment", NOTE) : addJpegComment(bytes, NOTE);
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = `${fileName}-tiny-face.${type === "image/png" ? "png" : "jpg"}`;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

// -- wiring ------------------------------------------------------------------

function restartWorker() {
  worker?.terminate();
  if (waiting) {
    const w = waiting;
    waiting = null;
    w({ type: "error", message: "cancelled" });
  }
  busy = false;
  warpDirty = false;
  startWorker().catch((err) => setStage("error", `The face finder couldn't start: ${err.message}`, true));
}

function reset() {
  if (busy) restartWorker();
  faces = [];
  enabled = [];
  ui.result.hidden = true;
  ui.pick.hidden = false;
  ui.file.value = "";
  setStage("pick", "");
}

ui.file.addEventListener("change", () => open(ui.file.files[0]));
ui.compare.addEventListener("input", () => ui.stage.style.setProperty("--split", `${ui.compare.value}%`));
ui.size.addEventListener("input", () => {
  ui.sizeOut.value = percent(ui.size.value);
  requestWarp();
});
ui.reach.addEventListener("input", () => {
  ui.reachOut.value = percent(ui.reach.value);
  requestWarp();
});
ui.savePng.addEventListener("click", () => save("image/png"));
ui.saveJpg.addEventListener("click", () => save("image/jpeg"));
ui.again.addEventListener("click", reset);

document.addEventListener("dragover", (e) => {
  if ([...(e.dataTransfer?.types || [])].includes("Files")) {
    e.preventDefault();
    document.body.classList.add("dragging");
  }
});
document.addEventListener("dragleave", (e) => {
  if (e.relatedTarget === null) document.body.classList.remove("dragging");
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  document.body.classList.remove("dragging");
  const file = [...(e.dataTransfer?.files || [])].find((f) => f.type.startsWith("image/")) || e.dataTransfer?.files?.[0];
  open(file);
});
window.addEventListener("paste", (e) => {
  const file = [...(e.clipboardData?.files || [])].find((f) => f.type.startsWith("image/"));
  if (file) open(file);
});

// Start loading the runtime and the model as soon as the page is up: this
// page exists to use them, and the photo usually takes longer to choose.
setStage("loading", "Loading the face finder…");
startWorker()
  .then(() => {
    if (document.body.dataset.stage === "loading") setStage("pick", "");
  })
  .catch((err) => setStage("error", `The face finder couldn't start: ${err.message}`, true));
