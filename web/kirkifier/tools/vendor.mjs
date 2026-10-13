// Copy the ONNX Runtime files the page loads out of node_modules into
// vendor/, and write their hashes next to them.
//
// Self-hosted rather than from a CDN: the runtime is the code that touches
// the photo, so it comes from the same origin as the page and is pinned to
// the exact version in package-lock.json. Only the plain WebAssembly build is
// copied -- no WebGL, WebGPU or WebNN backend is ever shipped.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dist = path.join(root, "node_modules/onnxruntime-web/dist");
const out = path.join(root, "vendor");
const FILES = ["ort.wasm.min.mjs", "ort-wasm-simd-threaded.mjs", "ort-wasm-simd-threaded.wasm"];

const pkg = JSON.parse(fs.readFileSync(path.join(root, "node_modules/onnxruntime-web/package.json"), "utf8"));
fs.mkdirSync(out, { recursive: true });
const manifest = { package: "onnxruntime-web", version: pkg.version, license: pkg.license, files: {} };
for (const f of FILES) {
  const bytes = fs.readFileSync(path.join(dist, f));
  fs.writeFileSync(path.join(out, f), bytes);
  manifest.files[f] = { bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}
fs.copyFileSync(path.join(root, "node_modules/onnxruntime-web/README.md"), path.join(out, "ONNXRUNTIME-README.md"));
fs.writeFileSync(path.join(out, "MANIFEST.json"), JSON.stringify(manifest, null, 2) + "\n");
console.log(`vendor/: onnxruntime-web ${pkg.version}, ${FILES.length} files`);
