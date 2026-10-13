// The tiny-face warp: shrink the features of a face toward its middle and
// let the surrounding skin stretch in to fill the gap, leaving the head,
// hair, jaw and everything around the face where they were.
//
// It is a geometric effect on the photo's own pixels. Nothing is generated
// and no other face is drawn in; the UI says so in as many words.
//
// Pure functions over RGBA typed arrays, so the worker and `node --test`
// run the same code.
//
// The shape of it, in one face's own frame (x along the eye line, y down the
// face, both divided by the ellipse's semi-axes so the warp region is the
// unit disc):
//
//   forward   F(r) = r * sigma(r)       where a source pixel at radius r ends up
//   sigma(r)  = s                       inside the core (r <= r0): a uniform shrink
//             = s + (1-s) * smooth(.)   across the band, easing to 1 at r = 1
//
// F is strictly increasing (F' = sigma + r * sigma' >= s > 0), F(1) = 1 and
// F'(1) = 1, so the warp is one-to-one, never folds the image over itself,
// and meets the untouched photo at the ellipse without a seam or a kink.
// Rendering needs the inverse (for each output pixel, where to read from),
// which is tabulated once per strength in `inverseTable`.

/** Default share of its size a face keeps. */
export const DEFAULT_SIZE = 0.55;
/** The warp is monotone for any size in (0, 1]; below this it is mush. */
export const MIN_SIZE = 0.25;

/** The ellipse, in units of the face's scale (see `faceFrame`). Chosen by
 * eye on the NASA test portraits against wider and narrower shapes: the
 * eyes, nose and mouth fall inside the uniformly shrunk core, so they come
 * out as a small copy rather than squeezed into slits, and the edge stays
 * short of the ears and the hairline, which a wider ellipse drags inward. */
export const SHAPE = {
  a: 1.15, // half-width, along the eye line
  b: 1.35, // half-height, down the face
  up: 0.15, // the centre sits this far above the landmark centroid
};
/** Radius (as a share of the ellipse) inside which the shrink is uniform. */
export const CORE = 0.74;

const TABLE_SIZE = 1024;

const smooth = (t) => t * t * (3 - 2 * t);

/** Where a source pixel at normalised radius r lands. */
export function forward(r, size, core = CORE) {
  if (r >= 1) return r;
  if (r <= core) return r * size;
  const sigma = size + (1 - size) * smooth((r - core) / (1 - core));
  return r * sigma;
}

/** F^-1 sampled on [0, 1]: `table[i]` is the source radius drawn at output
 * radius i / (TABLE_SIZE - 1). Built by bisection, which F's monotonicity
 * makes exact up to the tolerance. */
export function inverseTable(size, core = CORE) {
  const table = new Float32Array(TABLE_SIZE);
  for (let i = 0; i < TABLE_SIZE; i++) {
    const t = i / (TABLE_SIZE - 1);
    let lo = 0;
    let hi = 1;
    for (let k = 0; k < 40; k++) {
      const mid = (lo + hi) / 2;
      if (forward(mid, size, core) < t) lo = mid;
      else hi = mid;
    }
    table[i] = (lo + hi) / 2;
  }
  return table;
}

/** Linear lookup into an inverse table. */
export function inverseAt(table, t) {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const x = t * (TABLE_SIZE - 1);
  const i = Math.floor(x);
  const f = x - i;
  return table[i] * (1 - f) + table[Math.min(i + 1, TABLE_SIZE - 1)] * f;
}

/**
 * The warp's ellipse for one detection, from its five landmarks
 * ([rightEye, leftEye, nose, rightMouth, leftMouth], see `yunet.js`).
 *
 * The angle follows the eye line, so a tilted or upside-down face gets an
 * ellipse turned to match. The scale is the eye distance, or the eye-to-mouth
 * drop when the face is turned enough to foreshorten the eyes.
 * `reach` grows or shrinks the whole ellipse (the UI's "area").
 */
export function faceFrame(landmarks, reach = 1, shape = SHAPE) {
  const [re, le, nose, rm, lm] = landmarks;
  const ex = (re[0] + le[0]) / 2;
  const ey = (re[1] + le[1]) / 2;
  const mx = (rm[0] + lm[0]) / 2;
  const my = (rm[1] + lm[1]) / 2;
  const angle = Math.atan2(le[1] - re[1], le[0] - re[0]);
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const eyes = Math.hypot(le[0] - re[0], le[1] - re[1]);
  // The eye-to-mouth drop measured down the face, not along the image.
  const drop = Math.abs((mx - ex) * -sin + (my - ey) * cos);
  const unit = Math.max(eyes, drop / 1.15);
  const pts = [re, le, nose, rm, lm];
  let cx = pts.reduce((acc, p) => acc + p[0], 0) / 5;
  let cy = pts.reduce((acc, p) => acc + p[1], 0) / 5;
  // "Up the face" is minus the down axis (-sin, cos).
  cx += sin * shape.up * unit;
  cy -= cos * shape.up * unit;
  return {
    cx,
    cy,
    angle,
    a: shape.a * unit * reach,
    b: shape.b * unit * reach,
  };
}

/** The ellipse's axis-aligned bounding box, clipped to the image. */
export function frameBounds(frame, width, height, margin = 2) {
  const { cx, cy, a, b, angle } = frame;
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const hw = Math.sqrt(a * a * cos * cos + b * b * sin * sin);
  const hh = Math.sqrt(a * a * sin * sin + b * b * cos * cos);
  const x0 = Math.max(0, Math.floor(cx - hw) - margin);
  const y0 = Math.max(0, Math.floor(cy - hh) - margin);
  const x1 = Math.min(width, Math.ceil(cx + hw) + margin);
  const y1 = Math.min(height, Math.ceil(cy + hh) + margin);
  return { x0, y0, x1, y1 };
}

/** Bilinear RGBA sample with clamp-to-edge, accumulated into `acc`. */
function sampleInto(acc, src, sw, sh, ox, oy, x, y) {
  // `src` is a copy of the region [ox, ox+sw) x [oy, oy+sh); x, y are image
  // coordinates of a pixel centre.
  let fx = x - 0.5 - ox;
  let fy = y - 0.5 - oy;
  if (fx < 0) fx = 0;
  if (fy < 0) fy = 0;
  if (fx > sw - 1) fx = sw - 1;
  if (fy > sh - 1) fy = sh - 1;
  const ix = Math.floor(fx);
  const iy = Math.floor(fy);
  const ax = fx - ix;
  const ay = fy - iy;
  const ix1 = ix + 1 < sw ? ix + 1 : ix;
  const iy1 = iy + 1 < sh ? iy + 1 : iy;
  const i00 = (iy * sw + ix) * 4;
  const i10 = (iy * sw + ix1) * 4;
  const i01 = (iy1 * sw + ix) * 4;
  const i11 = (iy1 * sw + ix1) * 4;
  const w00 = (1 - ax) * (1 - ay);
  const w10 = ax * (1 - ay);
  const w01 = (1 - ax) * ay;
  const w11 = ax * ay;
  for (let k = 0; k < 4; k++) {
    acc[k] += src[i00 + k] * w00 + src[i10 + k] * w10 + src[i01 + k] * w01 + src[i11 + k] * w11;
  }
}

/**
 * Warp one face in place. `pixels` is the whole image's RGBA (an
 * ImageData's `data`); only the ellipse's bounding box is read or written,
 * and only pixels strictly inside the ellipse change.
 *
 * Anti-aliased by supersampling: a shrunk region reads up to 1/size source
 * pixels per output pixel, so each output pixel averages an n x n grid of
 * exact warp positions, n following the local shrink (capped at 3).
 */
export function warpFace(pixels, width, height, frame, table) {
  const { cx, cy, a, b, angle } = frame;
  const { x0, y0, x1, y1 } = frameBounds(frame, width, height);
  if (x1 <= x0 || y1 <= y0) return;
  const rw = x1 - x0;
  const rh = y1 - y0;
  // The source is a snapshot of the region: the warp reads pixels it has
  // already overwritten otherwise. Sources never leave the ellipse, because
  // the inverse maps the unit disc into itself.
  const src = new Uint8ClampedArray(rw * rh * 4);
  for (let y = 0; y < rh; y++) {
    const from = ((y0 + y) * width + x0) * 4;
    src.set(pixels.subarray(from, from + rw * 4), y * rw * 4);
  }
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const acc = [0, 0, 0, 0];
  // g(t)/t at the centre, where it is 1/size; the limit, not 0/0.
  const centre = inverseAt(table, 1e-3) / 1e-3;

  for (let py = y0; py < y1; py++) {
    for (let px = x0; px < x1; px++) {
      const dx = px + 0.5 - cx;
      const dy = py + 0.5 - cy;
      const lx = dx * cos + dy * sin;
      const ly = -dx * sin + dy * cos;
      const t = Math.hypot(lx / a, ly / b);
      if (t >= 1) continue;
      // How many source pixels one output pixel spans. The tangential
      // stretch g(t)/t bounds the radial one g'(t) everywhere (g' = 1/F'
      // and F' >= F/r), so it alone decides the grid.
      const stretch = t > 1e-6 ? inverseAt(table, t) / t : centre;
      const n = stretch > 2.05 ? 3 : stretch > 1.05 ? 2 : 1;
      acc[0] = acc[1] = acc[2] = acc[3] = 0;
      for (let sy = 0; sy < n; sy++) {
        for (let sx = 0; sx < n; sx++) {
          const ox = (sx + 0.5) / n - 0.5;
          const oy = (sy + 0.5) / n - 0.5;
          const ddx = dx + ox;
          const ddy = dy + oy;
          const qx = ddx * cos + ddy * sin;
          const qy = -ddx * sin + ddy * cos;
          const tq = Math.hypot(qx / a, qy / b);
          const k = tq >= 1 ? 1 : tq > 1e-6 ? inverseAt(table, tq) / tq : centre;
          const sxl = qx * k;
          const syl = qy * k;
          const ix = cx + sxl * cos - syl * sin;
          const iy = cy + sxl * sin + syl * cos;
          sampleInto(acc, src, rw, rh, x0, y0, ix, iy);
        }
      }
      const inv = 1 / (n * n);
      const o = (py * width + px) * 4;
      pixels[o] = acc[0] * inv;
      pixels[o + 1] = acc[1] * inv;
      pixels[o + 2] = acc[2] * inv;
      pixels[o + 3] = acc[3] * inv;
    }
  }
}

/** Warp every face in `faces` (frames), one after another. */
export function warpAll(pixels, width, height, frames, size, core = CORE) {
  if (size >= 1) return;
  const table = inverseTable(Math.max(MIN_SIZE, size), core);
  for (const frame of frames) warpFace(pixels, width, height, frame, table);
}
