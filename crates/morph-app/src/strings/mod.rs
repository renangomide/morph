// SPDX-License-Identifier: GPL-3.0-or-later
//! Every user-visible string of `Morph.exe`, in one place: this module.
//! Split by area so parallel work doesn't collide: the conversion window and
//! shared text here, the main window + Settings in `home.rs`, background /
//! tray / toast / results text in `background.rs`.

mod background;
mod home;
// A name defined here too would silently win over the glob: the test below
// keeps names unique across the three files.
pub use background::*;
pub use home::*;

pub const EMPTY_HEADER: &str = "No files yet";
pub const EMPTY_PLACEHOLDER: &str = "Drop files here";
pub const CLOSE: &str = "Close";
pub const CANCEL: &str = "Cancel";

pub const FILE_ONE: &str = "file";
pub const FILE_MANY: &str = "files";
pub const HEADER_SEPARATOR: &str = " · ";
pub const SIZE_MISSING: &str = "missing";

pub const ERROR_REQUEST_UNREADABLE: &str =
    "Couldn't read the request from Explorer. Right-click the files and try again.";
pub const RESULTS_MISSING: &str = "These results are no longer available.";
pub const RESULTS_MISSING_DETAIL: &str = "Results are kept for 7 days.";

/// Human-readable size units (1024-based, labelled like Explorer does).
pub const SIZE_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

// ── Conversion window ────────────────────────────────────────────────────────

pub const SHOW_FILES: &str = "Show files";
pub const HIDE_FILES: &str = "Hide files";
pub const REMOVE_FILE: &str = "Remove";
pub const ACTION_CONVERT: &str = "Convert";
pub const ACTION_COMPRESS: &str = "Compress";
pub const ACTION_RESIZE: &str = "Resize";
pub const ACTION_CLEAN: &str = "Clean";

pub const FORMAT: &str = "Format";
pub const QUALITY: &str = "Quality";
pub const QUALITY_DEFAULT: &str = "Default for each format";
pub const QUALITY_COMPRESS_HINT: &str =
    "Applies to JPG, WebP and AVIF. PNG is always optimized without loss.";
pub const LOSSLESS: &str = "Lossless";
pub const KEEP_METADATA: &str = "Keep metadata (date, camera, location)";
pub const REMOVE_GPS: &str = "Remove location (GPS) only";
pub const BACKGROUND: &str = "Background for transparent areas";
pub const BACKGROUND_WHITE: &str = "White";
pub const BACKGROUND_BLACK: &str = "Black";
pub const ICO_SIZES: &str = "Icon sizes";
pub const SVG_WIDTH: &str = "SVG width (px)";

pub const COMPRESS_MODE: &str = "Mode";
pub const COMPRESS_BEST: &str = "Best (automatic)";
pub const COMPRESS_BEST_HINT: &str =
    "Smallest file that still looks the same. Keeps the format; may take a few seconds per photo.";
pub const COMPRESS_BY_QUALITY: &str = "Quality";
pub const COMPRESS_BY_SIZE: &str = "Target size";
pub const TARGET_SIZE_AT_MOST: &str = "At most";
pub const UNIT_KB: &str = "KB";
pub const UNIT_MB: &str = "MB";
pub const CONVERT_TO_WEBP: &str = "Convert to WebP for best size";
pub const PNG_LOSSY: &str = "PNG: reduce colors (smaller, slightly lossy)";

pub const RESIZE_MODE: &str = "Resize by";
pub const RESIZE_PERCENT: &str = "Percent";
pub const RESIZE_MAX_WIDTH: &str = "Max width";
pub const RESIZE_MAX_HEIGHT: &str = "Max height";
pub const RESIZE_EXACT: &str = "Exact size";
pub const PERCENT: &str = "Percent (%)";
pub const WIDTH: &str = "Width (px)";
pub const HEIGHT: &str = "Height (px)";
pub const KEEP_ASPECT: &str = "Keep aspect ratio";
pub const NO_ENLARGE: &str = "Don't enlarge smaller images";

pub const STRIP_LABEL: &str = "Metadata";
pub const STRIP_ALL: &str = "Remove all metadata";
pub const STRIP_GPS: &str = "Remove GPS only";
pub const STRIP_NONE: &str = "Keep metadata";
pub const ROTATE: &str = "Rotate";
pub const ROTATE_NONE: &str = "No rotation";
pub const ROTATE_90: &str = "90° right";
pub const ROTATE_180: &str = "180°";
pub const ROTATE_270: &str = "90° left";
pub const FLIP_H: &str = "Flip horizontally";
pub const FLIP_V: &str = "Flip vertically";
pub const CLEAN_ROTATE_NOTE: &str = "Photos are always turned upright from their EXIF orientation.";

pub const SAVE_TO: &str = "Save to";
pub const SAVE_NEXT_TO: &str = "Next to the original files";
pub const SAVE_CHOOSE: &str = "Choose folder…";
pub const SAVE_BROWSE: &str = "Browse…";
pub const SAVE_NO_FOLDER: &str = "No folder chosen";
pub const PICK_FOLDER_TITLE: &str = "Choose where to save the converted files";
pub const REMEMBER: &str = "Remember these settings";

pub const BAD_TARGET_SIZE: &str = "Enter a target size greater than 0, like 500 KB or 1.5 MB.";
pub const BAD_PERCENT: &str = "Enter a percent between 1 and 1000.";
pub const BAD_DIMENSION: &str = "Enter a size between 1 and 65535 pixels.";
pub const BAD_ICO_SIZES: &str = "Pick at least one icon size.";

// ── PDF panels ───────────────────────────────────────────────────────────────

pub const PDF_ACTION_COMPRESS: &str = "Compress";
pub const PDF_ACTION_SPLIT: &str = "Split";
pub const PDF_ACTION_MERGE: &str = "Merge";

pub const PDF_COMPRESSION: &str = "Compression";
pub const PDF_PRESET_LOW: &str = "Low";
pub const PDF_PRESET_MEDIUM: &str = "Medium";
pub const PDF_PRESET_HIGH: &str = "High";
pub const PDF_PRESET_CUSTOM: &str = "Custom";
/// Helper text under the preset radios, one per preset (same order).
pub const PDF_PRESET_HINTS: [&str; 4] = [
    "Light compression. Pictures stay sharp enough for printing (up to 300 DPI).",
    "Recommended. Smaller file, pictures still look good on screen and on paper (up to 150 DPI).",
    "Smallest file. Pictures get softer (up to 96 DPI); fine for reading on screen.",
    "Choose the maximum picture resolution and the JPEG quality yourself.",
];
pub const PDF_MAX_DPI: &str = "Max picture DPI";
pub const PDF_COMPRESS_NOTE: &str =
    "Text stays sharp. If the PDF can't get smaller, no new file is written.";

pub const PDF_DPI: &str = "Resolution";
pub const PDF_DPI_CUSTOM: &str = "Custom";
/// DPI combo box entry: "150 DPI".
pub fn dpi_label(dpi: u32) -> String {
    format!("{dpi} DPI")
}
pub const PDF_DPI_VALUE: &str = "DPI";
pub const PDF_PAGES: &str = "Pages";
pub const PDF_PAGES_PLACEHOLDER: &str = "All pages (or e.g. 1-3,7)";
pub const PDF_TO_IMAGES_NOTE: &str = "Pages are saved in a new folder next to the PDF.";

pub const PDF_PAGE_SEPARATORS: &str = "Add a \"--- Page N ---\" line before each page";
pub const PDF_TO_TEXT_NOTE: &str =
    "Saves a .txt file. Scanned PDFs (pictures of text) have no text to extract.";

pub const PDF_SPLIT_MODE: &str = "Split";
pub const PDF_SPLIT_EVERY_PAGE: &str = "Every page";
pub const PDF_SPLIT_EVERY_N: &str = "Every N pages";
pub const PDF_SPLIT_RANGES: &str = "Custom ranges";
pub const PDF_SPLIT_EVERY: &str = "Pages per file";
pub const PDF_SPLIT_RANGES_LABEL: &str = "Ranges";
pub const PDF_SPLIT_RANGES_PLACEHOLDER: &str = "e.g. 1-3,4-10,11-";
pub const PDF_SPLIT_NOTE: &str = "Each part is saved as its own PDF in a new folder.";

pub const PDF_PAGE_SIZE: &str = "Page size";
pub const PDF_PAGE_FIT: &str = "Fit to image";
pub const PDF_PAGE_A4: &str = "A4";
pub const PDF_PAGE_LETTER: &str = "Letter";
pub const PDF_MARGIN: &str = "Margin (mm)";
pub const PDF_BOOKMARKS: &str = "Add a bookmark for each file";
pub const PDF_MERGE_NOTE: &str = "Page size and margin apply to images; PDF pages keep their size. Change the order in the file list above.";
/// Top of the file list when Merge is chosen.
pub const MERGE_ORDER_HINT: &str =
    "Files are merged in this order. Drag the handle or use the arrows to change it.";

/// M9 PDF tabs: Convert holds every "PDF to …" target as a chip.
pub const PDF_TABS: [&str; 4] = ["Convert", "Compress", "Split", "Merge"];
pub const PDF_CONVERT_TO: &str = "Convert PDF to";
/// Chips of the PDF Convert tab, in this order (the view maps them to actions).
pub const PDF_CONVERT_CHIPS: [&str; 5] = ["Word", "Markdown", "HTML", "Images", "Plain text"];
/// One line under each PDF compression preset card, same order as the presets.
pub const PDF_PRESET_DESCS: [&str; 4] = [
    "Up to 300 DPI · print quality",
    "Up to 150 DPI · recommended",
    "Up to 96 DPI · smallest file",
    "Your own DPI and quality",
];
/// One line under each image compression mode card (Best, Quality, Target size).
pub const COMPRESS_MODE_DESCS: [&str; 3] = [
    "Looks the same, smallest file",
    "Set the quality yourself",
    "At most a file size you pick",
];
/// Short chip names of the image formats with long list labels.
pub const CHIP_PDF: &str = "PDF";
pub const CHIP_ICO: &str = "ICO";
pub const MOVE_UP: &str = "Move up";
pub const MOVE_DOWN: &str = "Move down";

pub const BAD_DPI: &str = "Enter a resolution between 36 and 1200 DPI.";
pub const BAD_EVERY: &str = "Enter how many pages go in each file (1 to 10000).";
pub const BAD_MARGIN: &str = "Enter a margin between 0 and 100 mm.";
pub const SPLIT_RANGES_EMPTY: &str = "Enter the page ranges to split into, like 1-3,4-10,11-.";
/// Inline message under a page range box.
pub fn bad_page_range(spec: &str) -> String {
    format!("\"{spec}\" isn't a valid page range. Use something like 1-3,7,10-.")
}

/// SPEC §4.2.
pub const PDF_TO_WORD_WARNING: &str = "Layout may change; works best on text-based PDFs.";
pub const PDF_TO_WORD_NOTE: &str = "Saves a .docx file next to the PDF.";

pub const PDF_TO_MARKDOWN_NOTE: &str = "Headings, lists and tables are kept. Saves a .md or .html file next to the PDF. Scanned PDFs (pictures of text) have no text to read.";
/// Markdown / HTML outputs: pictures into a "name (images)" folder.
pub const SAVE_PICTURES: &str = "Save pictures (in a folder next to the file)";

// ── Office / text document panels ───────────────────────────────────────────

pub const OFFICE_ACTION_CONVERT: &str = "Convert";
pub const OFFICE_ACTION_TO_IMAGES: &str = "Export as images";
pub const OFFICE_CONVERT_TO: &str = "Convert to";
pub const CSV_DELIMITER: &str = "CSV separator";
/// Same order as `CsvDelimiter::ALL`.
pub const CSV_DELIMITERS: [&str; 4] = ["Detect automatically", "Comma (,)", "Semicolon (;)", "Tab"];
pub const CSV_DELIMITER_HINT: &str =
    "Used to read CSV files, and to write them (\"Detect automatically\" writes commas).";
pub const CSV_BOM: &str = "UTF-8 with BOM (Excel shows accents correctly)";
pub const OFFICE_CONVERT_NOTE: &str =
    "The new file is saved next to the original. Documents can take a few seconds each.";
pub const OFFICE_IMAGES_NOTE: &str =
    "Each page or slide becomes an image, saved in a new folder next to the file.";
pub const MAX_ROWS: &str = "Rows per sheet (0 = all)";
pub const SLIDE_NOTES: &str = "Include speaker notes";
pub const BAD_MAX_ROWS: &str = "Enter how many rows to keep per sheet (0 = all).";
pub const OFFICE_IMAGES_ONLY: &str = "Only documents and presentations have pages to export.";

/// SPEC §3.2 missing-engine banner.
pub const ENGINE_MISSING: &str =
    "To convert documents, install Microsoft Office or LibreOffice (free).";
pub const DOWNLOAD_LIBREOFFICE: &str = "Download LibreOffice";
pub const CHECK_AGAIN: &str = "Check again";
/// Under the banner when some files can still convert natively.
pub fn engine_missing_some(count: usize) -> String {
    match count {
        1 => "1 file needs it and will be skipped; the others convert now.".to_owned(),
        n => format!("{n} files need it and will be skipped; the others convert now."),
    }
}
/// Results row reason for a file that needs a missing engine.
pub const NEEDS_ENGINE: &str = "Needs Microsoft Office or LibreOffice (free).";
/// Results row reason: "This file already is PDF."
pub fn already_format(label: &str) -> String {
    format!("This file already is {label}.")
}
/// "Using LibreOffice", "Using Microsoft Word and LibreOffice".
pub fn using_engines(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("Using {one}"),
        [rest @ .., last] => format!("Using {} and {last}", rest.join(", ")),
    }
}

/// Short name of a document format, for messages ("PDF", "Markdown").
pub fn doc_format_short(format: morph_core::office::options::DocFormat) -> &'static str {
    use morph_core::office::options::DocFormat as F;
    match format {
        F::Pdf => "PDF",
        F::Docx => "DOCX",
        F::Odt => "ODT",
        F::Rtf => "RTF",
        F::Txt => "TXT",
        F::Html => "HTML",
        F::Md => "Markdown",
        F::Xlsx => "XLSX",
        F::Ods => "ODS",
        F::Csv => "CSV",
        F::Pptx => "PPTX",
        F::Odp => "ODP",
    }
}

/// Convert button when nothing can run (it is disabled; a banner or the
/// Skipped list says why).
pub const CONVERT_NOTHING: &str = "Convert";
/// "Convert 12".
pub fn convert_button(count: usize) -> String {
    format!("Convert {count}")
}

/// "Merge 3 files".
pub fn merge_button(count: usize) -> String {
    if count == 1 {
        "Merge 1 file".to_owned()
    } else {
        format!("Merge {count} files")
    }
}

/// Progress title of Merge: "Merging 3 files…".
pub fn merging(count: usize) -> String {
    if count == 1 {
        "Merging 1 file…".to_owned()
    } else {
        format!("Merging {count} files…")
    }
}

/// Results row title of a merge: "3 files" (→ Merged.pdf).
pub fn merged_inputs(count: usize) -> String {
    format!("{count} files")
}

/// After a folder name in a results row: " (12 files)".
pub fn files_in_folder(count: usize) -> String {
    format!(" ({count} files)")
}

/// "2 files can't use this action and will be skipped."
pub fn will_skip(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 file can't use this action and will be skipped.".to_owned(),
        n => format!("{n} files can't use this action and will be skipped."),
    }
}

// ── Mixed selection (tabs per type) ─────────────────────────────────────────

/// "Convert all 12" (one button for every tab).
pub fn convert_all_button(count: usize) -> String {
    format!("Convert all {count}")
}
/// Tabs of a mixed selection, with the file count.
pub fn tab_images(count: usize) -> String {
    format!("Images ({count})")
}
pub fn tab_pdf(count: usize) -> String {
    format!("PDF ({count})")
}
pub fn tab_documents(count: usize) -> String {
    format!("Documents ({count})")
}
/// Heading of the skipped files in "Show files": "Skipped (2)".
pub fn skipped_heading(count: usize) -> String {
    format!("Skipped ({count})")
}
/// Mixed window: "2 files will be skipped. Show files lists why."
pub fn will_skip_mixed(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 file will be skipped. Show files lists why.".to_owned(),
        n => format!("{n} files will be skipped. Show files lists why."),
    }
}

// ── Video and audio panels ──────────────────────────────────────────────────

/// Tabs of a selection with both videos and audio files.
pub fn tab_video(count: usize) -> String {
    format!("Video ({count})")
}
pub fn tab_audio(count: usize) -> String {
    format!("Audio ({count})")
}
pub const VIDEO_ACTION_CONVERT: &str = "Convert";
pub const VIDEO_ACTION_COMPRESS: &str = "Compress";
pub const VIDEO_ACTION_EXTRACT_AUDIO: &str = "Extract audio";
pub const VIDEO_ACTION_TO_GIF: &str = "To GIF";

pub const ADVANCED_SHOW: &str = "Show advanced options";
pub const ADVANCED_HIDE: &str = "Hide advanced options";

/// Same order as `VideoContainer::ALL`.
pub const VIDEO_CONTAINERS: [&str; 5] = ["MP4", "MKV", "WebM", "MOV", "AVI"];
pub const VIDEO_FAST: &str = "Fast mode (no quality loss when possible)";
pub const VIDEO_FAST_HINT: &str = "Copies the video and sound as they are when they fit the new format: takes seconds. Otherwise the video is re-encoded.";
pub const VIDEO_CODEC: &str = "Video codec";
/// Same order as `media_options::VIDEO_CODECS`.
pub const VIDEO_CODECS: [&str; 5] = ["Automatic", "H.264", "H.265 (HEVC)", "AV1", "VP9"];
pub const AUDIO_CODEC: &str = "Audio codec";
/// Same order as `media_options::VIDEO_AUDIO_CODECS`.
pub const VIDEO_AUDIO_CODECS: [&str; 4] = ["Automatic", "AAC", "Opus", "MP3"];
pub const VIDEO_QUALITY: &str = "Quality";
/// Same order as `media_options::VIDEO_QUALITIES`.
pub const VIDEO_QUALITIES: [&str; 3] = ["High (bigger file)", "Standard", "Small (lower quality)"];
pub const VIDEO_ENCODER: &str = "Encoder";
/// Same order as `media_options::ENCODERS`.
pub const VIDEO_ENCODERS: [&str; 2] = ["Automatic (graphics card if available)", "CPU only"];
pub const VIDEO_CONVERT_NOTE: &str = "The new video is saved next to the original.";

pub const VIDEO_PRESET: &str = "Preset";
/// Same order as `media_options::PRESETS`.
pub const VIDEO_PRESETS: [&str; 6] = [
    "Best (automatic)",
    "Balanced",
    "Smaller",
    "Discord (10 MB)",
    "Email (25 MB)",
    "Custom size",
];
/// One line under the preset box, same order.
pub const VIDEO_PRESET_HINTS: [&str; 6] = [
    "Smallest file with no visible quality loss. Keeps the size and format; never makes a file bigger.",
    "Good quality, smaller file (H.264). Plays everywhere.",
    "Even smaller (H.265). Some older devices can't play it.",
    "Fits Discord's 10 MB upload limit. Long videos get a lower resolution.",
    "Fits a 25 MB email attachment. Long videos get a lower resolution.",
    "Pick the largest file size you want.",
];
pub const VIDEO_TARGET_MB: &str = "Target size (MB)";
pub const VIDEO_RESOLUTION: &str = "Resolution";
/// Same order as `media_options::RESOLUTIONS`.
pub const VIDEO_RESOLUTIONS: [&str; 4] = ["Keep original", "1080p (Full HD)", "720p (HD)", "480p"];
pub const VIDEO_RESOLUTION_HINT: &str = "Smaller videos are never enlarged.";

pub const EXTRACT_AUDIO_NOTE: &str = "The sound track is saved as its own file next to the video. When it already fits the format, it is copied without any quality loss.";

pub const GIF_START: &str = "Start (mm:ss)";
pub const GIF_START_PLACEHOLDER: &str = "0:00";
pub const GIF_LENGTH: &str = "Length (seconds)";
pub const GIF_WIDTH: &str = "Width";
/// GIF width box entry: "Keep original" for 0, else "480 px".
pub fn gif_width_label(width: u32) -> String {
    if width == 0 {
        "Keep original".to_owned()
    } else {
        format!("{width} px")
    }
}
pub const GIF_FPS: &str = "Frame rate";
/// "15 frames per second".
pub fn gif_fps_label(fps: u32) -> String {
    format!("{fps} frames per second")
}
pub const GIF_NOTE: &str =
    "Good for chats and documents. GIFs get big quickly: keep them short and small.";

pub const AUDIO_BITRATE: &str = "Bitrate";
/// Bitrate box entry: "192 kbps".
pub fn kbps_label(kbps: u32) -> String {
    format!("{kbps} kbps")
}
pub const MP3_MODE: &str = "Bitrate mode";
/// Same order as `media_options::MP3_MODES`.
pub const MP3_MODES: [&str; 3] = [
    "Constant (CBR)",
    "Variable, best (VBR V0)",
    "Variable, smaller (VBR V2)",
];
pub const SAMPLE_RATE: &str = "Sample rate";
/// Same order as `media_options::SAMPLE_RATES`.
pub const SAMPLE_RATES: [&str; 3] = ["Keep original", "44.1 kHz", "48 kHz"];
pub const CHANNELS: &str = "Channels";
/// Same order as `media_options::CHANNELS`.
pub const CHANNEL_CHOICES: [&str; 2] = ["Keep original", "Mono"];
pub const KEEP_TAGS: &str = "Keep tags and cover art";
pub const AUDIO_CONVERT_NOTE: &str = "The new file is saved next to the original.";
pub const LOSSLESS_WARNING: &str = "Converting to a lossless format won't improve quality.";

/// Label for an audio output format.
pub fn audio_format_label(format: morph_core::media::options::AudioFormat) -> &'static str {
    use morph_core::media::options::AudioFormat as F;
    match format {
        F::Mp3 => "MP3",
        F::M4a => "M4A (AAC)",
        F::Wav => "WAV (lossless)",
        F::Flac => "FLAC (lossless)",
        F::Ogg => "OGG (Vorbis)",
        F::Opus => "Opus",
    }
}

pub const FFMPEG_MISSING: &str =
    "The video engine (FFmpeg) is missing. Reinstall the app to fix this.";
pub const BAD_TARGET_MB: &str = "Enter a target size greater than 0 MB, like 50.";
pub const BAD_GIF_START: &str = "Enter the start time as minutes:seconds, like 1:30.";
pub const BAD_GIF_LENGTH: &str = "Enter a length between 1 and 60 seconds.";
/// Results row reason for a file that neither video nor audio actions take.
pub const NOT_SUPPORTED_HERE: &str = "Not supported for this action.";
/// Running row with an estimate: "41% · about 2 min left".
pub fn time_left(seconds: u64) -> String {
    match seconds {
        0..60 => "less than a minute left".to_owned(),
        s => format!("about {} min left", (s + 30) / 60),
    }
}

pub const NOT_AVAILABLE_TITLE: &str = "Not available yet";
/// Message for a category that a later version handles.
pub fn not_available(what: &str) -> String {
    format!(
        "Converting {what} isn't available in this version yet. Images, PDFs, documents, video and audio work today."
    )
}
pub const WHAT_PDF: &str = "PDF files";
pub const WHAT_VIDEO: &str = "videos";
pub const WHAT_AUDIO: &str = "audio";
pub const WHAT_OFFICE: &str = "Office documents";
pub const WHAT_THESE: &str = "these files";
pub const NO_SUPPORTED_FILES: &str = "None of these files can be converted.";
/// Skipped row reason for a dropped or browsed folder.
pub const FOLDERS_NOT_SUPPORTED: &str = "Folders aren't supported.";

// ── M9: summary header, category cards, gear ─────────────────────────────

/// Summary header subtitle of a single-type selection: "Images batch ready".
pub fn batch_ready(name: &str) -> String {
    format!("{name} batch ready")
}
/// Summary header subtitle of a mixed selection: "3 categories selected".
pub fn categories_selected(count: usize) -> String {
    format!("{count} categories selected")
}
/// "12 files" (summary header count; the size is a separate pill).
pub fn file_count(count: usize) -> String {
    let noun = if count == 1 { FILE_ONE } else { FILE_MANY };
    format!("{count} {noun}")
}
/// Type name on cards and in the skip reason (Settings says "Office documents").
pub const GROUP_DOCUMENTS: &str = "Documents";
/// Category card checkbox, accessible name: "Include Images".
pub fn include_category(name: &str) -> String {
    format!("Include {name}")
}
/// Skip reason of a type the user unchecked.
pub fn skipped_by_choice(name: &str) -> String {
    format!("Skipped: {name} is unchecked.")
}
/// Category card summary of a conversion: "→ WebP".
pub fn summary_to(label: &str) -> String {
    format!("→ {label}")
}
pub const SUMMARY_PDF: &str = "PDF";
pub const SUMMARY_WORD: &str = "Word";
pub const SUMMARY_TEXT: &str = "Text";
pub const SUMMARY_MARKDOWN: &str = "Markdown";
pub const SUMMARY_HTML: &str = "HTML";
pub const SUMMARY_GIF: &str = "GIF";
/// Video container names, same order as `VIDEO_CONTAINERS`.
pub fn video_container_label(format: morph_core::media::options::VideoContainer) -> &'static str {
    use morph_core::media::options::VideoContainer as C;
    let i = match format {
        C::Mp4 => 0,
        C::Mkv => 1,
        C::Webm => 2,
        C::Mov => 3,
        C::Avi => 4,
    };
    VIDEO_CONTAINERS[i]
}
/// Gear button in the conversion window header (accessible name).
pub const SETTINGS_BUTTON: &str = "Settings";

/// Label for an image output format in the format list.
pub fn format_label(format: morph_core::image::options::OutputFormat) -> &'static str {
    use morph_core::image::options::OutputFormat as F;
    match format {
        F::Jpg => "JPG",
        F::Png => "PNG",
        F::Webp => "WebP",
        F::Avif => "AVIF",
        F::Bmp => "BMP",
        F::Tiff => "TIFF",
        F::Gif => "GIF",
        F::Ico => "ICO (icon)",
    }
}

/// Last entry of the image format list: Image → PDF (`image.to_pdf`).
pub const FORMAT_PDF: &str = "PDF (one per image)";
pub const IMAGE_TO_PDF_NOTE: &str = "Each image becomes a one-page PDF next to it. To put several images in one PDF, use \"Combine into one PDF…\" in the right-click menu.";

#[cfg(test)]
mod tests {
    /// `pub use home::*` / `background::*` silently lose to a same-named item
    /// here, so every name must be unique across the three files.
    #[test]
    fn no_string_is_defined_twice() {
        let mut seen = std::collections::HashSet::new();
        let files = [
            include_str!("mod.rs"),
            include_str!("home.rs"),
            include_str!("background.rs"),
        ];
        for line in files.iter().flat_map(|f| f.lines()) {
            let item = ["pub const ", "pub static ", "pub fn "]
                .iter()
                .find_map(|p| line.strip_prefix(p));
            let Some(item) = item else { continue };
            let name: String = item
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert!(seen.insert(name.clone()), "{name} is defined twice");
        }
        assert!(seen.len() > 100, "the check found the strings");
    }
}
