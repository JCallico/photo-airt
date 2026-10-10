#!/usr/bin/env node
// Copperplate Engraving: an example Photo·AIrt plug-in (contract version 1).
//
// It recreates the look of banknote and old book engravings. The picture is
// made only of parallel engraved lines: dark tones are thick lines, light
// tones thin ones. The lines bend with the image's tones, which gives the
// characteristic sculpted relief. Optional cross-hatching deepens the
// shadows.
//
// It uses only Node's standard library. Its name, description and settings
// live in plugin.toml; this script only renders:
//
//   node copperplate.mjs render <request.json>  -> read input.png, write output.png
//
// The request carries every setting declared in plugin.toml, already checked
// against its range, options and default.
//
// Unlike retro-print, it uses `scale` (line spacing is a size-like setting)
// and `seed` (paper grain), as every plug-in with size-like settings or
// randomness must.

import { readFileSync, writeFileSync } from "node:fs";
import { inflateSync, deflateSync } from "node:zlib";

// [ink RGB, paper RGB]
const INKS = {
  black: [[24, 22, 26], [246, 240, 225]],
  sepia: [[78, 46, 22], [244, 232, 208]],
  banknote: [[22, 70, 52], [236, 240, 222]],
  "intaglio-blue": [[24, 42, 96], [240, 240, 232]],
};

const say = (msg) => process.stdout.write(JSON.stringify(msg) + "\n");

// ------------------------------------------------------------- PNG I/O

function readPng(path) {
  const data = readFileSync(path);
  if (data.readUInt32BE(0) !== 0x89504e47) throw new Error("input is not a PNG");
  let pos = 8, width = 0, height = 0, ctype = 2;
  const idat = [];
  while (pos < data.length) {
    const len = data.readUInt32BE(pos);
    const kind = data.toString("latin1", pos + 4, pos + 8);
    const body = data.subarray(pos + 8, pos + 8 + len);
    pos += 12 + len;
    if (kind === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const depth = body[8], interlace = body[12];
      ctype = body[9];
      if (depth !== 8 || (ctype !== 2 && ctype !== 6) || interlace) throw new Error("expected a non-interlaced 8-bit RGB PNG");
    } else if (kind === "IDAT") idat.push(body);
    else if (kind === "IEND") break;
  }
  const bpp = ctype === 2 ? 3 : 4, stride = width * bpp;
  const raw = inflateSync(Buffer.concat(idat));
  const rgb = new Uint8Array(width * height * 3);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const ftype = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const cur = new Uint8Array(stride);
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? cur[i - bpp] : 0, b = prev[i], c = i >= bpp ? prev[i - bpp] : 0;
      let pred = 0;
      if (ftype === 1) pred = a;
      else if (ftype === 2) pred = b;
      else if (ftype === 3) pred = (a + b) >> 1;
      else if (ftype === 4) {
        const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        pred = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[i] = (line[i] + pred) & 255;
    }
    for (let x = 0; x < width; x++) {
      rgb.set(cur.subarray(x * bpp, x * bpp + 3), (y * width + x) * 3);
    }
    prev = cur;
  }
  return { width, height, rgb };
}

const CRC = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC[(c ^ b) & 255] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function writePng(path, width, height, rgb) {
  const chunk = (kind, body) => {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(body.length, 0);
    head.write(kind, 4, "latin1");
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])), 0);
    return Buffer.concat([head, body, crc]);
  };
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 3 + 1)] = 0;
    Buffer.from(rgb.buffer, rgb.byteOffset + y * width * 3, width * 3).copy(raw, y * (width * 3 + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  writeFileSync(path, Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw, { level: 6 })), chunk("IEND", Buffer.alloc(0)),
  ]));
}

// ------------------------------------------------------------- engraving

// Deterministic hash noise in [0, 1) for paper grain, keyed by the seed.
function hash(x, y, seed) {
  let h = (Math.imul(x, 0x8da6b343) ^ Math.imul(y, 0xd8163841) ^ Math.imul(seed, 0xcb1ab31f)) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 0x5bd1e995) >>> 0;
  return ((h ^ (h >>> 15)) & 0xffffff) / 0x1000000;
}

const smooth = (e0, e1, x) => {
  const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
};

// Separable box blur with running sums (O(1) per pixel), radius in pixels.
function boxBlur(src, w, h, r) {
  r = Math.max(0, Math.round(r));
  if (r === 0) return src.slice();
  const tmp = new Float32Array(w * h), out = new Float32Array(w * h), n = 2 * r + 1;
  for (let y = 0; y < h; y++) {
    let acc = 0;
    for (let i = -r; i <= r; i++) acc += src[y * w + Math.min(w - 1, Math.max(0, i))];
    for (let x = 0; x < w; x++) {
      tmp[y * w + x] = acc / n;
      acc += src[y * w + Math.min(w - 1, x + r + 1)] - src[y * w + Math.max(0, x - r)];
    }
  }
  for (let x = 0; x < w; x++) {
    let acc = 0;
    for (let i = -r; i <= r; i++) acc += tmp[Math.min(h - 1, Math.max(0, i)) * w + x];
    for (let y = 0; y < h; y++) {
      out[y * w + x] = acc / n;
      acc += tmp[Math.min(h - 1, y + r + 1) * w + x] - tmp[Math.max(0, y - r) * w + x];
    }
  }
  return out;
}

function engrave(img, p, seed, scale) {
  const { width: w, height: h, rgb } = img;
  const lum = new Float32Array(w * h);
  for (let i = 0; i < w * h; i++) {
    lum[i] = (0.299 * rgb[i * 3] + 0.587 * rgb[i * 3 + 1] + 0.114 * rgb[i * 3 + 2]) / 255;
  }
  // Auto-levels: stretch the 2nd–98th percentile to the full range, so dark
  // or flat photos still use every line width.
  const hist = new Uint32Array(256);
  for (const v of lum) hist[Math.min(255, Math.round(v * 255))]++;
  const pct = (q) => {
    let acc = 0;
    for (let i = 0; i < 256; i++) if ((acc += hist[i]) >= q * w * h) return i / 255;
    return 1;
  };
  const lo = pct(0.02), hi = Math.max(pct(0.98), lo + 0.05);
  for (let i = 0; i < w * h; i++) lum[i] = Math.min(1, Math.max(0, (lum[i] - lo) / (hi - lo)));

  const spacing = Math.max(2, p.spacing * scale); // size-like: scaled
  // Tone for line width: smoothed over about half a line, so lines stay whole.
  const tone = boxBlur(boxBlur(lum, w, h, spacing * 0.35), w, h, spacing * 0.35);
  // Tone for bending: much softer, so lines flow over forms instead of jittering.
  const shape = boxBlur(boxBlur(lum, w, h, spacing * 2.5), w, h, spacing * 2.5);
  const aa = 1.2 / spacing; // anti-aliasing width in phase units
  const angle = (p.angle * Math.PI) / 180;
  const [ca, sa] = [Math.cos(angle), Math.sin(angle)];
  const [cb, sb] = [Math.cos(angle + Math.PI / 3), Math.sin(angle + Math.PI / 3)];
  const [ink, paper] = INKS[p.ink] ?? INKS.black;
  const out = new Uint8Array(w * h * 3);
  const report = Math.max(1, Math.floor(h / 10));

  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      // Darkness after contrast around mid-grey.
      const d = Math.min(1, Math.max(0, (1 - tone[i] - 0.5) * p.contrast + 0.5));
      // Line phase, displaced by the soft tone so lines bend over forms (relief).
      const t = (x * sa - y * ca) / spacing + p.relief * shape[i];
      const f = t - Math.floor(t);
      const dist = Math.abs(f - 0.5) * 2; // 0 at the line centre, 1 between lines
      // Width follows darkness, capped so even the deepest shadow keeps a sliver
      // of paper between lines; very light tones fade to hairlines.
      const width = 0.86 * Math.pow(d, 1.15);
      let cover = smooth(width + aa, width - aa, dist) * Math.min(1, d * 10);
      if (p.crosshatch && d > 0.55) {
        const t2 = (x * sb - y * cb) / (spacing * 1.15) + p.relief * 0.5 * shape[i];
        const f2 = t2 - Math.floor(t2);
        const w2 = Math.min(0.7, (d - 0.55) * 1.4);
        cover = Math.max(cover, smooth(w2 + aa, w2 - aa, Math.abs(f2 - 0.5) * 2));
      }
      const grain = 1 - 0.05 * hash(x, y, seed);
      for (let c = 0; c < 3; c++) {
        const v = paper[c] * grain * (1 - cover) + ink[c] * cover;
        out[i * 3 + c] = Math.max(0, Math.min(255, Math.round(v)));
      }
    }
    if (y % report === 0) say({ progress: Math.round((y / h) * 1000) / 1000 });
  }
  return out;
}

function render(requestPath) {
  const req = JSON.parse(readFileSync(requestPath, "utf8"));
  if (req.protocol !== 1) throw new Error(`unsupported contract version ${req.protocol}`);
  const p = req.params;
  const img = readPng(req.input);
  if (img.width !== req.width || img.height !== req.height) throw new Error("input size does not match the request");
  const seed = Number(req.seed) >>> 0;
  const out = engrave(img, p, seed, Number(req.scale) || 1);
  writePng(req.output, img.width, img.height, out);
  say({ progress: 1 });
}

try {
  const [cmd, arg] = process.argv.slice(2);
  if (cmd === "render" && arg) render(arg);
  else throw new Error("usage: node copperplate.mjs render <request.json>");
} catch (err) {
  say({ error: String(err?.message ?? err) });
  process.exitCode = 1;
}
