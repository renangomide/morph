// SPDX-License-Identifier: GPL-3.0-or-later
//! Main window text: drop zone, Settings page, About (SPEC §3.4).

// ── Navigation ───────────────────────────────────────────────────────────────

pub const NAV_CONVERT: &str = "Convert";
pub const NAV_SETTINGS: &str = "Settings";
/// Settings inside the conversion window: back to the selection.
pub const BACK: &str = "Back";

// ── Convert (drop zone) ──────────────────────────────────────────────────────

pub const DROP_TITLE: &str = "Drop files here or click to browse";
/// Under the drop zone title (`app` = the app name).
pub fn drop_hint(app: &str) -> String {
    format!(
        "Select one or many files of any type, or right-click files in File Explorer and choose {app}."
    )
}
/// The supported-type cards under "Browse files…": name and what they take.
pub const DROP_CARDS: [(&str, &str); 4] = [
    ("Images", "JPG, PNG, WebP, AVIF, HEIC, SVG"),
    ("PDF", "Compress, split, merge, to Word, Markdown"),
    ("Video & audio", "MP4, MKV, WebM, MP3, FLAC, GIF"),
    ("Documents", "DOCX, XLSX, PPTX, CSV, Markdown"),
];
/// Bottom of the navigation rail.
pub const NAV_TIP_TITLE: &str = "Right-click menu";
pub const NAV_TIP_TEXT: &str = "Right-click files in File Explorer for quick conversions.";
pub const BROWSE: &str = "Browse files…";
pub const BROWSE_TITLE: &str = "Choose files to convert";

// ── Settings ─────────────────────────────────────────────────────────────────

pub const SAVE_FAILED: &str =
    "Couldn't save your settings. Check that the disk isn't full and try again.";

pub const SETTINGS_DESCRIPTION: &str =
    "Right-click menu, default presets, where files go, and engines.";

pub const SECTION_FILE_TYPES: &str = "Right-click menu";
pub const FILE_TYPES_HINT: &str =
    "Turned-off types disappear from the right-click menu and are skipped when dropped.";
/// Type names (also used by the conversion window: "Skipped: Video is unchecked.").
pub const TYPE_IMAGES: &str = "Images";
pub const TYPE_PDF: &str = "PDF";
pub const TYPE_VIDEO: &str = "Video";
pub const TYPE_AUDIO: &str = "Audio";
/// The File types toggles (same order as `TYPE_DESCRIPTIONS`).
pub const MENU_TYPE_LABELS: [&str; 5] = [
    "Images",
    "PDF documents",
    "Video files",
    "Audio files",
    "Office and text documents",
];
/// Descriptions under the File types toggles (same order as above).
pub const TYPE_DESCRIPTIONS: [&str; 5] = [
    "JPG, PNG, WebP, AVIF, HEIC, SVG, ICO, TIFF, BMP, GIF",
    "Compress, convert to Word, Markdown or images, split, merge",
    "MP4, MKV, WebM, MOV, AVI: compress, extract audio, GIF",
    "MP3, M4A, WAV, FLAC, OGG, Opus",
    "Word, Excel, PowerPoint, CSV, Markdown, HTML, TXT",
];

pub const SECTION_DEFAULTS: &str = "Defaults";
pub const DEFAULTS_TITLE: &str = "Default quality and presets";
pub const DEFAULTS_HINT: &str =
    "What a conversion starts with when no settings are remembered for it.";
pub const JPEG_QUALITY: &str = "JPEG quality";
pub const WEBP_QUALITY: &str = "WebP quality";
pub const AVIF_QUALITY: &str = "AVIF quality";
pub const PNG_LEVEL: &str = "PNG optimization level (0–6)";
pub const DEFAULT_PDF_PRESET: &str = "PDF compression";
pub const DEFAULT_PDF_PRESETS: [&str; 3] = ["Low", "Medium", "High"];
pub const DEFAULT_VIDEO_PRESET: &str = "Video compression";
pub const DEFAULT_VIDEO_PRESETS: [&str; 5] = [
    "Best (automatic)",
    "Balanced",
    "Smaller",
    "Discord (10 MB)",
    "Email (25 MB)",
];
pub const DEFAULT_VIDEO_ENCODER: &str = "Video encoder";
pub const DEFAULT_VIDEO_ENCODERS: [&str; 2] = ["Auto (graphics card if it works)", "CPU"];
pub const DEFAULT_MP3_BITRATE: &str = "MP3 bitrate";
pub const DEFAULT_MP3_BITRATES: [&str; 4] = ["128 kbps", "192 kbps", "256 kbps", "320 kbps"];
pub const DEFAULT_PDF_DPI: &str = "PDF → images resolution";
pub const DEFAULT_PDF_DPIS: [&str; 3] = ["72 DPI", "150 DPI", "300 DPI"];

/// Subtitle of the collapsed Defaults card, e.g.
/// "JPG 85 · WebP 80 · AVIF 60 · PNG level 2 · PDF Medium · Video Best (automatic) · MP3 192 kbps · 150 DPI".
/// `pdf`, `video`, `mp3`, `dpi` are entries of the lists above.
#[allow(clippy::too_many_arguments)]
pub fn defaults_summary(
    jpeg: i32,
    webp: i32,
    avif: i32,
    png: i32,
    pdf: &str,
    video: &str,
    mp3: &str,
    dpi: &str,
) -> String {
    format!(
        "JPG {jpeg} · WebP {webp} · AVIF {avif} · PNG level {png} · PDF {pdf} · Video {video} · MP3 {mp3} · {dpi}"
    )
}

pub const SECTION_OUTPUT: &str = "Output";
pub const OUTPUT_LOCATION: &str = "Default save location";
pub const OUTPUT_LOCATION_HINT: &str = "Originals are never changed. Name clashes get (1), (2)…";
pub const OUTPUT_NEXT_TO: &str = "Next to the original files";
pub const OUTPUT_FOLDER: &str = "Always in this folder";
pub const OUTPUT_NO_FOLDER: &str = "No folder chosen";
pub const CHOOSE_FOLDER: &str = "Choose folder…";
pub const CHOOSE_FOLDER_TITLE: &str = "Choose where converted files go";
pub const OPEN_FOLDER_WHEN_DONE: &str = "Open folder when done";
pub const KEEP_FILE_DATES: &str = "Keep original file dates";

pub const SECTION_ENGINES: &str = "Engines and performance";
pub const SECTION_OFFICE: &str = "Office document engine";
pub const OFFICE_ENGINE_HINT: &str = "Used for Word, Excel and PowerPoint files.";
pub const ENGINE_CHOICES: [&str; 3] = [
    "Auto (Microsoft Office, else LibreOffice)",
    "Microsoft Office",
    "LibreOffice",
];
pub const CHECK_ENGINES_AGAIN: &str = "Check again";
pub const DOWNLOAD_LIBREOFFICE_LINK: &str = "Download LibreOffice";
pub const LIBREOFFICE: &str = "LibreOffice";

/// "Microsoft Word 16.0 found" / "LibreOffice not found" (the page draws ✓ / ✕
/// icons before them: the UI font has no ✕ glyph).
pub fn engine_line(name: &str, version: Option<&str>) -> String {
    match version {
        Some("") => format!("{name} found"),
        Some(v) => format!("{name} {v} found"),
        None => format!("{name} not found"),
    }
}

pub const IMAGE_JOBS: &str = "Parallel image jobs";
pub const IMAGE_JOBS_HINT: &str = "How many images convert at the same time.";
pub const USE_GPU: &str = "Use the graphics card for video";
/// "Not checked yet. Morph checks the graphics card …" (`app` = the app name).
pub fn gpu_not_checked(app: &str) -> String {
    format!("Not checked yet. {app} checks the graphics card the first time you convert a video.")
}
pub const GPU_NONE: &str = "No working graphics card encoder was found. Video uses the CPU.";

/// First entry of the parallel jobs list.
pub fn image_jobs_auto(n: usize) -> String {
    format!("Automatic ({n})")
}

/// Cached GPU detection, e.g. "Graphics card encoders that work: h264_nvenc, hevc_nvenc".
pub fn gpu_found(encoders: &str) -> String {
    format!("Graphics card encoders that work: {encoders}")
}

pub const TOAST: &str = "Completion notification";
pub const TOAST_HINT: &str =
    "Show a Windows notification when a conversion finishes in the background.";

/// Header of the section that holds Appearance and Notifications.
pub const SECTION_APPEARANCE_NOTIFICATIONS: &str = "Appearance and notifications";
pub const THEME_LABEL: &str = "App theme";
pub const THEME_HINT: &str = "Follow the Windows color mode, or always use Light or Dark.";
pub const THEME_NAMES: [&str; 3] = ["System", "Light", "Dark"];

// ── About ────────────────────────────────────────────────────────────────────

pub const SECTION_ABOUT: &str = "About";
/// About card title: app name · "Version 0.1.0" · license.
pub fn about_title(app: &str, version: &str) -> String {
    format!("{app} · {version} · GPL-3.0-or-later")
}
/// One line under the About title (`app` = the app name).
pub fn about_description(app: &str) -> String {
    format!("{app} is a free, open-source right-click file converter for Windows 11.")
}

/// "Version 0.1.0".
pub fn version(v: &str) -> String {
    format!("Version {v}")
}

pub const GITHUB: &str = "GitHub";
pub const REPORT_ISSUE: &str = "Report a problem";
/// Placeholder links until the public repository exists.
pub const GITHUB_URL: &str = "https://github.com/renangomide/morph";
pub const ISSUES_URL: &str = "https://github.com/renangomide/morph/issues";
pub const OPEN_LOG_FOLDER: &str = "Open log folder";
pub const OPEN_LOGS_FAILED: &str = "Couldn't open the log folder.";
pub const THIRD_PARTY: &str = "Third-party licenses";
pub const THIRD_PARTY_SUMMARY: &str = "FFmpeg, PDFium, Slint, libheif, mozjpeg, oxipng…";
/// About page note above the component list (`app` = the app name).
pub fn third_party_hint(app: &str) -> String {
    format!(
        "{app} is free software (GPL-3.0-or-later) and is built with these components. Full license texts: THIRD_PARTY_NOTICES.md and the licenses folder in the install folder."
    )
}

/// Bundled components and their licenses: the headline entries of
/// `THIRD_PARTY_NOTICES.md` (keep in sync when it is regenerated).
pub const THIRD_PARTY_LIST: &[(&str, &str)] = &[
    ("FFmpeg 9.0.2 (morph-ffmpeg 9.0.2-1)", "GPL-3.0"),
    ("Slint 1.18.1", "GPL-3.0-only"),
    ("PDFium chromium/7881", "BSD-3-Clause, Apache-2.0"),
    (
        "FreeType, ICU, libpng, OpenJPEG, … (inside PDFium)",
        "permissive",
    ),
    ("libheif 1.23.5, libde265 1.1.3", "LGPL-3.0"),
    ("libaom 3.15.2", "BSD-2-Clause + AOM patent license"),
    ("libwebp 1.3.1", "BSD-3-Clause"),
    ("mozjpeg", "IJG, BSD-3-Clause, Zlib"),
    ("libdeflate 1.26", "MIT"),
    ("libimagequant", "GPL-3.0-or-later"),
    ("rav1e, ravif", "BSD-2-Clause, BSD-3-Clause"),
    (
        "image, resvg, tiny-skia, oxipng",
        "MIT, Apache-2.0, BSD-3-Clause",
    ),
    ("pdfium-render, lopdf", "MIT, Apache-2.0"),
    ("kamadak-exif", "BSD-2-Clause"),
    (
        "calamine, rust_xlsxwriter, pulldown-cmark, htmd",
        "MIT, Apache-2.0",
    ),
    ("encoding_rs", "MIT or Apache-2.0, BSD-3-Clause"),
    ("windows-rs, tray-icon, rfd", "MIT, Apache-2.0"),
    ("Feather icons", "MIT"),
    (
        "About 400 more Rust crates",
        "permissive (MIT, Apache-2.0, BSD, …)",
    ),
];

/// "FFmpeg — GPL-3.0".
pub fn third_party_line(name: &str, license: &str) -> String {
    format!("{name} — {license}")
}
