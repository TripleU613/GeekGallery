import { test } from "node:test";
import assert from "node:assert/strict";

import { sniff } from "../../src/sniff.js";
import { NOTE, addJpegComment, addPngText, crc32 } from "../../src/provenance.js";

function pngHead(w, h) {
  const b = new Uint8Array(33);
  b.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  new DataView(b.buffer).setUint32(16, w);
  new DataView(b.buffer).setUint32(20, h);
  return b;
}

test("a PNG's size comes from IHDR without decoding it", () => {
  assert.deepEqual(sniff(pngHead(40000, 30000)), { type: "png", width: 40000, height: 30000 });
});

test("a JPEG's size comes from its first frame header, past APP segments", () => {
  const app1 = [0xff, 0xe1, 0x00, 0x08, 1, 2, 3, 4, 5, 6];
  const sof0 = [0xff, 0xc0, 0x00, 0x11, 8, 0x01, 0x2c, 0x02, 0x58, 3];
  const b = new Uint8Array([0xff, 0xd8, ...app1, ...sof0, ...new Array(20).fill(0)]);
  assert.deepEqual(sniff(b), { type: "jpeg", width: 600, height: 300 });
});

test("WebP sizes come from VP8, VP8L and VP8X headers", () => {
  const riff = (chunk, body) => {
    const b = new Uint8Array(40);
    b.set([...Buffer.from("RIFF"), 0, 0, 0, 0, ...Buffer.from("WEBP"), ...Buffer.from(chunk)]);
    b.set(body, 20);
    return b;
  };
  // VP8X: canvas width-1 and height-1 as 24-bit little endian at 24 and 27.
  const vp8x = riff("VP8X", [0, 0, 0, 0, 0x1f, 0x03, 0, 0xdf, 0x01, 0]);
  assert.deepEqual(sniff(vp8x), { type: "webp", width: 800, height: 480 });
  // VP8 (lossy): 14-bit sizes at 26 and 28.
  const vp8 = riff("VP8 ", [0, 0, 0, 0, 0, 0, 0x20, 0x03, 0xe0, 0x01]);
  assert.deepEqual(sniff(vp8), { type: "webp", width: 800, height: 480 });
  // VP8L: signature 0x2f, then width-1 and height-1 packed in 14 bits each.
  const bits = 799 | (479 << 14);
  const vp8l = riff("VP8L", [0x2f, bits & 255, (bits >> 8) & 255, (bits >> 16) & 255, (bits >>> 24) & 255]);
  assert.deepEqual(sniff(vp8l), { type: "webp", width: 800, height: 480 });
});

test("GIFs are recognised so they can be refused by name; other bytes are not images", () => {
  assert.deepEqual(sniff(Buffer.from("GIF89a\x01\x00\x01\x00", "latin1")), { type: "gif" });
  assert.equal(sniff(Buffer.from("not an image at all")), null);
  assert.equal(sniff(new Uint8Array([0xff, 0xd8, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0])), null);
});

test("crc32 matches the standard check value", () => {
  assert.equal(crc32(Buffer.from("123456789")), 0xcbf43926);
});

test("the PNG note is a valid tEXt chunk straight after IHDR", () => {
  const png = new Uint8Array([...pngHead(1, 1), 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82]);
  const out = addPngText(png, "Comment", NOTE);
  const view = new DataView(out.buffer);
  const len = view.getUint32(33);
  assert.equal(Buffer.from(out.subarray(37, 41)).toString("latin1"), "tEXt");
  const data = out.subarray(41, 41 + len);
  assert.equal(Buffer.from(data).toString("latin1"), `Comment\0${NOTE}`);
  assert.equal(view.getUint32(41 + len), crc32(out.subarray(37, 41 + len)));
  assert.deepEqual(out.subarray(41 + len + 4), png.subarray(33));
});

test("the JPEG note is a COM segment after JFIF's APP0", () => {
  const app0 = [0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
  const jpeg = new Uint8Array([0xff, 0xd8, ...app0, 0xff, 0xd9]);
  const out = addJpegComment(jpeg, "hello");
  assert.deepEqual([...out.subarray(0, 8)], [0xff, 0xd8, ...app0]);
  assert.deepEqual([...out.subarray(8, 12)], [0xff, 0xfe, 0x00, 0x07]);
  assert.equal(Buffer.from(out.subarray(12, 17)).toString("latin1"), "hello");
  assert.deepEqual([...out.subarray(17)], [0xff, 0xd9]);
});

test("files that are not PNG or JPEG come back unchanged", () => {
  const junk = new Uint8Array([1, 2, 3]);
  assert.equal(addPngText(junk, "Comment", "x"), junk);
  assert.equal(addJpegComment(junk, "x"), junk);
});
