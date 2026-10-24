// SPDX-License-Identifier: GPL-3.0-or-later
// Renders src/index.html frame by frame in headless Chrome (seek contract: window.__seek(t), __duration, __fps).
//   node scripts/render.mjs                 -> work/frames/f0000.png ... (lossless PNG)
//   node scripts/render.mjs --still 1,2.5   -> work/stills/still_1.00s.png ...
//   --scale 2  renders at 2x device pixels (then encode.mjs downsizes)
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { launchChrome } from './_cdp.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const opt = (n, d) => { const i = args.indexOf('--' + n); return i >= 0 ? args[i + 1] : d; };
const stills = opt('still') ? opt('still').split(',').map(Number) : null;
const scale = Number(opt('scale', '1'));
const W = 960, H = 600;

const br = await launchChrome({ args: ['--allow-file-access-from-files', '--font-render-hinting=none'] });
try {
  const page = await br.newPage({ width: W, height: H, scale });
  page.on('Runtime.exceptionThrown', (p) => console.error('page error:', p.exceptionDetails?.exception?.description || p.exceptionDetails?.text));
  await page.goto(pathToFileURL(path.join(ROOT, 'src', 'index.html')).href);
  await page.eval('Promise.all([document.fonts.ready, ...[...document.images].map(i => i.decode().catch(() => 0))]).then(() => window.__ready)');
  const dur = await page.eval('window.__duration'), fps = await page.eval('window.__fps');
  const grab = async (t) => { await page.eval(`(async () => { window.__seek(${t}); await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))); })()`); return page.shot({ format: 'png' }); };
  if (stills) {
    const dir = path.join(ROOT, 'work', 'stills'); fs.mkdirSync(dir, { recursive: true });
    for (const t of stills) { const f = path.join(dir, `still_${t.toFixed(2)}s.png`); fs.writeFileSync(f, await grab(t)); console.log(f); }
  } else {
    const dir = path.join(ROOT, 'work', 'frames');
    fs.rmSync(dir, { recursive: true, force: true }); fs.mkdirSync(dir, { recursive: true });
    const n = Math.round(dur * fps), t0 = Date.now();
    for (let i = 0; i < n; i++) {
      fs.writeFileSync(path.join(dir, `f${String(i).padStart(4, '0')}.png`), await grab(i / fps));
      if (i % fps === 0) process.stdout.write(`\r  frame ${i}/${n}`);
    }
    console.log(`\r  ${n} frames (${dur}s @ ${fps} fps) in ${((Date.now() - t0) / 1000).toFixed(0)}s -> ${dir}`);
    fs.writeFileSync(path.join(dir, 'meta.json'), JSON.stringify({ fps, duration: dur, frames: n, scale }));
  }
} finally {
  await br.close();
}
