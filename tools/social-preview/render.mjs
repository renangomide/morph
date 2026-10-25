// SPDX-License-Identifier: GPL-3.0-or-later
// Renders src/index.html (1280x640) in headless Chrome to docs/images/social-preview.png.
//   node tools/social-preview/render.mjs              -> docs/images/social-preview.png
//   node tools/social-preview/render.mjs --out x.png  -> another path
// Reuses the zero-dependency Chrome DevTools helper of tools/readme-gif (Apache-2.0, from the motionmaxxing skill).
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { launchChrome } from '../readme-gif/scripts/_cdp.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..', '..');
const args = process.argv.slice(2);
const i = args.indexOf('--out');
const out = path.resolve(i >= 0 ? args[i + 1] : path.join(REPO, 'docs', 'images', 'social-preview.png'));

const br = await launchChrome({ args: ['--allow-file-access-from-files', '--font-render-hinting=none'] });
try {
  const page = await br.newPage({ width: 1280, height: 640, scale: 1 });
  page.on('Runtime.exceptionThrown', (p) => console.error('page error:', p.exceptionDetails?.exception?.description || p.exceptionDetails?.text));
  await page.goto(pathToFileURL(path.join(HERE, 'src', 'index.html')).href);
  await page.eval('Promise.all([document.fonts.ready, ...[...document.images].map(i => i.decode().catch(() => 0))]).then(() => window.__ready)');
  await page.eval('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, await page.shot({ format: 'png' }));
  console.log(`${out} (${(fs.statSync(out).size / 1024).toFixed(0)} KB)`);
} finally {
  await br.close();
}
