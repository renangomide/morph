// SPDX-License-Identifier: GPL-3.0-or-later
// Review aid: renders the rebuilt Morph window at 150 % (the reference captures were taken at 150 % scaling)
// and puts it beside a screenshot of the real app: node scripts/compare.mjs <t> <real.png> <out.png>
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { launchChrome } from './_cdp.mjs';
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const [t, real, out] = process.argv.slice(2);
const br = await launchChrome({ args: ['--allow-file-access-from-files'] });
try {
  const page = await br.newPage({ width: 602, height: 671, scale: 1.5 });
  await page.goto(pathToFileURL(path.join(ROOT, 'src', 'index.html')).href);
  await page.eval('document.fonts.ready');
  await page.eval(`window.__winShot(${t})`);
  const tmp = out + '.mine.png';
  fs.writeFileSync(tmp, await page.shot({ format: 'png' }));
  const ff = process.env.FFMPEG || 'ffmpeg';
  const r = spawnSync(ff, ['-v', 'error', '-y', '-i', real, '-i', tmp, '-filter_complex', '[0]scale=903:-1,pad=903:1007[a];[1]scale=903:-1,pad=903:1007[b];[a][b]hstack', out]);
  if (r.status) console.error(String(r.stderr));
} finally { await br.close(); }
