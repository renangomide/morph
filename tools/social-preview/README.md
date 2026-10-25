# Social preview image

`docs/images/social-preview.png` (1280×640, ~315 KB) is the card GitHub shows when the repository link is shared
(Reddit, X, Discord, Slack…). Upload it in the repository's **Settings → General → Social preview**. It is committed,
so it can be used in posts too.

It is made only from code: one HTML/CSS page (`src/index.html`) captured in headless Chrome. No stock or generated images.

## What it shows

The Morph icon and wordmark, the tagline "Convert files right from the Windows 11 right-click menu.", chips of real
Morph formats, "Free · Open source · Works offline", and on the right the Windows 11 context menu of a JPEG selection
with **Morph ▸** open (**Convert to WebP…** hovered) above the finished Morph window: "6 converted · saved 11.8 MB" and
the first file, 4.2 MB → 2.2 MB, −48 %.

## Regenerate

```powershell
node tools\social-preview\render.mjs
```

Writes `docs\images\social-preview.png` (`--out <file>` for another path). Requirements: Windows 11 (system fonts
Segoe UI Variable, Segoe Fluent Icons, Arial, Consolas), Node ≥ 22, Google Chrome (or set `CHROME_PATH`). No npm
packages: it uses the Chrome DevTools helper of `tools/readme-gif/scripts/_cdp.mjs`.

## Fidelity

- Context menu, submenu, cursor and the Morph window pieces are copied from `tools/readme-gif/src/index.html` (see its
  README): Windows 11 dark menu replica, Morph UI rebuilt from `crates/morph-app/ui/*.slint` in Arial like the real app.
- Submenu entries: `image_menu()` in `crates/morph-formats/src/menu.rs`. Numbers: `tools/readme-gif/src/results.json`
  (a real `morph-cli` conversion). Format chips: formats listed in the main README.
- Icon: `packaging/icon/morph.svg` (referenced directly); the purple glow uses its gradient colours.
- Layout keeps everything inside the central 1200×600 safe area; checked at 1:1 and downscaled to 600 px wide.
