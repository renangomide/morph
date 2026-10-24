# Morph — Right-click File Converter for Windows (Product & Technical Spec)

> **Working name:** "Morph" is a placeholder. The final name has not been chosen. Keep the display name in **one constant** (`APP_DISPLAY_NAME` in `morph-formats`) plus the MSIX manifest, so renaming is a find-and-replace.

---

## 0. One-paragraph summary

Morph is a Windows 11 app written in Rust. It adds a **"Morph ▸" submenu to the modern (first-level) Explorer right-click menu**, the same way WinRAR and 7-Zip do. Right-click one or many files and the submenu offers conversions that make sense for those files (images, PDFs, video, audio, Office documents). Clicking any item **opens a small conversion window**, pre-filled with the chosen action, where the user can adjust settings and press **Convert**. The window then shows progress. It can be closed to keep working in the background, and a Windows notification appears when the job is done. Results are saved **next to the originals with a new name; originals are never modified**. It installs as an **MSIX package**, is **open source on GitHub under GPL-3.0**, and the interface is in **English**.

---

## 1. Decisions (from the interview)

| Topic | Decision |
|---|---|
| File types in v1 | Images, PDF, Video & Audio, Office documents |
| Click behavior | **Always opens a window** (pre-filled with the clicked action) |
| Explorer menu | **Modern Windows 11 menu** (IExplorerCommand + package identity) |
| Menu shape | **Submenu with targets**: `Morph ▸ To PNG / To WebP / Compress… / More options…` |
| Multiple files | **Batch supported**: one window, one job for the whole selection |
| Mixed selection | **Grouped by type**: one tab per type, each with its own settings, one "Convert all" button. Special action "Combine into one PDF" when the selection is only images + PDFs |
| Engines | **Bundle FFmpeg**. For Office: **use MS Office if installed, otherwise LibreOffice if installed, otherwise show a download link** |
| Output location | **Same folder, new name**. Never overwrite. Collisions become `name (1).ext` |
| Language | English only (but keep all UI strings in one place) |
| UI toolkit | **Slint** (Fluent style, follows system light/dark) |
| Audience | **Open source on GitHub**; signing comes later |
| Installer | **MSIX**, per-user, no admin |
| Extras | **Settings app**, **drag & drop window**, **done notification (toast)**. *No* auto-update in v1 |
| License | **GPL-3.0-or-later** (allows bundling a full GPL FFmpeg build with x264/x265) |
| After "Convert" | Window turns into a **progress view** (per-file rows, cancel). Can be closed and the job keeps running in the background. **Toast when done** |

### Operations chosen for v1

- **Images:** convert format, compress, resize, clean & fix (strip metadata, auto-rotate, rotate/flip)
- **PDF:** compress, PDF → images, merge / split, PDF → text, PDF → Word
- **Video & audio:** convert format, compress video, extract audio from video, video → GIF (added 2026-10-20)
- **Office:** Office → PDF, between Office formats, export to images / text

### Explicitly out of scope for v1 (v2 candidates)
Trimming, auto-update, other languages, folder right-click ("convert everything inside"), animated image conversion beyond the first frame, OCR, ARM64 build, classic-menu fallback for Windows 10.

---

## 2. Target platform

- **Windows 11 22H2+, x64.** Windows 10 1903+ may work (same manifest), but it is best effort and untested.
- Rust stable, `x86_64-pc-windows-msvc` toolchain.
- **Development must happen on Windows** (MSVC, Windows SDK, MSIX tools, Explorer). Not in WSL.

---

## 3. User experience

### 3.1 The right-click menu

A single top-level entry **"Morph"** (with app icon) in the modern menu, with a submenu. The submenu content depends on what is selected. **Every leaf item opens the conversion window** with that action pre-selected. Items ending in "…" just signal that a window opens.

**Only images selected**
```
Morph ▸  Convert to JPG…
         Convert to PNG…
         Convert to WebP…
         Convert to AVIF…
         Convert to PDF…          (one PDF per image, §4.6)
         ─────────────
         Compress…
         Resize…
         Remove metadata…
         ─────────────
         More options…
```

**Only PDFs selected**
```
Morph ▸  Convert to Word…
         Convert to Markdown…
         Convert to images…
         Compress…
         Merge into one PDF…      (only when 2+ PDFs are selected)
         ─────────────
         More options…            (window: also HTML, Extract text, Split)
```

**Only videos selected**
```
Morph ▸  Convert to MP4…
         Convert to MKV…
         Convert to WebM…
         ─────────────
         Compress…
         Extract audio…           (MP3 by default)
         Convert to GIF…
         ─────────────
         More options…
```

**Only audio selected**
```
Morph ▸  Convert to MP3…
         Convert to M4A…
         Convert to WAV…
         Convert to FLAC…
         ─────────────
         More options…
```

**Only Office / text documents selected** (top 4 per type, §4.6; a target every selected file already is drops out and the next moves up)
```
Word (doc/docx/odt/rtf):   Convert to PDF… / Markdown… / ODT… / TXT…
Excel (xls/xlsx/ods/csv):  Convert to PDF… / CSV… / Markdown… / ODS…   (CSV input: XLSX first)
PowerPoint (ppt/pptx/odp): Convert to PDF… / images… / Markdown… / ODP…
Text (md/txt/html):        Convert to PDF… / DOCX… / HTML… / TXT…
         ─────────────
         More options…            (window lists every valid target)
```

**Mixed selection** (e.g. 3 photos + 1 PDF + 1 video)
```
Morph ▸  Convert 5 files…                   (opens the grouped window)
         Combine into one PDF…              (only if the selection is only images and/or PDFs)
```
With 16 or more selected files the entry reads **"Convert files…"** (no count; approved 2026-10-21). Windows 11 builds the menu from a 16-item sample first and then rebuilds it with the full selection; the title must be identical in both builds or the click is lost. The window shows the real count.

Rules:
- Hide the "Morph" entry completely if nothing in the selection is supported (e.g. only folders or `.exe`).
- Unsupported files in a mixed selection are ignored and listed as "Skipped" in the window.
- Menu visibility is decided **only from file extensions**. Never open or read files while building the menu (it must be instant).
- File types turned off in Settings do not count as supported.

### 3.2 The conversion window

A compact window (~560×640, resizable), centered on the mouse cursor's monitor, always on top only while it opens (then normal). It follows the system light/dark theme and uses the Fluent style.

```
┌──────────────────────────────────────────────────────────┐
│  Morph                                              ─ □ ✕ │
│  12 files · 48.3 MB                      [▾ Show files]   │
├──────────────────────────────────────────────────────────┤
│  [ Images (10) ]  [ PDF (1) ]  [ Video (1) ]   ← tabs only │
│                                                when mixed │
│  Action:   ( Convert ) ( Compress ) ( Resize ) ( Clean )  │
│                                                          │
│  Format:   [ WebP              ▾ ]                        │
│  Quality:  ───────────●────  80                           │
│            ☐ Lossless                                     │
│  ☑ Keep metadata   ☐ Remove location (GPS) only          │
│                                                          │
├──────────────────────────────────────────────────────────┤
│  Save to:  (•) Next to the original files                │
│            ( ) Choose folder…  [ C:\Users\…\Pictures ]    │
│  ☑ Remember these settings                                │
│                                                          │
│                           [ Cancel ]   [ Convert 12 ▸ ]  │
└──────────────────────────────────────────────────────────┘
```

- **Show files** expands a list of selected files with name, size, and a remove (✕) button per file. For **Merge**, this list is **drag-to-reorder** and starts sorted by natural filename order (`2.pdf` comes before `10.pdf`).
- The action chosen in the menu is pre-selected. All other actions for that type stay one click away.
- Option panels change with the action (see §4).
- **Remember these settings** (on by default) saves the last-used options per action. Next time, those are the defaults.
- Keyboard: `Enter` = Convert, `Esc` = Cancel/close, full Tab navigation.
- **Visual design (M9, approved 2026-10-22):** A UI/UX design spec and HTML prototype (design reference, not published in this repository) set the look: Fluent cards, segmented action bar, format chips, category colours, Windows accent colour, native Windows title bar. Where the prototype leaves out an option or behaviour, the current app/SPEC behaviour stays. Decisions:
  - **PDF tabs:** Convert (chips: Word, Markdown, HTML, Images, Plain text) | Compress | Split | Merge. Merge shows only with 2+ files. Menu entries and action IDs are unchanged.
  - **Mixed selections:** each category card shows its count, a one-line summary ("→ WebP") and a checkbox to skip the whole category. The Skipped list stays.
  - **Gear button** in the header opens Settings inside the same window, with **Back** returning to the unchanged selection.
  - Target formats stay limited to valid targets per input type.
  - The missing-engine banner also gets **Check again**.
- If an engine is missing (no Office/LibreOffice), that tab shows a banner: *"To convert documents, install Microsoft Office or LibreOffice (free)."* with a **Download LibreOffice** button. Convert is disabled for those files only (CSV ↔ XLSX still works, since it is native).

### 3.3 Progress view (same window, after Convert)

```
┌──────────────────────────────────────────────────────────┐
│  Converting 12 files…                     7 / 12          │
│  ██████████████████░░░░░░░░░  58%                         │
├──────────────────────────────────────────────────────────┤
│  ✓ IMG_0012.jpg → IMG_0012.webp    4.1 MB → 0.9 MB  −78%  │
│  ✓ IMG_0013.jpg → IMG_0013.webp    3.8 MB → 0.8 MB  −79%  │
│  ⟳ holiday.mp4  → holiday.mp4      ████░░░ 41%  ~1m left  │
│  ✕ broken.png   Failed: file is corrupted     [Details]   │
│  · report.docx  Waiting…                                  │
├──────────────────────────────────────────────────────────┤
│  [ Cancel all ]                      [ Run in background ] │
└──────────────────────────────────────────────────────────┘
```

- **Run in background** (or closing the window during a job) hides the window. A **tray icon** shows progress in its tooltip. Its menu has *Show progress* and *Cancel*. The process exits when the job ends.
- When finished, the window (if open) shows a summary: *"11 converted, 1 failed · saved 38.2 MB"* with **Open folder** and **Close** buttons. Each result row has *Open* and *Show in folder*.
- **Toast notification** when the job finishes, if the window is hidden or not focused: *"Converted 11 files · saved 38.2 MB"*, with an **Open folder** button. If some files failed: *"11 converted, 1 failed"*. Clicking the toast shows the results window.
- Cancel removes partial outputs. Files already completed are kept.
- **← Adjust options** (finished view, approved 2026-10-22) returns to the options page with the same files and options, so the user can change them and run again.

### 3.4 Main app (Start menu): drag & drop + Settings

Launching **Morph** from the Start menu opens the main window with two pages (left navigation):

1. **Convert**: a large drop zone ("Drop files here or click to browse"). Dropping files opens the same conversion window flow (same grouping logic as the right-click menu).
2. **Settings**:
   - **File types**: toggles for Images / PDF / Video / Audio / Office. Turning one off removes it from the right-click menu.
   - **Defaults** per type: JPEG quality (85), WebP quality (80), AVIF quality (60), PNG optimization level, PDF compression preset (Medium), video preset (Best (automatic)), video encoder (Auto/CPU; no forced GPU, approved 2026-10-21), audio bitrate (192 kbps MP3), image DPI for PDF → images (150).
   - **Output**: next to originals (default) / fixed folder. Option *"Open folder when done"* (off by default). Option *"Keep original file dates"* (off).
   - **Office engine**: Auto (MS Office → LibreOffice) / Microsoft Office / LibreOffice, plus detected status ("✓ Microsoft Word 16.0 found", "✕ LibreOffice not found [Download]").
   - **Performance**: parallel image jobs (default = CPU cores − 1), use GPU for video (default on if detected).
   - **Notifications**: toast on completion (on).
   - **Appearance**: System / Light / Dark.
   - **About**: version, links (GitHub, issues), *Open log folder*, third-party licenses list.

---

## 4. Formats & operations matrix

### 4.1 Images

**Input:** jpg, jpeg, jfif, png, webp, avif, heic, heif, bmp, tif, tiff, gif (first frame), ico (largest size), tga, qoi, svg (rasterized)
**Output:** JPG, PNG, WebP, AVIF, BMP, TIFF, GIF, ICO

| Action | Options |
|---|---|
| **Convert** | Target format. Quality slider for lossy formats (JPG/WebP/AVIF). *Lossless* toggle (WebP/AVIF). Background color when going from transparent to JPG (default white). ICO: size set (16, 32, 48, 64, 128, 256; default all; non-square images padded). SVG input: output width (default 1024 px). |
| **Compress** | Mode: **Best (automatic)** (default; approved 2026-10-19): per image, search quality levels and keep the smallest file that still looks identical to the original by a perceptual score (e.g. SSIMULACRA2); PNG: oxipng lossless, plus imagequant palette only if the result passes the same "looks identical" check; if nothing beats the input, keep it and report "Already optimized". Other modes: **Quality** (slider) or **Target size** (e.g. "≤ 500 KB": binary search on quality, max 8 iterations, then downscale if still too big). Keep the same format by default; optional "Convert to WebP for best size". PNG: lossless optimization (oxipng) + optional lossy palette quantization (imagequant). |
| **Resize** | By percent / max width / max height / exact W×H (with *keep aspect ratio*, default on). *Don't enlarge smaller images* (default on). Lanczos3 filter. |
| **Clean & fix** | Remove all metadata / remove only GPS location. Auto-rotate from EXIF orientation (always applied on any re-encode, then orientation tag reset). Rotate 90° / 180° / 270°, flip H/V. |

Rules:
- Every re-encode applies EXIF orientation first, so outputs are never sideways.
- Keep the ICC color profile when the output format supports it.
- When *Keep metadata* is on, copy EXIF/XMP into JPG/PNG/WebP/AVIF outputs where the format supports it (best effort).
- Animated GIF/WebP input: v1 converts the first frame only and shows a note in the results row.

### 4.2 PDF

| Action | Options / Behavior |
|---|---|
| **Compress** | Presets: **Low** (images ≤ 300 DPI, JPEG q85), **Medium** (≤ 150 DPI, q75, default), **High** (≤ 96 DPI, q60), plus Custom (DPI + quality). Rewrites embedded raster images (downscale + JPEG re-encode, skipping images that would get bigger), compresses uncompressed streams, removes unused objects, uses object streams. If the result is not smaller, keep the original and report "Already optimized". |
| **To images** | Format PNG/JPG/WebP, DPI (72/150/300/custom), page range (all / e.g. `1-3,7`). Output goes into a folder `name (pages)/name-01.png`. |
| **To Word** | Uses **MS Word** if installed (best quality: Word's PDF reflow); otherwise LibreOffice. Warn in the UI: *"Layout may change; works best on text-based PDFs."* |
| **Extract text** | `.txt` UTF-8, with an optional page separator line `--- Page N ---`. Scanned PDFs with no text: report "No text found (scanned PDF?)". |
| **Split** | Every page / every N pages / custom ranges (`1-3,4-10,11-`). Output folder `name (split)/name-p1-3.pdf`. |
| **Merge** | 2+ PDFs, and/or images, into one file. Reorderable list. Images become pages with page size: *Fit to image* (default) / A4 / Letter, with margin option. Output `Merged.pdf` (or `Merged (1).pdf`) in the folder of the first file. Option: add bookmarks per source file (default on). |

### 4.3 Video

**Input:** mp4, mkv, mov, avi, webm, wmv, flv, m4v, mpg, mpeg, ts, mts, 3gp
**Output containers:** MP4, MKV, WebM, MOV, AVI

| Action | Options / Behavior |
|---|---|
| **Convert** | Target container. **Fast mode (default on):** if the existing streams are compatible with the target container, remux with `-c copy` (seconds, no quality loss). Otherwise re-encode: MP4/MOV/MKV → H.264 + AAC; WebM → VP9 + Opus; AVI → H.264 + MP3. Advanced: video codec (H.264 / H.265 / AV1 / VP9), audio codec, quality. |
| **Compress** | Presets: **Best (automatic)** (default, approved 2026-10-20; see below), **Balanced** (H.264 CRF 23-equivalent), **Smaller** (H.265 CRF 28), **Discord (≤ 10 MB)**, **Email (≤ 25 MB)**, **Custom target size (MB)**. Resolution: Keep / 1080p / 720p / 480p (never upscale). Target-size presets use two-pass bitrate: `video_kbps = (0.96 × target_MB × 8000 / duration_s) − audio_kbps`, with audio 96–128 kbps; MB = 1,000,000 bytes and a 96% aim, so the file fits whichever MB the receiving site means (decided 2026-10-21; Explorer may show a 10.4 MB-in-bytes file as "9.9 MB", which still gets compressed). If the needed bitrate is too low for the resolution, automatically step down resolution and say so. Encoder: **Auto** (GPU if available: NVENC → QSV → AMF, else CPU x264/x265/SVT-AV1) / force CPU. |
| **Extract audio** | Saves the sound track as its own file next to the video: MP3 (default, 192 kbps), M4A, WAV, FLAC, Opus, same options as Audio → Convert. Copies the stream without re-encoding when it already fits the target (e.g. AAC → M4A). No audio track → plain-English error. Output `clip.mp3`. |
| **Convert to GIF** | Animated GIF for chats and docs. Options: Start (mm:ss, default 0:00), Length (seconds, default 10, max 60), Width (Keep / 640 / 480 (default) / 320, never upscale), Frame rate (10 / 15 (default) / 24). Two-step palette (`palettegen` + `paletteuse`) for good colours. Output `clip.gif`. |

**Compress "Best (automatic)"**: same promise as images: the smallest file with no visible quality loss, no settings needed. Keeps resolution and container; video H.264 (plays everywhere on Windows without extra codecs), audio copied when it is already AAC/Opus at ≤ 192 kbps, else AAC 160 kbps. Picks the quality by encoding a few short samples spread across the clip at candidate CRFs and keeping the highest CRF whose VMAF stays ≥ 93 (FFmpeg's `libvmaf`), then one full encode. Never writes a file bigger than the original: if the result isn't at least 5% smaller, nothing is written and the file shows "Already well compressed". Uses the GPU encoder when available (quality search then uses the encoder's own quality setting). WebM inputs stay WebM: VP9 + Opus instead of H.264 (approved 2026-10-20).

**Never bigger (all quality presets, approved 2026-10-20):** Best, Balanced and Smaller never write a file that isn't at least 5% smaller than the original; the file shows "Already well compressed" instead. Target-size presets (Discord, Email, Custom) aim at a size, not a saving: they write whenever the original is over the target, and skip with "Already under 10 MB" (etc.) when it isn't.

### 4.4 Audio

**Input:** mp3, wav, flac, aac, m4a, ogg, opus, wma, aiff, aif
**Output:** MP3, M4A (AAC), WAV, FLAC, OGG (Vorbis), Opus

| Action | Options |
|---|---|
| **Convert** | Bitrate for lossy formats (MP3: 128/192/256/320 kbps CBR or VBR V0/V2; AAC 128–256; Opus 64–192). Sample rate: Keep / 44.1 / 48 kHz. Channels: Keep / Mono. Keep tags and cover art (`-map_metadata 0`, cover where the container allows). Warn when converting lossy → lossless ("won't improve quality"). |

### 4.5 Office documents

| Family | Input | Outputs |
|---|---|---|
| Word-like | doc, docx, odt, rtf | PDF, DOCX, ODT, RTF, TXT, HTML, Markdown, PNG/JPG (pages) |
| Excel-like | xls, xlsx, ods, csv | PDF, XLSX, ODS, CSV |
| PowerPoint-like | ppt, pptx, odp | PDF, PPTX, ODP, PNG/JPG (slides) |
| Text-like (approved 2026-10-19) | md, markdown, txt, html, htm | DOCX, PDF, ODT, HTML, Markdown, TXT (never the same format as the input) |

Text-like inputs: Markdown → HTML natively (CommonMark + tables, e.g. `pulldown-cmark`; works without any engine); TXT → HTML natively (escaped, paragraphs kept); HTML → Markdown / TXT natively; HTML → DOCX / PDF / ODT through the Office engine. Local images referenced by relative paths are kept; remote images are not fetched.

Behavior:
- **Engine choice** (Settings → Office engine, default Auto): Microsoft Office via COM automation if installed, else LibreOffice headless if installed, else unavailable.
- **CSV ↔ XLSX is native** (no Office needed): `calamine` reads, `rust_xlsxwriter` writes. CSV options: delimiter (auto-detect / `,` / `;` / tab), encoding UTF-8 (with BOM option for Excel). XLSX with multiple sheets → CSV creates one file per sheet: `name - Sheet1.csv`.
- **Markdown** output: convert to HTML with the engine, then HTML → Markdown natively (`htmd` or similar).
- **Word → images**: convert to PDF in a temp folder, then render pages with PDFium (same code as PDF → images).
- **PowerPoint → images**: PowerPoint COM `SaveAs` PNG (one file per slide), or LibreOffice → PDF → PDFium render. Output folder `name (slides)/name-01.png`.
- **Excel → PDF**: whole workbook, landscape if the sheet is wider than tall (use the engine's default print setup; don't fight it).

### 4.6 Two-way conversions and Markdown quality (approved 2026-10-20)

Goal: most conversions work in both directions, and "to Markdown" matches or beats Microsoft MarkItDown (MIT; ideas may be ported, any copied code keeps its MIT notice in THIRD_PARTY_NOTICES).

| From ↔ To | Forward | Back |
|---|---|---|
| PDF ↔ Markdown | **Native** PDFium layout reader (below) | MD → PDF (engine) |
| PDF ↔ HTML | Same reader → Markdown → HTML | HTML → PDF (engine) |
| PDF ↔ TXT | Extract text (exists) | TXT → PDF (engine) |
| PDF ↔ DOCX | PDF → Word (exists) | DOCX → PDF (exists) |
| PDF ↔ images | PDF → images (exists) | Image → PDF, **one PDF per image** (new), or Merge into one |
| DOCX/ODT/RTF ↔ Markdown | **Native OOXML reader** for docx (styles + outline levels → headings, numbering → nested lists, tables, links, footnotes, image alt text + images to `name (images)/`); doc/odt/rtf via engine → HTML → Markdown | MD → DOCX/ODT (engine) |
| XLSX/XLS/ODS ↔ Markdown | **Native** (calamine): `## Sheet` + one pipe table per sheet; ints without ".0", real dates, `\|` and newlines escaped, empty edges trimmed, empty sheets skipped | — (MD → XLSX: later) |
| XLSX ↔ CSV | Exists (one CSV per sheet) | CSV → XLSX (exists) |
| PPTX ↔ Markdown | **Native OOXML reader**: `## Slide N: Title`, nested bullets from levels, reading order by position, tables, links, picture alt text, chart data as a table, speaker notes | — (MD → PPTX: later) |
| HTML ↔ Markdown | Pre-clean (drop script/style/nav/noscript, shorten data: images) + htmd | MD → HTML (exists) |

**PDF → Markdown reader** (in order of payoff): lines from char baselines and word gaps; paragraphs joined by vertical gap/indent; headings from font size vs. the body (modal) size plus bold-and-short, levels by size rank, PDF outline used as a hint; bullets/numbered lists; de-hyphenation; repeated headers/footers and bare page numbers removed; two-column reading order via a whitespace gutter; simple tables when ≥ 3 rows share ≥ 2 aligned columns; images saved to `name (images)/`. Scanned PDFs: "No text found (scanned PDF?)" (OCR is a later version).

**Not now:** PDF → XLSX/CSV (table extraction from PDFs is unreliable), MD → XLSX, MD → PPTX.

**Right-click menu:** each file type shows its 4 most common targets + "More options…"; the window lists every valid target. XLSX shows "Convert to CSV…".

---

## 5. Architecture

### 5.1 Cargo workspace

```
morph/
├─ Cargo.toml                 # [workspace]
├─ crates/
│  ├─ morph-formats/          # TINY, no heavy deps. Extension → category table, menu item table,
│  │                          # action IDs, APP_DISPLAY_NAME, config "menu" section reader.
│  │                          # Used by the shell DLL, so it must stay lightweight.
│  ├─ morph-core/             # Job model, scheduler, all converters, engine detection,
│  │                          # output naming, progress events, cancellation. No UI.
│  ├─ morph-shell/            # cdylib → morph_shell.dll: IExplorerCommand COM server.
│  ├─ morph-app/              # Slint GUI → Morph.exe (conversion window, progress, main window,
│  │                          # settings, tray, toasts).
│  └─ morph-cli/              # morph-cli.exe: same core from the command line (testing + power users).
├─ packaging/
│  ├─ AppxManifest.xml.in     # template, version injected at build
│  ├─ Assets/                 # icons and logos at all required scales
│  ├─ make-msix.ps1           # stage layout → makeappx pack → signtool sign
│  ├─ dev-register.ps1        # register loose layout (Developer Mode, no signing) + restart Explorer
│  └─ dev-cert.ps1            # create and trust a self-signed test certificate
├─ scripts/fetch-deps.ps1     # downloads pinned FFmpeg, PDFium etc. with SHA-256 check → vendor/
├─ vendor/                    # (gitignored) third-party binaries
├─ vcpkg.json                 # native libs built through vcpkg (libheif, dav1d) if used
├─ tests/fixtures/            # small sample files for every input format
├─ docs/                      # architecture notes, contributing, build guide
├─ LICENSE                    # GPL-3.0
├─ THIRD_PARTY_NOTICES.md
└─ .github/workflows/{ci.yml, release.yml}
```

### 5.2 Process model

```
Explorer ──loads──▶ morph_shell.dll (in a COM surrogate, with package identity)
                         │ user clicks "Convert to WebP…"
                         │ 1. collect file paths from IShellItemArray
                         │ 2. write request JSON → <LocalData>\requests\<uuid>.json
                         │ 3. activate the app: IApplicationActivationManager
                         │    .ActivateApplication(AUMID, "--request <path>")
                         ▼
                    Morph.exe (Slint) ── reads + deletes request ── shows window
                         │ Convert pressed
                         ▼
                    morph-core scheduler (threads inside Morph.exe)
                         ├─ CPU pool (rayon): images, PDF work, CSV/XLSX
                         ├─ FFmpeg queue: 1 concurrent ffmpeg.exe child (configurable 1–2)
                         └─ Office queue: 1 concurrent (Word/Excel/PowerPoint COM or soffice.exe)
```

- **Request file** (instead of command-line args) avoids the 32K command-line limit for big selections:
  ```json
  { "version": 1, "files": ["C:\\…\\a.jpg", "…"], "action": "image.convert", "preset": { "format": "webp" } }
  ```
- `<LocalData>` = the package's local app data folder (`ApplicationData::Current().LocalFolder()` when packaged). When running unpackaged in development, use `%LOCALAPPDATA%\Morph`. Put this resolution in `morph-formats` so the DLL and the exe always agree.
- One `Morph.exe` process per request. Two right-clicks give two windows. That's fine.
- The app must also accept `Morph.exe <file1> <file2> …` (unpackaged/dev, and Start menu drag-and-drop onto the shortcut).

### 5.3 Core API sketch (`morph-core`)

```rust
pub enum Category { Image, Pdf, Video, Audio, Word, Excel, PowerPoint }

pub enum Action {
    ImageConvert(ImageConvertOpts), ImageCompress(ImageCompressOpts),
    ImageResize(ImageResizeOpts),   ImageClean(ImageCleanOpts),
    PdfCompress(PdfCompressOpts),   PdfToImages(PdfToImagesOpts),
    PdfToWord, PdfToText(PdfToTextOpts), PdfSplit(PdfSplitOpts), PdfMerge(PdfMergeOpts),
    VideoConvert(VideoConvertOpts), VideoCompress(VideoCompressOpts),
    AudioConvert(AudioConvertOpts),
    OfficeConvert(OfficeConvertOpts),
}

pub struct Task { pub id: TaskId, pub inputs: Vec<PathBuf>, pub action: Action, pub output: OutputSpec }
pub struct Job  { pub tasks: Vec<Task> }

pub enum Event {
    TaskStarted { id: TaskId },
    TaskProgress { id: TaskId, fraction: f32, eta: Option<Duration> },
    TaskDone { id: TaskId, outputs: Vec<PathBuf>, bytes_in: u64, bytes_out: u64, note: Option<String> },
    TaskFailed { id: TaskId, error: String, details: String },
    TaskSkipped { id: TaskId, reason: String },
    JobFinished { summary: Summary },
}

pub trait Converter: Send + Sync {
    fn run(&self, task: &Task, ctx: &TaskCtx) -> Result<Vec<PathBuf>, ConvertError>;
}
// TaskCtx carries: a cancellation token (checked often), a progress sender, a temp dir, engine handles.
```

- All options structs are `serde`-serializable (for "remember settings", the request file, and the CLI).
- Every option struct has `Default` matching the Settings defaults.

### 5.4 Output naming & file safety

1. Output folder = folder of the input (or the folder chosen in the window).
2. Base name rules:
   - Format changes: `photo.jpg → photo.webp`
   - Same extension (compress/resize/clean): add a suffix: `photo (compressed).jpg`, `photo (resized).jpg`, `photo (clean).jpg`, `video (compressed).mp4`
   - Multi-output: folder `name (pages)`, `name (split)`, `name (slides)`
3. If the target exists: `photo (1).webp`, `photo (2).webp`, …
4. Write to a temp file in the **same folder** (`.~morph-<uuid>.tmp`, hidden attribute; FFmpeg outputs stay visible because Windows won't let FFmpeg overwrite a hidden file, noted 2026-10-20), then rename atomically. On failure/cancel, delete the temp file.
5. **Never modify, move, or delete input files.**
6. If the output folder is not writable (e.g. read-only media, Program Files), fail that task with a clear message suggesting *Choose folder…*.
7. Long paths: use `\\?\` prefixed paths internally. Unicode file names must work (test with accented and CJK names).

### 5.5 Configuration

- `config.toml` in `<LocalData>`: user settings (Settings page). Includes a `[menu]` section with enabled categories. The DLL reads only this section and caches it by file modified time.
- `state.toml` in `<LocalData>`: last-used options per action ("Remember these settings").
- Logs: `<LocalData>\logs\morph-YYYY-MM-DD.log` via `tracing` + `tracing-appender`, keep 7 days. The shell DLL logs only errors, to `shell.log`.

---

## 6. The shell extension (`morph-shell`) — the riskiest part

- `cdylib` built with the `windows` crate (`#[implement(IExplorerCommand)]`, `IEnumExplorerCommand`, `IClassFactory`). Exports `DllGetClassObject` and `DllCanUnloadNow`.
- **Root command** "Morph": `GetFlags` → `ECF_HASSUBCOMMANDS`; `GetIcon` → path to the package's icon (`morph.ico` beside the DLL); `GetState` → `ECS_HIDDEN` when the selection contains no supported/enabled file.
- **Subcommands** are built from the static menu table in `morph-formats` (§3.1 logic), based on the categories present in the selection. Separators via `ECF_ISSEPARATOR`.
- `Invoke` → write request JSON → `IApplicationActivationManager::ActivateApplication(AUMID, args, AO_NONE)`.
- **Must never crash Explorer:** wrap every COM method body in `std::panic::catch_unwind`, return `E_FAIL` on panic, and log it. No `unwrap()` in this crate (enforce with `#![deny(clippy::unwrap_used, clippy::expect_used)]`).
- **Must be fast:** `GetState`/`EnumSubCommands` only look at path extensions (`SIGDN_FILESYSPATH`). No file I/O except the cached config read. Target < 10 ms.
- **Must be small:** depends only on `windows`, `morph-formats`, `serde_json`, `uuid`. No image/PDF/FFmpeg crates.
- Fixed CLSID for the root command (generate once with `uuidgen`, store in `morph-formats` and the manifest).

### MSIX manifest essentials

```xml
<Package ... xmlns:desktop4=".../windows10/desktop/4" xmlns:desktop5=".../windows10/desktop/5"
             xmlns:com=".../appx/manifest/com/windows10" xmlns:uap=".../appx/manifest/uap/windows10"
             xmlns:rescap=".../appx/manifest/foundation/windows10/restrictedcapabilities">
  <Applications>
    <Application Id="Morph" Executable="Morph.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Morph" ... />
      <Extensions>
        <desktop4:Extension Category="windows.fileExplorerContextMenus">
          <desktop4:FileExplorerContextMenus>
            <desktop5:ItemType Type="*">
              <desktop5:Verb Id="MorphMenu" Clsid="{ROOT-CLSID}" />
            </desktop5:ItemType>
          </desktop4:FileExplorerContextMenus>
        </desktop4:Extension>
        <com:Extension Category="windows.comServer">
          <com:ComServer>
            <com:SurrogateServer DisplayName="Morph Shell Extension">
              <com:Class Id="{ROOT-CLSID}" Path="morph_shell.dll" ThreadingModel="STA" />
            </com:SurrogateServer>
          </com:ComServer>
        </com:Extension>
        <!-- optional: uap:FileTypeAssociation is NOT used; we don't want to become a default app -->
      </Extensions>
    </Application>
  </Applications>
  <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
```
(Use `Type="*"` and filter in `GetState`, so the extension list lives in one place: `morph-formats`.)

**Dev loop:** enable Windows Developer Mode → `dev-register.ps1` runs `Add-AppxPackage -Register .\layout\AppxManifest.xml` (no signing needed) → restart Explorer (`Stop-Process -Name explorer`) to reload the DLL.

---

## 7. Engines & libraries

Check the current version of every crate before adding it (use the latest stable). Pin the downloaded binaries by version + SHA-256 in `fetch-deps.ps1`.

| Need | Choice | Notes |
|---|---|---|
| GUI | `slint` (Fluent style) | Light/dark from system. |
| Tray icon | `tray-icon` (+ `muda` menu) | Only during background jobs. |
| Toasts | `windows` crate `Windows.UI.Notifications` | Works because the app has package identity. Toast button relaunches `Morph.exe --open-folder <path>` / `--show-results <job>`. |
| Image decode/encode (base) | `image` | JPEG/PNG/GIF/BMP/TIFF/ICO/TGA/QOI decode; BMP/TIFF/GIF encode. |
| JPEG encode | `mozjpeg` | Smaller files at the same quality. |
| WebP lossy + lossless | `webp` (libwebp) | `image`'s own WebP encoder is lossless-only. |
| AVIF encode | `ravif` | Pure Rust (rav1e). |
| AVIF decode | `libheif-rs` (libheif + libaom via vcpkg, BSD-2) | *Changed in M2 (approved):* vcpkg's libheif can't use dav1d, so AVIF goes through libheif + aom too. |
| HEIC/HEIF decode | `libheif-rs` (libheif + libde265 via vcpkg, LGPL; fine with GPL-3) | Linked statically (`x64-windows-static`), no DLLs to ship. x265 (encoder) deliberately not included. |
| SVG input | `resvg` | |
| PNG optimize / quantize | `oxipng`, `imagequant` (GPL-3, compatible) | |
| Resize | `fast_image_resize` | Lanczos3. |
| EXIF / metadata | `kamadak-exif` (read), `img-parts` (copy/strip EXIF, ICC, XMP) | |
| ICO writing | `ico` | Multi-size. |
| PDF render + text + page copy | `pdfium-render` + bundled `pdfium.dll` (bblanchon/pdfium-binaries) | Render pages, extract text, merge/split by page import. |
| PDF compress / low-level | `lopdf` | Walk image XObjects, downscale + re-encode, rewrite streams, prune objects. |
| Images → PDF pages | `pdfium-render` (or `printpdf`) | |
| Video/audio | bundled **FFmpeg GPL build**: morph-ffmpeg 9.0.2-1, a trimmed shared build with only what Morph uses (`ffmpeg.exe`, `ffprobe.exe` + 7 DLLs, ~62 MB; https://github.com/renangomide/morph-ffmpeg publishes the build scripts and complete source; replaced the 231 MB gyan.dev full build on 2026-10-23) | Spawn as a child process with `CREATE_NO_WINDOW`; parse `-progress pipe:1`; duration from `ffprobe -show_format -of json`; kill on cancel. |
| GPU encoder detection | 1-second test encode per encoder (`h264_nvenc`, `h264_qsv`, `h264_amf`, + hevc variants) at first use | Cache result in `state.toml` with the FFmpeg version and driver key. |
| MS Office | COM late binding via `windows` crate `IDispatch` (small helper module) | Word: `Documents.Open` (ReadOnly, Visible=false) → `SaveAs2`/`ExportAsFixedFormat`. Excel: `Workbooks.Open` → `ExportAsFixedFormat`/`SaveAs`. PowerPoint: `Presentations.Open(WithWindow=false)` → `SaveAs`. Always `DisplayAlerts=false`, always `Close`+`Quit`. Timeout 180 s, then kill the process. Fallback if IDispatch is too painful: an embedded PowerShell script per app. |
| LibreOffice | `soffice.exe --headless --norestore --nolockcheck -env:UserInstallation=file:///<private profile> --convert-to <fmt> --outdir <tmp> <file>` | Private profile so it won't conflict with the user's open LibreOffice. Detect via registry (`HKLM\SOFTWARE\LibreOffice\UNO\InstallPath`) and default paths. |
| CSV/XLSX native | `calamine`, `rust_xlsxwriter`, `csv` | |
| HTML → Markdown | `htmd` | |
| Errors / logging | `thiserror`, `anyhow` (app only), `tracing` | |
| Paths | `dunce`, `directories` | |
| Concurrency | `rayon`, `crossbeam-channel` | |

**Bundled in the MSIX:** `Morph.exe`, `morph-cli.exe`, `morph_shell.dll`, `ffmpeg.exe`, `ffprobe.exe`, `pdfium.dll`, native DLLs from vcpkg (libheif etc.), icons, `THIRD_PARTY_NOTICES.md`. Expected size: ~300–350 MB installed (FFmpeg ~240 MB of it), ~120–150 MB package (updated 2026-10-20).

---

## 8. Licensing

- Project: **GPL-3.0-or-later**. Put the SPDX header in every source file.
- FFmpeg GPL build: OK under GPL-3. Ship its license and a link to its exact source in `THIRD_PARTY_NOTICES.md`.
- PDFium (BSD-3/Apache-2.0), libheif/libde265 (LGPL-3), dav1d (BSD-2), imagequant (GPL-3): all compatible. List them all in `THIRD_PARTY_NOTICES.md` and the About page.
- Do **not** bundle Ghostscript (AGPL; large). If it is installed, it could be offered later as an optional "maximum PDF compression" engine (v2).
- LibreOffice and MS Office are not bundled; they are used only if the user has them.

---

## 9. Packaging, signing, distribution

- `make-msix.ps1`: `cargo build --release` → stage `target/release-layout/` (not the dev-registered `layout/`; binaries + vendor + Assets + manifest with version) → `makeappx pack` → `signtool sign`.
- **Version** comes from the workspace `Cargo.toml` (`x.y.z.0` in the manifest).
- **Signing roadmap:**
  1. Dev: loose registration (no signing) or self-signed cert via `dev-cert.ps1`.
  2. GitHub releases: MSIX signed with a trusted cert. Free/cheap options for open source: **SignPath Foundation** (free for OSS) or **Azure Trusted Signing**. Until then, publish the `.cer` and a 2-step install guide in the README.
  3. Optional: **Microsoft Store** (free individual developer account; the Store signs the package) and then **winget**.
- **M10 decisions (user, 2026-10-22):** first public version `0.1.0` (changed from 0.9.0 on 2026-10-23: "reset the count, it's official now"; 0.9.0 was only a test build). The C runtime is linked statically (no VC++ redist needed for our binaries). `Morph.exe` embeds the app icon. First distribution = a zip with `Morph.msix` + `Morph.cer` (self-signed) + `Install.cmd` (run as admin: trusts the cert in TrustedPeople, installs the package). The GitHub repo is created last. A custom smaller FFmpeg build is researched before release (GPL source obligation). Package identity/publisher: no personal name: name `OpenMorph.Morph`, publisher `CN=OpenMorph`, publisher display name "OpenMorph", app display name "Morph", application Id `Morph`, root CLSID unchanged. The zip (`packaging/make-release.ps1` -> `target/release-zip/Morph-<version>-x64.zip`) holds `Morph.msix`, `Morph.cer`, `Install.cmd` (asks for admin only to put the cert in LocalMachine\TrustedPeople, then installs for the signed-in user; `/check` = dry run), `Uninstall.cmd` (optionally removes the cert) and `README.txt`.
- Uninstall = standard Windows "Remove app". The MSIX removes the menu, files, and data automatically.

---

## 10. Errors, robustness, performance

- Each task fails independently. A failure never stops the batch.
- User-facing errors are one short plain-English sentence (*"This file seems to be damaged."*, *"LibreOffice took too long and was stopped."*). **Details** shows the technical error + engine stderr tail (last 30 lines), with **Copy** button.
- Cancellation is checked between steps, inside image loops (per row/tile where possible), and kills child processes (FFmpeg, soffice, Office COM) immediately.
- Temp files go in `<LocalData>\tmp\<job-id>\` and are cleaned on exit and on next start.
- **Performance targets:**
  - Menu shows with no visible delay (< 10 ms in the DLL).
  - Conversion window visible < 500 ms after click (cold start).
  - 12 MP JPG → WebP q80 < 1.5 s on a modern 8-core CPU; batches run in parallel.
  - 100-page PDF → PNG @150 DPI < 30 s.
- Memory: very large images (> 100 MP) are processed with a warning note. Hard limit about 1 gigapixel ("This image is too large to process"), so one file can't exhaust RAM (approved 2026-10-19). Tasks start only while their estimated memory fits 60% of free RAM.

---

## 11. Testing & CI

- **Unit tests** (`morph-core`): output naming & collisions, option defaults, target-size math (images & video), page-range parsing (`1-3,7,10-`), CSV delimiter detection, extension → category → menu table.
- **Integration tests** with `tests/fixtures/` (tiny files for every input format, plus unicode-named ones). Each converter test checks the output **opens correctly**: image decodes with the expected size/format; PDF has the expected page count; `ffprobe` reports the expected codecs; DOCX/XLSX open with `calamine`/zip check.
- Tests that need Office/LibreOffice/GPU are marked `#[ignore]` unless the engine is detected (`MORPH_TEST_OFFICE=1`).
- **CLI** (`morph-cli convert <files…> --action image.convert --format webp --quality 80`) mirrors the request/options model, so every converter can be tested without the GUI.
- **Manual test checklist** (`docs/manual-tests.md`): menu appears for each type; mixed selection; 500-file batch; cancel mid-video; close window → tray → toast; uninstall removes menu.
- **GitHub Actions** (`windows-latest`): `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, build MSIX artifact (test-signed). `release.yml` on tag `v*`: build, sign (when cert is available), create a GitHub Release with `.msix` + `.cer` + notes.

---

## 12. Milestones (build in this order)

Each milestone ends with: code compiles, `fmt` + `clippy` clean, tests pass, a short note in the milestone log.

1. **Spike: menu → window (de-risk first).** Workspace skeleton; `morph-formats`; `morph-shell` with root + static submenu; minimal Slint window that lists the received files; MSIX manifest + `dev-register.ps1`. ✅ *Right-click 3 JPGs on Windows 11 → "Morph ▸ Convert to PNG…" appears in the modern menu → window opens listing the 3 files.*
2. **Core + images.** Job/scheduler/events/cancellation, output naming, all image actions, CLI. ✅ *All image fixtures convert to every output format via CLI; tests pass.*
3. **Conversion window + progress view.** Real option panels for images, Remember settings, progress view, results, toast, tray/background. ✅ *50 photos → WebP from right-click with live progress and a toast at the end.*
4. **PDF.** PDFium + lopdf; compress, to images, text, split, merge (incl. images → PDF, reorder list). ✅ *Compress Medium makes an image-heavy fixture ≥ 40% smaller and still valid.*
5. **Office** (moved before video, approved 2026-10-19). Engine detection, MS Office COM, LibreOffice runner, native CSV/XLSX, Markdown, text-like inputs (md/txt/html), doc/slides → images, PDF → Word. ✅ *DOCX/XLSX/PPTX → PDF works with each engine present; "missing engine" banner shows when neither is installed.*
6. **Video & audio.** FFmpeg in `fetch-deps.ps1`, FFmpeg runner with progress, remux fast-path, presets, target-size two-pass, GPU detection, Compress "Best (automatic)", Extract audio, Video → GIF. ✅ *A 200 MB clip compresses to ≤ 10 MB with the Discord preset and plays.*
7. **Mixed selections + main window.** Grouped tabs, "Combine into one PDF", drag & drop page, Settings page (all options from §3.4 working, menu respects toggles). ✅ *Mixed selection of 4 types converts in one job.*
8. **Polish (no visual changes).** Third-party licenses (`THIRD_PARTY_NOTICES.md`, shipped `licenses\`, list in About), logging, error messages pass, README build guide, CI/release workflows (build, test, package MSIX), `docs/manual-tests.md` written. ✅ *CI builds a signed-test MSIX from a clean checkout; every bundled library's license is shipped.*
9. **UI/UX adjustments** (brought by the user, 2026-10-21). Visual and interaction changes per the design reference (§3.2) and the §3.2/§3.3 decisions; app icon drawn by an agent for the user to approve (Explorer menu icon, app icon, MSIX tiles). ✅ *Every screen in the prototype matches in light and dark mode; all SPEC options still reachable.*
10. **Release.** README install guide with screenshots, `docs/manual-tests.md` run through. ✅ *Fresh Windows 11 machine: install MSIX → everything in the checklist works → uninstall leaves nothing behind.*

---

## 13. Risks & notes for the builder

- **The modern menu is the hardest part.** Do Milestone 1 before anything else. If COM surrogate + `ActivateApplication` causes trouble, a fallback is to launch `Morph.exe` by its full path inside the package folder (resolved from the DLL's own module path).
- **Native builds** (libheif, dav1d, libwebp, mozjpeg) can be painful on Windows. Prefer vcpkg. Prefer WIC decoders as a fallback over spending days on a build.
- **Office COM** can hang on modal dialogs (password-protected files, repair prompts). Always open read-only, with alerts off, and with a hard timeout + process kill. Password-protected files: fail with *"This file is password-protected."*
- **LibreOffice** can't run two conversions at once with the same profile. Keep the Office queue at 1.
- **PDF → Word** quality is limited by the engine. Set expectations in the UI.
- **Unsigned MSIX** can't be installed by normal users without trusting a certificate. Solve signing before announcing a public release.
- Explorer keeps the DLL loaded. During development, restart Explorer after each rebuild of `morph-shell`.

---

## 14. Defaults I chose (change if you disagree)

- Windows 11 x64 only for v1 (Windows 10 best effort; no ARM64).
- Placeholder name "Morph", fixed menu item names in English as listed in §3.1.
- Remember-last-settings on by default.
- "Keep metadata" on by default for Convert/Compress/Resize; off only in Clean.
- Output collisions get ` (1)`, never overwrite, no "replace original" option.
- Tray icon only appears while a background job is running.
- No telemetry, no network access except the "Download LibreOffice" link (opens the browser).
