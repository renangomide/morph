<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Test fixtures

Tiny files for every image input format (SPEC section 4.1), plus edge cases. All together about 130 KB.
Regenerate with `scripts\make-fixtures.ps1` (needs ImageMagick 7 and ffmpeg with libx265 on PATH).
`sample.svg` is hand-written and is not touched by the script.

"Size" is in pixels as stored in the file.

## One file per input format

| File | Size | What it tests |
|---|---|---|
| `sample.jpg` | 96x64 | Baseline JPEG, no metadata. |
| `sample.jpeg` | 96x64 | Same bytes as `sample.jpg`, `.jpeg` extension. |
| `sample.jfif` | 96x64 | Same bytes as `sample.jpg`, `.jfif` extension. |
| `sample.png` | 96x64 | RGBA PNG with fully and partly transparent areas (transparent to JPG/BMP gets the background color). |
| `sample.webp` | 96x64 | Lossy WebP. |
| `sample.avif` | 96x64 | AV1 AVIF. |
| `sample.heic` | 96x64 | HEVC HEIC (major brand `heic`), built from an x265 intra frame. |
| `sample.heif` | 96x64 | Same HEVC picture, major brand `mif1`, `.heif` extension. |
| `sample.bmp` | 96x64 | 24-bit BMP (v3 header). |
| `sample.tif` | 96x64 | LZW TIFF, opaque. |
| `sample.tiff` | 96x64 | LZW TIFF with alpha, `.tiff` extension. |
| `sample.gif` | 64x48 | Animated GIF, 3 frames (red, green, blue); only the first frame is converted. |
| `sample.ico` | 48, 32, 24, 16 px squares | Multi-size ICO with alpha; the largest (48x48) is used. |
| `sample.tga` | 96x64 | RLE-compressed Targa. |
| `sample.qoi` | 96x64 | QOI. |
| `sample.svg` | 100x50 (viewBox) | SVG; rasterized at the default width of 1024, so 1024x512. |

## Metadata and orientation

| File | Size | What it tests |
|---|---|---|
| `orientation6.jpg` | 64x32 stored | EXIF orientation 6 (rotate 90 CW). Must display and convert as 32x64; orientation tag reset to 1 on re-encode. |
| `exif-gps.jpg` | 96x64 | EXIF with Make "Morph Test", Model "Fixture Cam", DateTime, DateTimeOriginal, ISO 200 and GPS (37 46 29.7 N, 122 25 9.9 W). For "remove all metadata" and "remove only GPS". |
| `icc-profile.jpg` | 64x48 | Embedded ICC profile (sRGB IEC61966-2.1, ~3 KB). Must be kept on re-encode. |

## Names and errors

| File | Size | What it tests |
|---|---|---|
| `Férias 夏.jpg` | 64x48 | Unicode name with accent, CJK character and a space. |
| `café.png` | 48x48 | Unicode name; RGBA PNG. |
| `broken.png` | - | First 120 bytes of `sample.png` (truncated). Must fail with a plain-English "damaged image" error and never leave output behind. |

## Not covered

Nothing from the SPEC list is missing. Notes:

- ImageMagick cannot write HEIC/HEIF (read only), so those two are packaged by the script itself: an x265 (ffmpeg) intra frame wrapped in a minimal HEIF container. ImageMagick (libheif) decodes both.
- No exiftool is needed: EXIF blocks are built by the script and inserted as an APP1 segment.
- The ICO stops at 48 px to stay small; a 256 px entry would make it far bigger than 30 KB.

## PDF (SPEC section 4.2)

Built from code, no tools needed: `cargo run -p morph-core --example make_pdf_fixtures`
(`crates/morph-core/examples/make_pdf_fixtures.rs`: lopdf + fixed seeds, so the bytes are the same on every run,
except `password.pdf`, whose encryption salt is random). All pages are US Letter. Text uses Helvetica (not embedded).

| File | Pages | What it tests |
|---|---|---|
| `text-3pages.pdf` | 3 | Text only. Each page says "Text - Page N of 3" and "Accented words: café, relatório, ação." (text extraction, split, merge). |
| `images.pdf` | 3 | Image-heavy, ~2.1 MB: one 2000x1500 photo-like JPEG (q92) per page, placed at 300 DPI, plus a caption. Compress Medium must make it at least 40% smaller. |
| `password.pdf` | 1 | Encrypted (RC4 128-bit), user password `morph`. Must fail with "This PDF is password-protected." |
| `scanned.pdf` | 2 | Only a grey-scale picture of text on each page, no text layer. Extract text: "No text found (scanned PDF?)". |
| `Relatório 報告.pdf` | 1 | Unicode name with accent, CJK character and a space. |
| `pages-100.pdf` | 100 | Text pages, for the performance target (100 pages to PNG at 150 DPI in under 30 s). |
| `broken.pdf` | - | First 300 bytes of `text-3pages.pdf`. Must fail with a plain-English "damaged" error. |

## Office and text-like documents (SPEC section 4.5)

Built from code, no Office or LibreOffice needed: `cargo run -p morph-core --example make_office_fixtures`
(`crates/morph-core/examples/make_office_fixtures.rs`). DOCX/PPTX/ODT are minimal hand-written zips, XLSX comes
from rust_xlsxwriter; zip dates and document dates are fixed, so the bytes are the same on every run. The text
files are marked `-text` in `.gitattributes`, so CRLF and the BOM survive checkout.

| File | What it is | What it tests |
|---|---|---|
| `sample.docx` | 1 page (Letter): Heading 1/2 styles, accented and CJK text, 3-item bullet list, 3x3 bordered table, inline picture (`sample.png`) | DOCX → PDF/ODT/RTF/TXT/HTML/Markdown, pages → images. LibreOffice → PDF: 1 page. |
| `Relatório 報告.docx` | Same bytes as `sample.docx` | Unicode name with accent, CJK and a space. |
| `broken.docx` | First 300 bytes of `sample.docx` (truncated zip) | Must fail with a plain-English "damaged" error, nothing left behind. |
| `sample.odt` | Heading, accented/CJK text, 2-item list, 2x2 table | ODT input (Word-like). 1 page as PDF. |
| `sample.xlsx` | 2 sheets: `Sales` (header + 4 rows, numbers, `=B+C` formulas, "São Paulo", "東京") and `Notes` (text with `;`, `"` and `,`) | XLSX → CSV (one file per sheet), → PDF (2 pages with LibreOffice), → ODS. |
| `sample.csv` | Comma-separated, UTF-8 without BOM, CRLF, quoted fields with `""` and commas, CJK | CSV → XLSX, delimiter auto-detect. |
| `semicolon-bom.csv` | Same data, Excel-Europe style: UTF-8 BOM, `;` separators, decimal comma (`1234,5`) | BOM handling, `;` detection. |
| `sample.pptx` | 3 slides (4:3), each with a title "Slide N of 3", an accented/CJK line and a coloured box | PPTX → PDF (3 pages), → ODP, slides → images. |
| `sample.md` | Headings, pipe table, fenced `rust` code block, relative image `sample.png`, list with emphasis and a link | Markdown → HTML/TXT/DOCX/PDF/ODT. |
| `sample.html` | UTF-8 page with `<style>`, `<script>` (must be dropped), heading, list, table, relative image `sample.png`, a link and a remote image (never fetched) | HTML → Markdown/TXT/DOCX/PDF/ODT. |
| `sample.txt` | CRLF, two paragraphs, accented/CJK text, `<`, `&`, `*`, `#` | TXT → HTML (escaping, paragraphs) / Markdown. |

| `sample.doc` | `sample.docx` saved by LibreOffice 26.8 as "MS Word 97" | Legacy Word input; not encrypted. |
| `password.doc` | Same, saved with password `secret` (RC4, FIB `fEncrypted`) | Password check for old `.doc` before any engine starts. |
| `sample.xls` | `sample.xlsx` saved by LibreOffice 26.8 as "MS Excel 97" | Legacy Excel input; not encrypted. |
| `password.xls` | Same, saved with password `secret` (`FILEPASS` record) | Encrypted old `.xls` (explains the engine failure). |

Not covered: an encrypted OOXML file (needs real encryption; the tests fake the OLE container) and `.ppt`
(LibreOffice can't write an encrypted `.ppt`; the test builds a minimal OLE file with an encrypted `Current User` atom).

## Two-way conversions and Markdown quality (SPEC section 4.6, M5.1)

Built from code, no tools needed: `cargo run -p morph-core --example make_twoway_fixtures`
(`crates/morph-core/examples/make_twoway_fixtures.rs`; fixed dates, same bytes on every run). The expected Markdown
for each file is asserted in `crates/morph-core/tests/` (`pdf_to_markdown.rs`, `docx_markdown.rs`, `office_markdown.rs`).

| File | What it is | What it tests |
|---|---|---|
| `structured.pdf` | 3 Letter pages, Helvetica/Helvetica-Bold (not embedded). Running header `Morph Field Report — 2026` and a bare page number footer on every page. P1: 24 pt title, 18 pt H1, 14 pt H2, paragraphs wrapping over lines (one hyphenated word `environ-`/`mental`, `café`), 3 `•` bullets (one wrapping). P2: two 14 pt headings, a `1.`–`3.` numbered list, a 3x4 table (bold header, text only, no rules). P3: 18 pt heading, then two columns (`Alpha …` left, `Beta …` right, same baselines) | PDF → Markdown/HTML reader: heading levels by size, paragraphs, de-hyphenation, lists, header/footer removal, table detection, column order. |
| `structured.docx` | Title style, custom style `My Chapter` based on Heading 1 (no outline level of its own), Heading 2, a Normal paragraph with `w:outlineLvl` 2, bold/italic runs, external hyperlink, footnote, nested bullets (3 levels) and nested numbering (decimal/letter), 3x3 table with `a\|b` and a line break in a cell, inline picture (`sample.png`) with alt text | DOCX → Markdown reader. LibreOffice opens it (→ PDF: 1 page). |
| `structured.pptx` | 3 slides (4:3) on a "Title and Content" layout, notes master and a notes page per slide. 1: title + bullets at levels 0/1/2. 2: title, a 3x3 table, and a text box that is first in the XML but lowest on the slide. 3: title, picture with alt text, text box with an external link | PPTX → Markdown reader: placeholders, levels, reading order by position, tables, alt text, links, notes. LibreOffice → PDF: 3 pages. |
| `structured.xlsx` | Sheet `Data`: header + 4 rows (text incl. CJK, whole numbers stored as floats, `0.00`-formatted floats, `yyyy-mm-dd` dates, `a\|b pipe`, a two-line cell, an empty cell), formatted empty cells below and to the right. Sheet `Summary`: formula with cached result 16, a number, a boolean. Sheet `Empty` | Sheets → Markdown: ints without `.0`, dates, escaping, empty edges trimmed, empty sheet skipped. |
| `structured.html` | UTF-8 page with `<style>`, `<script>`, `<noscript>`, `<header><nav>`, `<main><article>` (heading, inline formatting, link, nested list, table with `a\|b`, a `data:` PNG with alt text) and a `<footer>` with nav | HTML pre-clean + HTML → Markdown. |
