// SPDX-License-Identifier: GPL-3.0-or-later
// One command, from scratch: demo photos -> real morph-cli conversion -> frames -> out/menu.gif (+ out/menu.mp4).
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO = path.resolve(ROOT, '..', '..');
// the app icon has one source of truth: packaging/icon/morph.svg
fs.copyFileSync(path.join(REPO, 'packaging', 'icon', 'morph.svg'), path.join(ROOT, 'src', 'morph.svg'));
for (const step of ['gen-photos.mjs', 'convert.mjs', 'render.mjs', 'encode.mjs']) {
  console.log(`> ${step}`);
  const r = spawnSync(process.execPath, [path.join(ROOT, 'scripts', step)], { stdio: 'inherit', cwd: ROOT });
  if (r.status !== 0) process.exit(r.status || 1);
}
