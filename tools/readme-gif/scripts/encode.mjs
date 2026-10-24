// SPDX-License-Identifier: GPL-3.0-or-later
// work/frames/*.png -> out/menu.gif (one global palette, diff-rectangle optimisation) + out/menu.mp4.
// FFmpeg: $FFMPEG, else ffmpeg on PATH. Needs the image2 demuxer (any standard build); the repo's trimmed
// vendor/ffmpeg build has palettegen/paletteuse but cannot read image sequences.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const FR = path.join(ROOT, 'work', 'frames'), OUT = path.join(ROOT, 'out');
const meta = JSON.parse(fs.readFileSync(path.join(FR, 'meta.json'), 'utf8'));
const ff = process.env.FFMPEG || 'ffmpeg';
const probe = spawnSync(ff, ['-hide_banner', '-demuxers'], { encoding: 'utf8' });
if (probe.status !== 0 || !/ image2 /.test(probe.stdout)) { console.error(`error: ${ff} not found or has no image2 demuxer; set FFMPEG to a standard ffmpeg build`); process.exit(1); }
fs.mkdirSync(OUT, { recursive: true });
const run = (args) => { const r = spawnSync(ff, ['-hide_banner', '-loglevel', 'error', '-y', ...args], { stdio: 'inherit' }); if (r.status !== 0) process.exit(1); };
const input = ['-framerate', String(meta.fps), '-i', path.join(FR, 'f%04d.png')];
const W = 960;
const scale = meta.scale !== 1 ? `scale=${W}:-1:flags=lanczos,` : '';
const palette = path.join(ROOT, 'work', 'palette.png');

// palette from the whole film (stats_mode=full keeps the UI greys and the photo colours)
run([...input, '-vf', `${scale}palettegen=max_colors=256:stats_mode=full:reserve_transparent=0`, palette]);
// ordered (bayer) dither: stable from frame to frame, so the GIF diff stays small; text edges stay clean
run([...input, '-i', palette, '-lavfi', `${scale}fps=${meta.fps}[v];[v][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle`, '-loop', '0', path.join(OUT, 'menu.gif')]);
run([...input, '-vf', `${scale}format=yuv420p`, '-c:v', 'libx264', '-preset', 'slow', '-crf', '16', '-tune', 'animation', '-movflags', '+faststart', path.join(OUT, 'menu.mp4')]);

for (const f of ['menu.gif', 'menu.mp4']) console.log(`  out/${f}  ${(fs.statSync(path.join(OUT, f)).size / 1048576).toFixed(2)} MB`);
