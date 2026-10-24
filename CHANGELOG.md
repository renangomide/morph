# Changelog

## 0.1.0 (2026-10-23)

First public release.

### Right-click menu

- One **Morph ▸** entry in the modern Windows 11 File Explorer menu. Its items depend on the selection: the most common targets per file type, plus **More options…**.
- Mixed selections: **Convert N files…** (one section per type, each with its own settings) and, for images and PDFs only, **Combine into one PDF…**.
- File types turned off in Settings disappear from the menu. The menu is built from file extensions only, so it shows instantly.

### Conversions

- **Images** (JPG, PNG, WebP, AVIF, HEIC/HEIF, GIF, BMP, TIFF, ICO, TGA, QOI, SVG): convert to JPG, PNG, WebP, AVIF, BMP, TIFF, GIF, ICO or PDF; compress (*Best (automatic)* with a perceptual check, quality, or target size); resize; remove metadata or only the GPS location; rotate and flip. EXIF orientation and ICC profiles are respected.
- **PDF**: compress (Low / Medium / High / Custom); convert to Word, Markdown, HTML, plain text or images; split; merge PDFs and images into one file with bookmarks.
- **Video**: convert to MP4, MKV, WebM, MOV, AVI (stream copy when possible); compress (*Best (automatic)* with VMAF, Balanced, Smaller, Discord ≤ 10 MB, Email ≤ 25 MB, custom size); extract audio; convert to GIF. GPU encoders (NVENC, Quick Sync, AMF) are used when available.
- **Audio**: convert to MP3, M4A, WAV, FLAC, OGG, Opus, keeping tags and cover art.
- **Documents** (Word, Excel, PowerPoint, OpenDocument, RTF, CSV, Markdown, TXT, HTML): to PDF, Markdown, images and between formats, through Microsoft Office or LibreOffice. CSV ↔ XLSX and the Markdown/TXT/HTML conversions to Markdown, HTML or TXT are native and need neither. Native readers for DOCX, PPTX, spreadsheets and PDF → Markdown.

### App

- Conversion window with options per action, **Remember these settings**, and a progress view with per-file results, details for errors, and **Adjust options** to run again.
- Jobs can continue in the background (tray icon) with a notification when done.
- Start menu window: drop zone and Settings (file types, defaults, output folder, Office engine, performance, notifications, appearance, About with logs and third-party licenses).
- Light and dark theme, Windows accent colour.
- `morph-cli.exe`: the same conversions from the command line.

### Safety

- Originals are never modified; results get new names and collisions get ` (1)`.
- One failed file never stops a batch.
- Works offline: no network access, no telemetry.

### Install

- Zip with `Morph.msix` (self-signed by "OpenMorph"), `Morph.cer`, `Install.cmd` and `Uninstall.cmd`. Windows 11 x64.
- Bundles a trimmed GPL FFmpeg 9.0.2 build ([morph-ffmpeg](https://github.com/renangomide/morph-ffmpeg)) and PDFium.
