// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable action IDs. Used in request files, `state.toml` and the CLI.

pub const IMAGE_CONVERT: &str = "image.convert";
pub const IMAGE_COMPRESS: &str = "image.compress";
pub const IMAGE_RESIZE: &str = "image.resize";
pub const IMAGE_CLEAN: &str = "image.clean";
/// Each image → its own PDF (one page). Combining several into one PDF is `pdf.merge`.
pub const IMAGE_TO_PDF: &str = "image.to_pdf";

pub const PDF_COMPRESS: &str = "pdf.compress";
pub const PDF_TO_IMAGES: &str = "pdf.to_images";
pub const PDF_TO_WORD: &str = "pdf.to_word";
pub const PDF_TO_TEXT: &str = "pdf.to_text";
/// PDF → Markdown or HTML (`format` option: md, html), native layout reader.
pub const PDF_TO_MARKDOWN: &str = "pdf.to_markdown";
pub const PDF_SPLIT: &str = "pdf.split";
pub const PDF_MERGE: &str = "pdf.merge";

pub const VIDEO_CONVERT: &str = "video.convert";
pub const VIDEO_COMPRESS: &str = "video.compress";
/// The sound track as its own audio file (MP3 by default).
pub const VIDEO_EXTRACT_AUDIO: &str = "video.extract_audio";
/// Animated GIF from a part of the video.
pub const VIDEO_TO_GIF: &str = "video.to_gif";

pub const AUDIO_CONVERT: &str = "audio.convert";

/// Word-, Excel-, PowerPoint- and text-like documents → another document format
/// (`format` option: pdf, docx, odt, rtf, txt, html, md, xlsx, ods, csv, pptx, odp).
pub const OFFICE_CONVERT: &str = "office.convert";
/// Word pages / PowerPoint slides → PNG/JPG images.
pub const OFFICE_TO_IMAGES: &str = "office.to_images";

/// Mixed selection: open the grouped window, no action pre-selected.
pub const MIXED_CONVERT: &str = "mixed.convert";
