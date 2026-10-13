// A static server for local use and the browser tests. Sends the page's
// Content Security Policy as a header too (a worker takes its policy from its
// own response), the right types for .wasm and .onnx, and no caching.
//
//   node tools/serve.mjs [--port 3200] [--isolate]
//
// --isolate adds COOP/COEP so the page is cross-origin isolated and the
// runtime may use threads; without it the runtime runs on one thread.

import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const port = Number(args[args.indexOf("--port") + 1]) || Number(process.env.PORT) || 3200;
const isolate = args.includes("--isolate");
// --log FILE: one JSON line per request (method, path, request body size,
// response size). The browser tests read it to prove what left the page.
const logFile = args.includes("--log") ? args[args.indexOf("--log") + 1] : null;
function log(req, status, bytes, bodyBytes) {
  if (!logFile) return;
  const line = { t: Date.now(), method: req.method, path: req.url, status, bytes, bodyBytes };
  fs.appendFileSync(logFile, JSON.stringify(line) + "\n");
}

export const CSP =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; " +
  "img-src 'self' blob: data:; style-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".onnx": "application/octet-stream",
  ".jpg": "image/jpeg",
  ".png": "image/png",
  ".webp": "image/webp",
};

const server = http.createServer((req, res) => {
  let bodyBytes = 0;
  req.on("data", (chunk) => (bodyBytes += chunk.length));
  req.on("end", () => handle(req, res, bodyBytes));
});

function handle(req, res, bodyBytes) {
  const url = new URL(req.url, "http://x");
  let file = path.normalize(path.join(root, decodeURIComponent(url.pathname)));
  if (!file.startsWith(root + path.sep) && file !== root) {
    log(req, 403, 0, bodyBytes);
    res.writeHead(403).end();
    return;
  }
  if (file === root || url.pathname.endsWith("/")) file = path.join(file, "index.html");
  const type = TYPES[path.extname(file)];
  if (req.method !== "GET" && req.method !== "HEAD") {
    log(req, 405, 0, bodyBytes);
    res.writeHead(405).end();
    return;
  }
  fs.readFile(file, (err, body) => {
    if (err || !type) {
      log(req, 404, 0, bodyBytes);
      res.writeHead(404, { "Cache-Control": "no-store" }).end();
      return;
    }
    log(req, 200, body.length, bodyBytes);
    const headers = {
      "Content-Type": type,
      "Content-Length": body.length,
      "Cache-Control": "no-cache",
      "Content-Security-Policy": CSP,
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "no-referrer",
    };
    if (isolate) {
      headers["Cross-Origin-Opener-Policy"] = "same-origin";
      headers["Cross-Origin-Embedder-Policy"] = "require-corp";
      headers["Cross-Origin-Resource-Policy"] = "same-origin";
    }
    res.writeHead(200, headers);
    res.end(req.method === "HEAD" ? undefined : body);
  });
}

server.listen(port, "127.0.0.1", () => {
  console.log(`tiny-face: http://127.0.0.1:${port}/${isolate ? " (cross-origin isolated)" : ""}`);
});
