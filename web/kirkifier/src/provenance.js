// A plain-text note inside every exported file saying what was done to it.
//
// The same idea as the gallery's own provenance (PNG text chunk, JPEG
// comment), applied to the browser's encoded output just before download.
// It is metadata, so it does not survive a screenshot or a re-encode; the
// page's own label is the part a viewer always sees.

export const NOTE =
  "Edited with the tiny-face warp: a geometric distortion of this photo's own pixels, " +
  "made in the browser. Not an unedited photograph.";

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

export function crc32(bytes) {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** Latin-1 bytes of an ASCII string; anything else becomes '?'. */
function latin1(s) {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    out[i] = c < 256 ? c : 0x3f;
  }
  return out;
}

/** A `tEXt` chunk inserted straight after IHDR. Returns new bytes; the input
 * is returned unchanged if it is not a PNG this can read. */
export function addPngText(png, keyword, text) {
  if (png.length < 33 || png[12] !== 0x49 || png[13] !== 0x48 || png[14] !== 0x44 || png[15] !== 0x52) {
    return png;
  }
  const ihdrEnd = 8 + 4 + 4 + 13 + 4;
  const data = new Uint8Array([...latin1(keyword), 0, ...latin1(text)]);
  const chunk = new Uint8Array(12 + data.length);
  const view = new DataView(chunk.buffer);
  view.setUint32(0, data.length);
  chunk.set(latin1("tEXt"), 4);
  chunk.set(data, 8);
  view.setUint32(8 + data.length, crc32(chunk.subarray(4, 8 + data.length)));
  const out = new Uint8Array(png.length + chunk.length);
  out.set(png.subarray(0, ihdrEnd), 0);
  out.set(chunk, ihdrEnd);
  out.set(png.subarray(ihdrEnd), ihdrEnd + chunk.length);
  return out;
}

/** A COM segment after SOI (and after APP0, which JFIF wants first).
 * Returns the input unchanged if it is not a JPEG. */
export function addJpegComment(jpeg, text) {
  if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8) return jpeg;
  let at = 2;
  if (jpeg[2] === 0xff && jpeg[3] === 0xe0) at = 4 + ((jpeg[4] << 8) | jpeg[5]);
  const body = latin1(text).subarray(0, 65533);
  const seg = new Uint8Array(4 + body.length);
  seg[0] = 0xff;
  seg[1] = 0xfe;
  seg[2] = ((body.length + 2) >> 8) & 0xff;
  seg[3] = (body.length + 2) & 0xff;
  seg.set(body, 4);
  const out = new Uint8Array(jpeg.length + seg.length);
  out.set(jpeg.subarray(0, at), 0);
  out.set(seg, at);
  out.set(jpeg.subarray(at), at + seg.length);
  return out;
}
