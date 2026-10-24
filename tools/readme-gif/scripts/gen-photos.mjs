// SPDX-License-Identifier: GPL-3.0-or-later
// Draws the demo "trip photos" (procedural landscapes, 4000x3000 JPEG) in headless Chrome.
// Deterministic: same code => same pictures. No AI images, no stock photos.
// Writes work/photos/IMG_xxxx.jpg (the files morph-cli converts) and src/thumbs/IMG_xxxx.jpg (small, for the Explorer view).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchChrome } from './_cdp.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PHOTOS = path.join(ROOT, 'work', 'photos');
const THUMBS = path.join(ROOT, 'src', 'thumbs');
fs.mkdirSync(PHOTOS, { recursive: true });
fs.mkdirSync(THUMBS, { recursive: true });

// name, scene, seed
export const SHOTS = [
  ['IMG_2041', 'lake', 11],
  ['IMG_2042', 'peaks', 23],
  ['IMG_2043', 'coast', 37],
  ['IMG_2044', 'forest', 41],
  ['IMG_2045', 'dunes', 53],
  ['IMG_2046', 'dusk', 67],
];

const DRAW = String.raw`
window.drawShot = (scene, seed, W, H, grain) => {
  let s = seed >>> 0;
  const rnd = () => { s |= 0; s = (s + 0x6D2B79F5) | 0; let t = Math.imul(s ^ (s >>> 15), 1 | s); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  const c = document.createElement('canvas'); c.width = W; c.height = H;
  const g = c.getContext('2d');
  const lin = (y0, y1, stops) => { const gr = g.createLinearGradient(0, y0, 0, y1); stops.forEach(([o, col]) => gr.addColorStop(o, col)); return gr; };
  // 1-D value noise for ridge lines
  const noise = (oct) => { const pts = []; for (let i = 0; i < 64; i++) pts.push(rnd()); return (x) => { let v = 0, a = 1, f = 1, n = 0; for (let o = 0; o < oct; o++) { const p = x * f * 8, i = Math.floor(p), t = p - i, u = t * t * (3 - 2 * t); v += a * (pts[(i + o * 7) & 63] * (1 - u) + pts[(i + 1 + o * 7) & 63] * u); n += a; a *= 0.5; f *= 2.1; } return v / n; }; };
  const ridge = (base, amp, col, oct, sharp) => { const n = noise(oct); g.beginPath(); g.moveTo(0, H); for (let x = 0; x <= W; x += W / 400) { let v = n(x / W); if (sharp) v = 1 - Math.abs(v * 2 - 1); g.lineTo(x, base - v * amp); } g.lineTo(W, H); g.closePath(); g.fillStyle = col; g.fill(); };
  const sun = (x, y, r, core, glow) => { const gr = g.createRadialGradient(x, y, 0, x, y, r * 6); gr.addColorStop(0, glow); gr.addColorStop(1, 'rgba(255,255,255,0)'); g.fillStyle = gr; g.fillRect(0, 0, W, H); g.beginPath(); g.arc(x, y, r, 0, 7); g.fillStyle = core; g.fill(); };
  const reflect = (y, tint) => { g.save(); g.translate(0, 2 * y); g.scale(1, -1); g.globalAlpha = 0.55; g.drawImage(c, 0, 0, W, y, 0, 0, W, y); g.restore(); g.fillStyle = tint; g.fillRect(0, y, W, H - y); g.fillStyle = 'rgba(255,255,255,0.10)'; for (let i = 0; i < 260; i++) { const yy = y + Math.pow(rnd(), 1.6) * (H - y); g.fillRect(rnd() * W, yy, 40 + rnd() * 600, 2 + rnd() * 5); } };
  const trees = (base, n, hMin, hMax, col) => { g.fillStyle = col; for (let i = 0; i < n; i++) { const x = rnd() * W, h = hMin + rnd() * (hMax - hMin), w = h * (0.22 + rnd() * 0.1); g.beginPath(); g.moveTo(x, base - h); g.lineTo(x - w, base + 6); g.lineTo(x + w, base + 6); g.closePath(); g.fill(); } g.fillRect(0, base, W, H - base); };
  if (scene === 'lake') {
    g.fillStyle = lin(0, H * 0.62, [[0, '#2f4f86'], [0.55, '#e59a7a'], [1, '#ffd9a8']]); g.fillRect(0, 0, W, H);
    sun(W * 0.62, H * 0.5, H * 0.045, '#fff3d6', 'rgba(255,214,160,0.55)');
    ridge(H * 0.56, H * 0.22, '#6f6f9a', 5, true); ridge(H * 0.6, H * 0.12, '#47507a', 5, false); ridge(H * 0.62, H * 0.05, '#252a45', 4, false);
    reflect(H * 0.62, 'rgba(20,30,60,0.35)');
  } else if (scene === 'peaks') {
    g.fillStyle = lin(0, H * 0.7, [[0, '#4a86c8'], [1, '#d6e8f5']]); g.fillRect(0, 0, W, H);
    ridge(H * 0.5, H * 0.38, '#e9eef5', 6, true); ridge(H * 0.6, H * 0.3, '#8fa3bb', 6, true); ridge(H * 0.72, H * 0.22, '#4f6278', 5, true);
    trees(H * 0.86, 120, H * 0.06, H * 0.16, '#1e3326');
  } else if (scene === 'coast') {
    g.fillStyle = lin(0, H * 0.55, [[0, '#2d6fb0'], [1, '#bfe1f2']]); g.fillRect(0, 0, W, H);
    for (let i = 0; i < 14; i++) { const x = rnd() * W, y = rnd() * H * 0.35, r = H * (0.03 + rnd() * 0.05); g.fillStyle = 'rgba(255,255,255,0.55)'; for (let k = 0; k < 6; k++) { g.beginPath(); g.ellipse(x + (k - 3) * r * 0.9, y + rnd() * r * 0.4, r * (1 + rnd()), r * 0.6, 0, 0, 7); g.fill(); } }
    g.fillStyle = lin(H * 0.55, H * 0.78, [[0, '#1f6f9a'], [1, '#3fb3c4']]); g.fillRect(0, H * 0.55, W, H * 0.25);
    g.fillStyle = 'rgba(255,255,255,0.35)'; for (let i = 0; i < 400; i++) g.fillRect(rnd() * W, H * 0.55 + rnd() * H * 0.23, 30 + rnd() * 260, 3 + rnd() * 4);
    g.fillStyle = lin(H * 0.76, H, [[0, '#e9d7b0'], [1, '#c9ae7f']]); g.beginPath(); g.moveTo(0, H * 0.8); for (let x = 0; x <= W; x += 40) g.lineTo(x, H * 0.78 + Math.sin(x / W * 5) * H * 0.015); g.lineTo(W, H); g.lineTo(0, H); g.fill();
  } else if (scene === 'forest') {
    g.fillStyle = lin(0, H, [[0, '#cfe3d6'], [1, '#7fa58d']]); g.fillRect(0, 0, W, H);
    sun(W * 0.3, H * 0.15, H * 0.03, 'rgba(255,252,235,0.9)', 'rgba(255,250,220,0.6)');
    trees(H * 0.62, 160, H * 0.18, H * 0.34, '#9cb9a4'); trees(H * 0.74, 140, H * 0.26, H * 0.44, '#5d8268'); trees(H * 0.9, 90, H * 0.4, H * 0.62, '#26402f');
  } else if (scene === 'dunes') {
    g.fillStyle = lin(0, H * 0.5, [[0, '#f0b27a'], [1, '#fde3c0']]); g.fillRect(0, 0, W, H);
    sun(W * 0.74, H * 0.28, H * 0.05, '#fff7e8', 'rgba(255,230,190,0.5)');
    ridge(H * 0.6, H * 0.14, '#e0a066', 3, false); ridge(H * 0.72, H * 0.16, '#c97f45', 3, false); ridge(H * 0.86, H * 0.14, '#9c5a2c', 3, false); ridge(H * 0.98, H * 0.1, '#6d3a1c', 3, false);
  } else {
    g.fillStyle = lin(0, H * 0.7, [[0, '#141a3a'], [0.6, '#5a3b74'], [1, '#e5807a']]); g.fillRect(0, 0, W, H);
    g.fillStyle = 'rgba(255,255,255,0.8)'; for (let i = 0; i < 700; i++) { const r = rnd() * 4 + 1; g.fillRect(rnd() * W, rnd() * H * 0.45, r, r); }
    sun(W * 0.2, H * 0.18, H * 0.035, '#fdf6e3', 'rgba(220,220,255,0.25)');
    ridge(H * 0.72, H * 0.2, '#2a2246', 5, true); ridge(H * 0.8, H * 0.1, '#15122a', 4, false);
    reflect(H * 0.8, 'rgba(10,8,30,0.45)');
  }
  // vignette
  const v = g.createRadialGradient(W / 2, H / 2, H * 0.3, W / 2, H / 2, H * 0.95); v.addColorStop(0, 'rgba(0,0,0,0)'); v.addColorStop(1, 'rgba(0,0,0,0.35)'); g.fillStyle = v; g.fillRect(0, 0, W, H);
  // sensor grain: gives the JPEG a camera-like amount of detail
  if (grain) { const id = g.getImageData(0, 0, W, H), d = id.data; for (let i = 0; i < d.length; i += 4) { const n = (rnd() - 0.5) * grain; d[i] += n; d[i + 1] += n; d[i + 2] += n; } g.putImageData(id, 0, 0); }
  return c;
};
window.toJpeg = (c, q) => c.toDataURL('image/jpeg', q).split(',')[1];
window.thumb = (c, w, h) => { const t = document.createElement('canvas'); t.width = w; t.height = h; t.getContext('2d').drawImage(c, 0, 0, w, h); return t.toDataURL('image/jpeg', 0.92).split(',')[1]; };
true`;

const br = await launchChrome();
try {
  const page = await br.newPage({ width: 800, height: 600 });
  await page.eval(DRAW);
  for (const [name, scene, seed] of SHOTS) {
    const big = await page.eval(`(() => { window.__c = drawShot('${scene}', ${seed}, 4000, 3000, 26); return toJpeg(window.__c, 0.93); })()`, { timeout: 120000 });
    fs.writeFileSync(path.join(PHOTOS, name + '.jpg'), Buffer.from(big, 'base64'));
    const th = await page.eval(`thumb(window.__c, 320, 240)`);
    fs.writeFileSync(path.join(THUMBS, name + '.jpg'), Buffer.from(th, 'base64'));
    console.log(name, (fs.statSync(path.join(PHOTOS, name + '.jpg')).size / 1048576).toFixed(2), 'MB');
  }
} finally {
  await br.close();
}
