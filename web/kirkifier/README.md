# Tiny-face warp

A browser tool that shrinks the face already in a photo: the eyes, nose and
mouth are pulled toward the middle of the face and the skin around them
stretches to fill in, while the head, hair, jaw and everything else stay put.
It is a geometric warp of the photo's own pixels. It does not generate
anything and does not put anyone else's face in, and the page says so.

Everything runs on the visitor's CPU, in the browser: no GPU (WebGL, WebGPU
and WebNN are never used), no server, no account, no upload.

Standalone for now. It is not wired into the gallery app; see "Joining the
gallery" below for how it would be.

## Run it

```bash
cd web/kirkifier
npm ci              # onnxruntime-web 1.30.0, pinned; puppeteer-core for the tests
npm run vendor      # copies the runtime into vendor/ with its hashes
npm run serve       # http://127.0.0.1:3200/  (--isolate for COOP/COEP)
npm test            # unit tests (Node), including the real model on the CPU backend
FIREFOX_PATH=/path/to/firefox npm run e2e   # Chromium and Firefox, end to end
```

`npm run e2e` downloads its test photos from NASA's image library on first
run (public domain, sha256-pinned in `test/e2e/fixtures.json`, never
committed) and writes results, request logs and screenshots to
`test-output/`. Chromium defaults to the Playwright build at
`/opt/pw-browsers`; set `CHROMIUM_PATH` to use another.

## How it works

| Step | Where | What |
|---|---|---|
| Check | page | The first megabyte is sniffed for type and dimensions before anything decodes: only JPEG, PNG and WebP; GIFs refused by name (the warp would drop the animation); over 100 megapixels refused outright; over 40 MB refused. |
| Decode | page | `createImageBitmap` with EXIF orientation applied. Over 24 megapixels is worked on scaled down to 24. |
| Detect | worker | [YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet) (`face_detection_yunet_2026may.onnx`, 230 KB, MIT) on ONNX Runtime Web's WebAssembly backend, at three scales (longest side 640, 320, 160) because YuNet is trained on 10 to 300 px faces; merged with NMS that also drops a box nested inside a better one. Each face gives a box, a score and five landmarks. |
| Warp | worker | Per face, an ellipse is fitted to the landmarks (rotated with the eye line, sized from the eye distance or the eye-to-mouth drop for turned faces). Inside it, a radial map shrinks the core uniformly to the chosen size and eases back to the identity at the edge. The map is strictly monotone with slope 1 at the edge, so it never folds and has no seam; output is supersampled so the shrunk part doesn't alias. Pixels outside every ellipse are untouched. |
| Show | page | Before/after split, per-face checkboxes (score >= 0.75 on by default, weaker ones shown unticked as "unsure"), size and area sliders. |
| Save | page | PNG or JPEG from the canvas, with a text note (PNG `tEXt`, JPEG `COM`) saying it was edited with this warp. |

`src/yunet.js` and `src/warp.js` are pure functions over typed arrays, so the
unit tests run exactly the code the worker runs.

## Measured

On this sandbox: Intel Xeon @ 2.10 GHz, 4 cores, Linux, headless, GPU
switched off in both browsers (`--disable-gpu --disable-3d-apis` in Chromium;
`webgl.disabled`, no WebGPU, no acceleration in Firefox). Nine NASA photos,
1515-1920 px on the long side, three passes each; "warm" excludes the first
pass. One thread (the default deployment).

| | Chromium 141 | Firefox 157 |
|---|---|---|
| Page load to ready (runtime + model, localhost) | 570 ms | 469 ms |
| Face detection, three scales, p50 / p95 | 70 / 86 ms | 76 / 101 ms |
| First detection after load | 106 ms | 135 ms |
| Warp, all chosen faces, p50 / p95 | 47 / 151 ms | 38 / 182 ms |
| Open photo to result shown, p50 / p95 | 231 / 350 ms | 251 / 402 ms |
| Re-warp after a slider move, p50 | 57 ms | 61 ms |
| Pixels changed outside the face ellipses | 0 on every photo | 0 on every photo |
| Requests after load / with a body / CSP violations | 0 / 0 / 0 | 0 / 0 / 0 |

With `--isolate` (COOP/COEP, 4 threads) detection p50 drops to 49 ms in
Chromium and 65 ms in Firefox. Not worth cross-origin isolation for a
gallery whose media is cross-origin; one thread is the default.

What it loads, once (raw / gzip):

| | raw | gzip |
|---|---|---|
| ONNX Runtime wasm (`ort-wasm-simd-threaded.wasm`) | 14.24 MB | 3.65 MB |
| ONNX Runtime JS (two files) | 74 KB | 25 KB |
| YuNet model | 230 KB | 202 KB |
| Page, styles and app code | 50 KB | 19 KB |
| **Total** | **14.59 MB** | **3.90 MB** |

The runtime is 94% of it. A custom ONNX Runtime build with only YuNet's
operators would cut that a lot; not done yet.

Detection per photo (Chromium, first pass): three frontal faces 3/3; six
small faces at night 6/6; crews in orbit with tilted and upside-down heads
6 of 8 and 4 of 6 applied (the rest found but unsure); one large portrait
face 1/1 twice; no face 0. Known misses: a face in a dark cabin under
coloured light (the detector boxed a hatch at 0.67 instead, which is shown
unticked, not applied), and small faces turned away in a control room.
Firefox's canvas downscales slightly differently, so borderline scores
differ by browser (one more orbit face applied in Firefox).

Not measured here: Windows, macOS, Android or iOS, and any real phone.
Those numbers are the next thing to get; nothing above says how a midrange
phone does.

## Privacy, and how it is checked

- The page's Content Security Policy (`connect-src 'self'`, `img-src 'self'
  blob: data:`, no third-party script) means it cannot send anything to
  another origin, whatever its code does.
- So the dev server's own request log is the complete record of what the
  page sent. The e2e suite asserts that after the page has loaded, nothing
  reaches the server at all while photos are opened, warped and exported,
  and that no request in the whole session carries a body or uses a method
  other than GET.
- The runtime and the model are self-hosted and pinned (`vendor/MANIFEST.json`
  records their SHA-256), not fetched from a CDN.
- No analytics, no cookies, no storage. The export note is metadata and does
  not survive a screenshot; the "Edited · tiny-face warp" tag on the page is
  what a viewer of the page always sees.

## Joining the gallery

Not done; the shape it would take:

- A plain Axum route (like `/embed/:id`), say `/tinyface`, serving these
  static files from the image, registered only when the flavor turns it on
  (a new `SITE_*` key), so other sites are unaffected.
- The route's own CSP as above (`'wasm-unsafe-eval'`, `worker-src 'self'`),
  and `application/wasm` for the runtime; the colours already read the
  flavor's `--c-*` variables.
- The runtime and model cached `immutable` under hashed names, like `/pkg/`.
- A link to it from the upload page; an upload of a result goes through the
  normal upload flow, and only if the person chooses to.

## Files

```
index.html, style.css      the page
src/app.js                 page logic: picking, checks, compare, export
src/worker.js              runtime, detection and warp, off the main thread
src/yunet.js               detector pre/post-processing (pure)
src/warp.js                the warp (pure)
src/sniff.js               type and size from the file's first bytes (pure)
src/provenance.js          the export note (pure)
models/                    YuNet ONNX (MIT, see models/README.md)
tools/vendor.mjs           copies the pinned runtime into vendor/
tools/serve.mjs            dev/test server with CSP and a request log
test/unit/                 node --test
test/e2e/                  Chromium + Firefox, via puppeteer-core
```
