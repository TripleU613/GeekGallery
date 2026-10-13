// Test-only raster helpers: the browser does these with a canvas.

/** Area-average resize of RGBA pixels, the same idea as a canvas drawImage
 * at "high" smoothing quality. Good enough to stand in for it in Node. */
export function resize(rgba, sw, sh, dw, dh) {
  const out = new Uint8ClampedArray(dw * dh * 4);
  const fx = sw / dw;
  const fy = sh / dh;
  for (let y = 0; y < dh; y++) {
    const y0 = y * fy;
    const y1 = Math.min(sh, y0 + Math.max(1, fy));
    for (let x = 0; x < dw; x++) {
      const x0 = x * fx;
      const x1 = Math.min(sw, x0 + Math.max(1, fx));
      const acc = [0, 0, 0, 0];
      let n = 0;
      for (let yy = Math.floor(y0); yy < Math.ceil(y1); yy++) {
        for (let xx = Math.floor(x0); xx < Math.ceil(x1); xx++) {
          const i = (yy * sw + xx) * 4;
          acc[0] += rgba[i];
          acc[1] += rgba[i + 1];
          acc[2] += rgba[i + 2];
          acc[3] += rgba[i + 3];
          n++;
        }
      }
      const o = (y * dw + x) * 4;
      for (let k = 0; k < 4; k++) out[o + k] = acc[k] / n;
    }
  }
  return out;
}
