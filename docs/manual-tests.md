# Manual test checklist

Run on a **fresh Windows 11 machine** (or a clean VM) with the release MSIX, before each release (SPEC §11, §12 item 10).
Tick each box when the expected result matches. Note the Windows build (`winver`) and the Morph version at the top of the run.

- Windows build: ______  Morph version: ______  Date: ______  Office installed: yes / no  LibreOffice installed: yes / no

Package identity (from `packaging/common.ps1`): name `OpenMorph.Morph`, publisher `CN=OpenMorph`, package family name **`OpenMorph.Morph_kxvcpsh9twvzt`** (pinned by a `morph-formats` test).

Test files: copy `tests\fixtures\` from the repo to `C:\MorphTest\` (it has every input format, Unicode names and broken files). Add a few of your own photos, a long video (~200 MB, 1080p, a few minutes), a DOCX, an XLSX and a PPTX.

## 0. Install

Use the release zip `Morph-<version>-x64.zip` (`packaging\make-release.ps1`), copied to a folder **with a space** in its path (e.g. `C:\Users\<you>\Downloads\Morph test\`), on a PC **without** Developer Mode.

- [ ] **Extract.** Right-click the zip → Extract All. (If Windows marks it as downloaded: Properties → Unblock first.)
- [ ] **Dry run.** In a terminal in the extracted folder: `.\Install.cmd /check`. Expected: package `OpenMorph.Morph`, publisher and certificate `CN=OpenMorph`, "Check passed"; nothing is changed.
- [ ] **Install (standard double-click).** Double-click `Install.cmd`. Expected: if SmartScreen warns, More info → Run anyway; one UAC prompt (trust the certificate); then "Morph 0.1.0.0 is installed."; the window waits for a key. "Morph" is in the Start menu with its icon. No Explorer restart or reboot needed.
- [ ] **Cert store.** `certlm.msc` → Trusted People → Certificates has `OpenMorph`; **Trusted Root Certification Authorities does not**.
- [ ] **UAC "No".** Uninstall, remove the cert (Uninstall.cmd → Y), run Install.cmd and click **No** at UAC. Expected: a red plain-English message, nothing installed, the window stays open.
- [ ] **Run as administrator.** Right-click `Install.cmd` → Run as administrator. Expected: no second prompt, installs, reports success (re-running over an installed copy also succeeds).

## 1. Right-click menu (SPEC §3.1, §12 item 1)

In each case right-click the selection in Explorer (the modern Windows 11 menu, not "Show more options").

- [ ] **3 JPGs.** Expected: a **Morph ▸** entry with the app icon. Submenu: Convert to JPG… / PNG… / WebP… / AVIF… / PDF…, separator, Compress…, Resize…, Remove metadata…, separator, More options…. "Convert to JPG…" is shown even when every file is already a JPG (re-encode).
- [ ] **Click "Convert to PNG…".** Expected: the conversion window opens within about half a second, with PNG pre-selected and the 3 files listed (Show files).
- [ ] **1 PDF.** Expected: Convert to Word… / Markdown… / images…, Compress…, separator, More options…. No "Merge".
- [ ] **2+ PDFs.** Expected: same plus **Merge into one PDF…**.
- [ ] **Videos only** (MP4, MOV, MKV). Expected: Convert to MP4… / MKV… / WebM…, separator, Compress…, Extract audio…, Convert to GIF…, separator, More options….
- [ ] **Audio only** (MP3, WAV, FLAC). Expected: Convert to MP3… / M4A… / WAV… / FLAC…, separator, More options….
- [ ] **Word, Excel, PowerPoint, text** (each alone: DOCX; XLSX; CSV; PPTX; MD). Expected: the top 4 targets per type from SPEC §4.6, PDF first (DOCX: PDF, Markdown, ODT, TXT; CSV: PDF, XLSX, Markdown, ODS; a format is never offered for files already in it), then More options….
- [ ] **Mixed selection** (3 photos + 1 PDF + 1 video). Expected: **Convert 5 files…** only (no "Combine", because of the video).
- [ ] **Images + PDFs only** (2 photos + 1 PDF). Expected: **Convert 3 files…** and **Combine into one PDF…**.
- [ ] **16 or more mixed files** (e.g. 10 photos + 6 PDFs + 1 video, 17 in total). Expected: the entry reads **Convert files…** (no number). Clicking it opens the window, which shows the real count (17) on grouped tabs. Repeat 3 times: the click is never lost.
- [ ] **Same-type big selection** (500 JPGs in one folder, Ctrl+A). Expected: the menu appears with no visible delay and shows the normal image submenu; "Convert to WebP…" opens the window with all 500 files.
- [ ] **Unsupported only** (a folder, an `.exe`, a `.zip`). Expected: no Morph entry at all.
- [ ] **Unsupported in a mixed selection** (2 JPGs + 1 `.exe`). Expected: the image submenu; in the window the `.exe` is listed under **Skipped**.
- [ ] **Unicode and spaces** (fixtures with Unicode names, a folder named `Fotos de férias`). Expected: menu and conversion work; output names keep the characters.
- [ ] **Speed.** Right-click a large folder selection repeatedly. Expected: Explorer never freezes or crashes; the menu opens as fast as without Morph.

## 2. Images: CLI-free end-to-end (§12 items 2–3)

- [ ] **50 photos → WebP from right-click.** Expected: live progress per file, the window can be closed to the tray while it runs, a toast at the end. Outputs are next to the originals; originals unchanged (size and modified date).
- [ ] **Name collision.** Convert the same JPG to PNG twice. Expected: the second output is `name (1).png`; nothing is overwritten.
- [ ] **HEIC, AVIF, SVG inputs** → JPG. Expected: correct images, correct orientation.
- [ ] **Compress, Resize, Remove metadata (all / GPS only).** Expected: each produces a new file; GPS-only keeps the camera info but Properties ▸ Details shows no GPS.
- [ ] **Broken file** in a batch (a `broken.jpg` fixture + 3 good ones). Expected: 3 succeed, 1 fails with a short plain-English message; **Details** shows the technical error and **Copy** copies it. The batch is not stopped.
- [ ] **Remember these settings.** Set WebP quality 60 with Remember on, convert, open the window again. Expected: 60 is pre-filled.

## 3. PDF (§12 item 4)

- [ ] **Compress Medium** on an image-heavy PDF. Expected: output at least 40% smaller, opens in Edge with all pages.
- [ ] **Convert to images** (PNG, 150 DPI). Expected: one image per page, numbered.
- [ ] **Merge 3 PDFs**, reorder them in the list first. Expected: one PDF in the chosen order.
- [ ] **More options… → Split** with range `1-2,4`. Expected: the selected pages as separate files (or one file, per the option).
- [ ] **Combine into one PDF** (2 photos + 1 PDF). Expected: one PDF, photos as pages, in list order.
- [ ] **PDF → Word / Markdown / text.** Expected: files open in Word / Notepad with the text.

## 4. Office (§12 item 5)

- [ ] **With Microsoft Office installed:** DOCX, XLSX, PPTX → PDF. Expected: correct PDFs; Settings ▸ Office engine shows "Microsoft Word … found".
- [ ] **With LibreOffice only** (Settings ▸ Office engine: LibreOffice, or uninstall Office): the same three → PDF. Expected: correct PDFs.
- [ ] **Neither engine installed** (fresh machine before installing Office/LibreOffice): right-click a DOCX → Convert to PDF…. Expected: the banner **"To convert documents, install Microsoft Office or LibreOffice (free)."** with **Download LibreOffice** and **Check again**; Convert is not possible for that file. With a DOCX + a CSV selected, the banner says 1 file will be skipped and the CSV still converts.
- [ ] **Check again** after installing LibreOffice (without closing the window). Expected: the banner disappears and the conversion works.
- [ ] **CSV → XLSX, XLSX → CSV, MD → DOCX/HTML** with no Office engine. Expected: work natively.

## 5. Video and audio (§12 item 6)

- [ ] **Discord preset.** Right-click the ~200 MB clip → Compress… → preset **Discord (10 MB)** → Convert. Expected: output ≤ 10 MB, plays in the Media Player with sound, progress bar moves smoothly.
- [ ] **Convert to MP4 from an MKV with H.264** (remux). Expected: finishes almost instantly, plays.
- [ ] **Extract audio** → MP3. Expected: an MP3 that plays.
- [ ] **Convert to GIF** (short clip). Expected: an animated GIF.
- [ ] **Audio:** WAV → MP3 192 kbps, FLAC → M4A. Expected: files play; MP3 Properties shows 192 kbps.
- [ ] **Cancel mid-video.** Start a long compress, click **Cancel**. Expected: stops within a second or two; no partial output and no `.~morph-*.tmp` left in the folder; Task Manager shows no `ffmpeg.exe` left running.

## 6. Window, tray, toast

- [ ] **Run in background / close during a job.** Expected: the window hides, a Morph tray icon appears; the job continues. Clicking the tray icon brings the progress back.
- [ ] **Toast at the end.** Expected: a Windows notification "done" with the result; clicking it opens the results (or the folder). With Settings ▸ Notifications off: no toast.
- [ ] **Results view.** Open, Show in folder, and Details work for each row.
- [ ] **Open folder when done** (Settings ▸ Output on). Expected: the output folder opens after the job.

## 7. Main window, drag & drop, Settings (§3.4, §12 item 7)

- [ ] **Start menu → Morph.** Expected: the main window with Convert and Settings pages.
- [ ] **Drag & drop** a mix of 4 types (photo, PDF, video, DOCX) onto **"Drop files here or click to browse"**. Expected: the same grouped conversion window as the right-click menu; all 4 convert in one job.
- [ ] **Click to browse.** Expected: a file picker; picked files open the conversion window. A dropped folder shows as Skipped.
- [ ] **File types toggles.** Turn **Video** off in Settings. Right-click an MP4: no Morph entry. Right-click MP4 + JPG: the image submenu only, the MP4 ignored. Turn it back on: the video menu returns (no Explorer restart needed). Repeat for Images, PDF, Audio, Office.
- [ ] **Defaults.** Change JPEG quality to 70 and the video preset; open a new conversion. Expected: the new defaults are pre-filled.
- [ ] **Output: fixed folder.** Choose a folder; convert. Expected: outputs land there.
- [ ] **Keep original file dates** on. Expected: outputs have the input's modified date.
- [ ] **Appearance** System / Light / Dark. Expected: the window follows the choice.
- [ ] **About.** Version matches the release; **Open log folder** opens `…\LocalState\logs`; the third-party components list is shown, and `THIRD_PARTY_NOTICES.md` plus a `licenses` folder are in the install folder.

## 8. Uninstall leaves nothing behind (§9, §12 item 10)

- [ ] **Uninstall:** double-click `Uninstall.cmd` (answer **Y** to also remove the certificate), or Settings ▸ Apps ▸ Installed apps ▸ Morph ▸ Uninstall.
- [ ] **Menu gone.** Right-click a JPG (no Explorer restart). Expected: no Morph entry. After a sign-out/sign-in: still none.
- [ ] **Start menu.** Expected: no Morph entry.
- [ ] **Package gone.** PowerShell: `Get-AppxPackage OpenMorph.Morph` → no output.
- [ ] **Data gone.** `Test-Path "$env:LOCALAPPDATA\Packages\OpenMorph.Morph_kxvcpsh9twvzt"` → `False` (this held config, logs, requests, results and tmp).
- [ ] **No unpackaged data.** `Test-Path "$env:LOCALAPPDATA\Morph"` → `False` (only `cargo run` dev builds use it).
- [ ] **Program files gone.** Admin PowerShell: `Get-ChildItem 'C:\Program Files\WindowsApps' -Filter 'OpenMorph.Morph_*'` → no output.
- [ ] **No leftovers in output folders.** `Get-ChildItem C:\MorphTest -Recurse -Force -Filter '.~morph-*'` → no output.
- [ ] **`morph:` links.** Win+R → `morph:test`. Expected: Windows asks which app to use (no Morph handler left).
- [ ] **Converted files stay.** Expected: outputs you created are still there (they are your files, not app data).
- [ ] **Certificate** (if not removed by Uninstall.cmd): admin PowerShell `Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object Subject -eq 'CN=OpenMorph' | Remove-Item`.
