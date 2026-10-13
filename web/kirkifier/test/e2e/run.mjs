// The page in real browsers, on the CPU, end to end.
//
//   FIREFOX_PATH=/path/to/firefox node test/e2e/run.mjs [--browsers chromium,firefox] [--isolate] [--runs 3]
//
// For each browser: load the page cold, open every fixture photo (several
// times, for medians), move the sliders, export PNG and JPEG, and feed it
// the files it must refuse. Along the way it checks that
//
//   - no pixel outside a face's ellipse changes (the warp's "mask"),
//   - the exports carry the edited-with note,
//   - nothing reaches the server after the page has loaded, and nothing is
//     ever sent with a body -- read from the server's own request log, which
//     is complete because the page's CSP only lets it talk to that server,
//   - no CSP violation, page error or console error happens.
//
// GPU paths are switched off in both browsers, so a pass also shows nothing
// needs WebGL or WebGPU. Results go to test-output/ and a summary to stdout.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const outDir = path.join(root, "test-output");
const fixDir = path.join(outDir, "fixtures");
const args = process.argv.slice(2);
const opt = (name, dflt) => (args.includes(name) ? args[args.indexOf(name) + 1] : dflt);
const browsers = opt("--browsers", "chromium,firefox").split(",");
const isolate = args.includes("--isolate");
const runs = Number(opt("--runs", "3"));
const fixtures = JSON.parse(fs.readFileSync(path.join(root, "test/e2e/fixtures.json"), "utf8")).images;

const failures = [];
function check(ok, what) {
  if (!ok) failures.push(what);
  return ok;
}

// -- fixtures ----------------------------------------------------------------

async function fetchFixtures() {
  fs.mkdirSync(fixDir, { recursive: true });
  for (const f of fixtures) {
    const file = path.join(fixDir, `${f.id}.jpg`);
    const ok = () => createHash("sha256").update(fs.readFileSync(file)).digest("hex") === f.sha256;
    if (fs.existsSync(file) && ok()) continue;
    const asset = await (await fetch(`https://images-api.nasa.gov/asset/${f.id}`)).json();
    const href = asset.collection.items.map((i) => i.href).find((h) => h.endsWith("~large.jpg"));
    const res = await fetch(href.replace("http://", "https://"));
    fs.writeFileSync(file, Buffer.from(await res.arrayBuffer()));
    if (!ok()) throw new Error(`fixture ${f.id} does not match its sha256`);
  }
}

// -- browsers ------------------------------------------------------------------

function launchOptions(name) {
  if (name === "chromium") {
    const sandbox = process.getuid?.() === 0 ? ["--no-sandbox"] : [];
    return {
      browser: "chrome",
      executablePath: process.env.CHROMIUM_PATH || "/opt/pw-browsers/chromium-1194/chrome-linux/chrome",
      args: ["--disable-gpu", "--disable-3d-apis", "--disable-features=WebGPU,Vulkan", ...sandbox],
    };
  }
  if (name === "firefox") {
    if (!process.env.FIREFOX_PATH) throw new Error("set FIREFOX_PATH to a Firefox binary");
    return {
      browser: "firefox",
      executablePath: process.env.FIREFOX_PATH,
      extraPrefsFirefox: {
        "webgl.disabled": true,
        "dom.webgpu.enabled": false,
        "layers.acceleration.disabled": true,
        "gfx.webrender.software": true,
      },
    };
  }
  throw new Error(`unknown browser ${name}`);
}

// Installed before any page script: CSP violations, export blobs, and a
// download click that does nothing (the test reads the blob instead).
const PRELOAD = `
  window.__csp = [];
  document.addEventListener("securitypolicyviolation", (e) => window.__csp.push(e.violatedDirective + " " + e.blockedURI));
  window.__blobs = [];
  const create = URL.createObjectURL.bind(URL);
  URL.createObjectURL = (b) => { window.__blobs.push(b); return create(b); };
  const click = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function () { if (!this.download) click.call(this); };
`;

function startServer(port, logFile) {
  const argv = [path.join(root, "tools/serve.mjs"), "--port", String(port), "--log", logFile];
  if (isolate) argv.push("--isolate");
  const child = spawn(process.execPath, argv, { stdio: ["ignore", "pipe", "inherit"] });
  return new Promise((resolve) => child.stdout.once("data", () => resolve(child)));
}

const readLog = (file) =>
  fs.existsSync(file)
    ? fs.readFileSync(file, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
    : [];

/** Put bytes into the page's file input the way a person would. */
async function openBytes(page, name, type, bytes) {
  const before = await page.evaluate(() => Number(document.body.dataset.warps || 0));
  await page.evaluate(
    (name, type, b64) => {
      const bin = atob(b64);
      const arr = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
      const dt = new DataTransfer();
      dt.items.add(new File([arr], name, { type }));
      const input = document.getElementById("file");
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    },
    name,
    type,
    Buffer.from(bytes).toString("base64"),
  );
  return before;
}

async function waitWarp(page, after) {
  await page.waitForFunction(
    (n) => Number(document.body.dataset.warps || 0) > n || ["error", "refused"].includes(document.body.dataset.stage),
    { timeout: 120_000 },
    after,
  );
  return page.evaluate(() => ({
    stage: document.body.dataset.stage,
    status: document.getElementById("status").textContent,
    metrics: JSON.parse(document.getElementById("metrics").value || "{}"),
  }));
}

/** Pixels that differ between the before and after canvases, split into
 * inside and outside the union of the warped faces' ellipses. */
async function leakage(page) {
  return page.evaluate(async () => {
    const { faceFrame } = await import("/src/warp.js");
    const m = JSON.parse(document.getElementById("metrics").value);
    const reach = Number(document.getElementById("reach").value) / 100;
    const frames = m.detections.filter((d) => d.on).map((d) => faceFrame(d.landmarks, reach));
    const b = document.getElementById("before");
    const a = document.getElementById("after");
    const w = b.width;
    const h = b.height;
    const pb = b.getContext("2d").getImageData(0, 0, w, h).data;
    const pa = a.getContext("2d").getImageData(0, 0, w, h).data;
    let inside = 0;
    let outside = 0;
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const i = (y * w + x) * 4;
        if (pb[i] === pa[i] && pb[i + 1] === pa[i + 1] && pb[i + 2] === pa[i + 2] && pb[i + 3] === pa[i + 3]) continue;
        const inAny = frames.some((f) => {
          const dx = x + 0.5 - f.cx;
          const dy = y + 0.5 - f.cy;
          const c = Math.cos(f.angle);
          const s = Math.sin(f.angle);
          return Math.hypot((dx * c + dy * s) / f.a, (-dx * s + dy * c) / f.b) < 1;
        });
        if (inAny) inside++;
        else outside++;
      }
    }
    return { inside, outside };
  });
}

async function setSlider(page, id, value) {
  const before = await page.evaluate(() => Number(document.body.dataset.warps || 0));
  await page.evaluate(
    (id, value) => {
      const el = document.getElementById(id);
      el.value = String(value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    },
    id,
    value,
  );
  return waitWarp(page, before);
}

async function exportCheck(page, button) {
  const n = await page.evaluate(() => window.__blobs.length);
  await page.click(button);
  await page.waitForFunction((n) => window.__blobs.length > n, { timeout: 30_000 }, n);
  return page.evaluate(async () => {
    const blob = window.__blobs[window.__blobs.length - 1];
    const bytes = new Uint8Array(await blob.arrayBuffer());
    const text = new TextDecoder("latin1").decode(bytes.subarray(0, 4096));
    const png = bytes[0] === 0x89 && bytes[1] === 0x50;
    const jpeg = bytes[0] === 0xff && bytes[1] === 0xd8;
    const dims = png ? [new DataView(bytes.buffer).getUint32(16), new DataView(bytes.buffer).getUint32(20)] : null;
    return { type: blob.type, bytes: bytes.length, png, jpeg, dims, note: text.includes("tiny-face warp") };
  });
}

// A JPEG drawn in the page, half red and half blue, stored 200x100 with EXIF
// orientation 6 (rotate 90 clockwise): it must open as 100x200.
const EXIF_JPEG = `
  (async () => {
    const c = document.createElement("canvas");
    c.width = 200; c.height = 100;
    const g = c.getContext("2d");
    g.fillStyle = "#f00"; g.fillRect(0, 0, 100, 100);
    g.fillStyle = "#00f"; g.fillRect(100, 0, 100, 100);
    const jpg = new Uint8Array(await (await new Promise((r) => c.toBlob(r, "image/jpeg", 0.9))).arrayBuffer());
    const tiff = [0x4d,0x4d,0,0x2a,0,0,0,8, 0,1, 0x01,0x12,0,3,0,0,0,1,0,6,0,0, 0,0,0,0];
    const payload = [0x45,0x78,0x69,0x66,0,0, ...tiff];
    const len = payload.length + 2;
    const app1 = [0xff,0xe1,len >> 8,len & 255, ...payload];
    const out = new Uint8Array(jpg.length + app1.length);
    out.set(jpg.subarray(0, 2), 0); out.set(app1, 2); out.set(jpg.subarray(2), 2 + app1.length);
    let s = ""; for (const b of out) s += String.fromCharCode(b);
    return btoa(s);
  })()
`;

function pngHeader(w, h) {
  const b = Buffer.alloc(33);
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(b, 0);
  b.writeUInt32BE(13, 8);
  b.write("IHDR", 12, "latin1");
  b.writeUInt32BE(w, 16);
  b.writeUInt32BE(h, 20);
  b[24] = 8;
  b[25] = 6;
  return b;
}

const pct = (xs, p) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length ? s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] : null;
};

async function runBrowser(name, port) {
  const tag = `${name}${isolate ? "-isolated" : ""}`;
  const logFile = path.join(outDir, `requests-${tag}.jsonl`);
  fs.rmSync(logFile, { force: true });
  const server = await startServer(port, logFile);
  const browser = await puppeteer.launch({ headless: true, ...launchOptions(name) });
  const result = { browser: name, version: await browser.version(), isolate, images: [], refusals: {}, errors: [] };
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 900 });
    page.on("pageerror", (e) => result.errors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") result.errors.push(`console: ${m.text()}`);
    });
    await page.evaluateOnNewDocument(PRELOAD);

    // Cold load: navigation to "the face finder is ready".
    const t0 = Date.now();
    await page.goto(`http://127.0.0.1:${port}/`);
    await page.waitForFunction(() => document.body.dataset.stage !== "loading", { timeout: 120_000 });
    result.coldReadyMs = Date.now() - t0;
    const loadLog = readLog(logFile);
    result.coldBytes = loadLog.reduce((a, l) => a + l.bytes, 0);
    result.coldRequests = loadLog.length;
    result.threads = (await page.evaluate(() => JSON.parse(document.getElementById("metrics").value || "{}"))).threads;

    // Photos.
    for (let r = 0; r < runs; r++) {
      for (const f of fixtures) {
        const bytes = fs.readFileSync(path.join(fixDir, `${f.id}.jpg`));
        const t = Date.now();
        const n = await openBytes(page, `${f.id}.jpg`, "image/jpeg", bytes);
        const res = await waitWarp(page, n);
        const wall = Date.now() - t;
        const m = res.metrics;
        const row = { id: f.id, run: r, size: m.size, faces: m.faces, warped: m.warpedFaces, detectMs: m.detectMs, warpMs: m.warpMs, wallMs: wall };
        check(res.stage === "result", `${name}: ${f.id} ended in stage ${res.stage}: ${res.status}`);
        if (f.expectFaces !== undefined) {
          check(m.warpedFaces === f.expectFaces, `${name}: ${f.id} warped ${m.warpedFaces} face(s), expected ${f.expectFaces}`);
        }
        if (r === 0) {
          const leak = await leakage(page);
          row.changedInside = leak.inside;
          row.changedOutside = leak.outside;
          check(leak.outside === 0, `${name}: ${f.id} changed ${leak.outside} pixel(s) outside the face ellipses`);
          if (m.warpedFaces > 0) check(leak.inside > 0, `${name}: ${f.id} warped faces but changed nothing`);
        }
        result.images.push(row);
      }
    }

    // Sliders on the three-face portrait: warm re-warps only, no re-detect.
    const apollo = fs.readFileSync(path.join(fixDir, "S69-31740.jpg"));
    await waitWarp(page, await openBytes(page, "S69-31740.jpg", "image/jpeg", apollo));
    result.rewarpMs = [];
    for (const v of [40, 70, 30, 95, 55]) result.rewarpMs.push((await setSlider(page, "size", v)).metrics.warpMs);
    const reach = await setSlider(page, "reach", 120);
    const leak = await leakage(page);
    check(leak.outside === 0, `${name}: area 120% changed ${leak.outside} pixel(s) outside the ellipses`);
    result.rewarpMs.push(reach.metrics.warpMs);
    await setSlider(page, "reach", 100);

    // Exports.
    result.exportPng = await exportCheck(page, "#save-png");
    result.exportJpeg = await exportCheck(page, "#save-jpg");
    check(result.exportPng.png && result.exportPng.note, `${name}: PNG export missing or without the note`);
    check(result.exportJpeg.jpeg && result.exportJpeg.note, `${name}: JPEG export missing or without the note`);
    check(result.exportPng.dims?.[0] === 1920 && result.exportPng.dims?.[1] === 1540, `${name}: PNG export is not full size`);

    // EXIF orientation, from a JPEG drawn in the page.
    const exif = Buffer.from(await page.evaluate(EXIF_JPEG), "base64");
    const ex = await waitWarp(page, await openBytes(page, "rotated.jpg", "image/jpeg", exif));
    result.exifSize = ex.metrics.size;
    check(ex.metrics.size === "100x200", `${name}: EXIF orientation 6 opened as ${ex.metrics.size}, expected 100x200`);

    // Files that must be refused before anything decodes them.
    const refusals = {
      gif: [Buffer.from("GIF89a\x01\x00\x01\x00\x00\x00\x00;", "latin1"), "image/gif", "GIFs aren't supported"],
      text: [Buffer.from("not an image at all"), "image/png", "doesn't look like"],
      bomb: [pngHeader(20000, 20000), "image/png", "too large"],
    };
    for (const [key, [bytes, type, expect]] of Object.entries(refusals)) {
      const before = await page.evaluate(() => document.getElementById("status").textContent);
      await openBytes(page, `${key}.bin`, type, bytes);
      await page.waitForFunction((b) => document.getElementById("status").textContent !== b, { timeout: 10_000 }, before);
      const status = await page.evaluate(() => document.getElementById("status").textContent);
      result.refusals[key] = status;
      check(status.includes(expect), `${name}: ${key} file not refused as expected: "${status}"`);
    }

    // What left the page.
    const log = readLog(logFile);
    const afterLoad = log.slice(loadLog.length);
    result.requestsAfterLoad = afterLoad.map((l) => `${l.method} ${l.path}`);
    result.requestsWithBody = log.filter((l) => l.bodyBytes > 0 || l.method !== "GET").map((l) => `${l.method} ${l.path}`);
    result.csp = await page.evaluate(() => window.__csp);
    check(afterLoad.length === 0, `${name}: ${afterLoad.length} request(s) after load: ${result.requestsAfterLoad.join(", ")}`);
    check(result.requestsWithBody.length === 0, `${name}: request(s) with a body: ${result.requestsWithBody.join(", ")}`);
    check(result.csp.length === 0, `${name}: CSP violations: ${result.csp.join(", ")}`);
    check(result.errors.length === 0, `${name}: errors: ${result.errors.join(" | ")}`);

    await page.screenshot({ path: path.join(outDir, `screenshot-${tag}.png`), fullPage: true });
  } finally {
    await browser.close();
    server.kill();
  }

  const warm = result.images.filter((r) => r.run > 0);
  result.summary = {
    detectP50: pct(warm.map((r) => r.detectMs), 50),
    detectP95: pct(warm.map((r) => r.detectMs), 95),
    warpP50: pct(warm.map((r) => r.warpMs), 50),
    warpP95: pct(warm.map((r) => r.warpMs), 95),
    wallP50: pct(warm.map((r) => r.wallMs), 50),
    wallP95: pct(warm.map((r) => r.wallMs), 95),
    firstDetectMs: result.images[0]?.detectMs,
    rewarpP50: pct(result.rewarpMs, 50),
  };
  fs.writeFileSync(path.join(outDir, `e2e-${tag}.json`), JSON.stringify(result, null, 2));
  return result;
}

await fetchFixtures();
fs.mkdirSync(outDir, { recursive: true });
const cpu = os.cpus()[0]?.model;
console.log(`machine: ${cpu}, ${os.cpus().length} cores, ${os.platform()} ${os.release()}`);
let port = 3310;
for (const name of browsers) {
  const r = await runBrowser(name, port++);
  const s = r.summary;
  console.log(
    `\n${r.browser} ${r.version}${r.isolate ? " (isolated)" : ""}: threads=${r.threads}, cold ready ${r.coldReadyMs}ms ` +
      `(${r.coldRequests} requests, ${(r.coldBytes / 1e6).toFixed(2)} MB uncompressed)`,
  );
  console.log(
    `  detect p50 ${s.detectP50}ms p95 ${s.detectP95}ms (first ${s.firstDetectMs}ms) | warp p50 ${s.warpP50}ms p95 ${s.warpP95}ms | ` +
      `open-to-result p50 ${s.wallP50}ms p95 ${s.wallP95}ms | slider re-warp p50 ${s.rewarpP50}ms`,
  );
  for (const row of r.images.filter((x) => x.run === 0)) {
    console.log(
      `  ${row.id.padEnd(20)} ${String(row.size).padEnd(10)} faces ${row.faces} warped ${row.warped}  ` +
        `detect ${row.detectMs}ms warp ${row.warpMs}ms  changed in/out ${row.changedInside}/${row.changedOutside}`,
    );
  }
  console.log(`  requests after load: ${r.requestsAfterLoad.length}; with a body: ${r.requestsWithBody.length}; CSP violations: ${r.csp.length}`);
}
if (failures.length) {
  console.log(`\nFAILED (${failures.length}):\n  ${failures.join("\n  ")}`);
  process.exit(1);
}
console.log("\nall checks passed");
