// What a file is and how big it will be once decoded, from its first bytes.
//
// Read before anything decodes it: a 40,000 x 40,000 PNG is a few kilobytes
// on disk and 6.4 GB as pixels, and `createImageBitmap` would try. The type
// comes from the bytes, never from the file name or the browser's guess.

/** Above this many pixels a file is refused outright. */
export const REFUSE_PIXELS = 100e6;

const u16be = (b, i) => (b[i] << 8) | b[i + 1];
const u32be = (b, i) => ((b[i] << 24) >>> 0) + (b[i + 1] << 16) + (b[i + 2] << 8) + b[i + 3];
const u16le = (b, i) => b[i] | (b[i + 1] << 8);
const u24le = (b, i) => b[i] | (b[i + 1] << 8) | (b[i + 2] << 16);
const ascii = (b, i, n) => String.fromCharCode(...b.subarray(i, i + n));

/**
 * `{ type, width, height }` for a PNG, JPEG or WebP; `{ type: "gif" }` for a
 * GIF (recognised so it can be refused by name); `null` for anything else or
 * a header too short or broken to read.
 */
export function sniff(bytes) {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (b.length >= 24 && b[0] === 0x89 && ascii(b, 1, 3) === "PNG" && ascii(b, 12, 4) === "IHDR") {
    return { type: "png", width: u32be(b, 16), height: u32be(b, 20) };
  }
  if (b.length >= 6 && (ascii(b, 0, 6) === "GIF87a" || ascii(b, 0, 6) === "GIF89a")) {
    return { type: "gif" };
  }
  if (b.length >= 30 && ascii(b, 0, 4) === "RIFF" && ascii(b, 8, 4) === "WEBP") {
    const chunk = ascii(b, 12, 4);
    if (chunk === "VP8 ") {
      return { type: "webp", width: u16le(b, 26) & 0x3fff, height: u16le(b, 28) & 0x3fff };
    }
    if (chunk === "VP8L" && b[20] === 0x2f) {
      const bits = b[21] | (b[22] << 8) | (b[23] << 16) | (b[24] << 24);
      return { type: "webp", width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 };
    }
    if (chunk === "VP8X") {
      return { type: "webp", width: u24le(b, 24) + 1, height: u24le(b, 27) + 1 };
    }
    return null;
  }
  if (b.length >= 4 && b[0] === 0xff && b[1] === 0xd8) {
    // Walk the marker segments to the first start-of-frame.
    let i = 2;
    while (i + 9 < b.length) {
      if (b[i] !== 0xff) return null;
      const marker = b[i + 1];
      if (marker === 0xff) {
        i++;
        continue;
      }
      const sof =
        marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
      if (sof) return { type: "jpeg", width: u16be(b, i + 7), height: u16be(b, i + 5) };
      if (marker === 0xd9 || marker === 0xda) return null;
      i += 2 + u16be(b, i + 2);
    }
    return null;
  }
  return null;
}
