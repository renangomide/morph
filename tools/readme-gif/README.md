# README demo GIF

`out/menu.gif` (960×600, 20 fps, 13.7 s loop, ~5.4 MB) is the animation at the top of Morph's README. `out/` is not committed: after a rebuild, copy `out\menu.gif` to `docs\images\menu.gif`.
It is made only from code: an HTML/CSS/SVG page whose every frame is a pure function of time, captured
frame by frame in headless Chrome and encoded with FFmpeg. No video editor, stock footage or generated images.

## What it shows

Explorer (Windows 11, dark) with six photos in `Pictures › Trip` → drag-select → right-click →
**Morph ▸** → **Convert to WebP…** → the conversion window (Images · Convert, WebP preselected, Quality 80) →
**Convert 6** → progress view → "6 converted · saved 11.8 MB" → Close → the six `.webp` files sit next to
the originals → the folder resets and the loop starts again.

## Regenerate

```powershell
npm --prefix tools\readme-gif run build
```

Steps (`scripts/build.mjs`):

1. `gen-photos.mjs` draws six procedural landscape "photos" (4000×3000 JPEG, ~4.2 MB each, seeded, so the
   same every time) into `work/photos/`, and their Explorer thumbnails into `src/thumbs/`.
2. `convert.mjs` converts them for real with `morph-cli convert --action image.convert --format webp --quality 80`
   (same engine and defaults as the app's window) and writes `src/results.json` / `results.js` with the sizes,
   formatted like the app (`view.rs::format_size`, `progress.rs::size_change`). If `morph-cli.exe` is not built,
   the committed `results.json` is used. Build it with `cargo build -p morph-cli --release`.
3. `render.mjs` renders `src/index.html` to `work/frames/*.png` (`npm run stills -- 1.5,4.8` for single PNGs).
4. `encode.mjs` builds one 256-colour palette for the whole film, then the GIF (Bayer dither, changed rectangles
   only) and `out/menu.mp4`.

Requirements: Windows 11 (the page uses the system fonts Segoe UI, Segoe UI Variable, Segoe Fluent Icons, Arial and
Consolas), Node ≥ 22, Google Chrome (or set `CHROME_PATH`), and an FFmpeg build with the `image2` demuxer on `PATH`
(or set `FFMPEG`). The repo's trimmed `vendor\ffmpeg` has `palettegen`/`paletteuse` but cannot read image sequences.
No npm packages.

## Fidelity

- **Morph window**: rebuilt from `crates/morph-app/ui/*.slint` (theme tokens, card/chip/pill/segment sizes, paddings,
  Feather icons) and checked against screenshots of the real release build (dark theme, 150 % scaling) with
  `scripts/compare.mjs <t> <capture.png> <out.png>`, which puts the rebuilt window beside a capture. Slint 1.18 with no
  font set renders in Arial on Windows, so the rebuild uses Arial too. The accent is the Windows **default** blue
  (`#60CDFF` in dark mode), not a personal accent colour.
- **Strings and numbers**: the real menu entries of `image_menu()` in `crates/morph-formats/src/menu.rs`, the app's own
  labels, and the sizes from the real conversion above (4.2 MB → 2.2 MB, −48 % …, saved 11.8 MB).
- **Explorer and the context menu** are Windows UI, rebuilt by hand in the Windows 11 dark style with Segoe Fluent Icons
  glyphs. Only Windows' usual entries for a multi-photo selection plus Morph's entry are shown.

## Files

- `src/index.html`: the composition (scene, timeline `T`, camera keys, cursor path).
- `scripts/`: `build`, `gen-photos`, `convert`, `render`, `encode`; review aids `compare.mjs` and `sheet.py`
  (Python + Pillow, tiles `work/stills/` into a contact sheet).
- `scripts/_cdp.mjs`: zero-dependency Chrome DevTools helper copied from the motionmaxxing skill (Apache-2.0); the
  easing curves in `index.html` come from the same runtime (Apache-2.0).
- `work/`, `src/thumbs/`, `src/results.js`, `src/morph.svg` (copied from `packaging/icon/morph.svg`) are generated
  and git-ignored.

## Review notes

Checked on contact sheets of the rendered frames and of frames decoded back from the GIF: text crisp at 1:1, no
clipping inside the windows, camera always inside the Explorer window, seamless loop (last frame differs from the first
only by a few cursor pixels). motionmaxxing gates on `out/menu.mp4`: G0 and G2 PASS (`look.py`), G5 PASS (`lint.mjs`;
one WARN for the muted size pill, which is the real `text-muted` token). G3 "final shot ≤ 25 %" does not apply: this is
one continuous loop with no end card.
