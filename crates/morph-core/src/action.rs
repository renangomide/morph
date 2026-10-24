// SPDX-License-Identifier: GPL-3.0-or-later
//! What to do with the files. Serialized as `{"action": "<id>", "options": {...}}`
//! (request files, `state.toml`, CLI). IDs come from `morph_formats::actions`.
use crate::image::options::{
    ImageCleanOpts, ImageCompressOpts, ImageConvertOpts, ImageResizeOpts, ImageToPdfOpts,
};
use crate::media::options::{
    AudioConvertOpts, VideoCompressOpts, VideoConvertOpts, VideoToGifOpts,
};
use crate::office::options::{OfficeConvertOpts, OfficeToImagesOpts};
use crate::pdf::options::{
    PdfCompressOpts, PdfMergeOpts, PdfSplitOpts, PdfToImagesOpts, PdfToMarkdownOpts, PdfToTextOpts,
    PdfToWordOpts,
};
use morph_formats::{Category, actions};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "options")]
pub enum Action {
    #[serde(rename = "image.convert")]
    ImageConvert(ImageConvertOpts),
    #[serde(rename = "image.compress")]
    ImageCompress(ImageCompressOpts),
    #[serde(rename = "image.resize")]
    ImageResize(ImageResizeOpts),
    #[serde(rename = "image.clean")]
    ImageClean(ImageCleanOpts),
    /// Each image → its own one-page PDF.
    #[serde(rename = "image.to_pdf")]
    ImageToPdf(ImageToPdfOpts),
    #[serde(rename = "pdf.compress")]
    PdfCompress(PdfCompressOpts),
    #[serde(rename = "pdf.to_images")]
    PdfToImages(PdfToImagesOpts),
    #[serde(rename = "pdf.to_word")]
    PdfToWord(PdfToWordOpts),
    #[serde(rename = "pdf.to_text")]
    PdfToText(PdfToTextOpts),
    /// PDF → Markdown / HTML (native layout reader).
    #[serde(rename = "pdf.to_markdown")]
    PdfToMarkdown(PdfToMarkdownOpts),
    #[serde(rename = "pdf.split")]
    PdfSplit(PdfSplitOpts),
    /// Also "Combine into one PDF" (images and PDFs).
    #[serde(rename = "pdf.merge")]
    PdfMerge(PdfMergeOpts),
    /// Word-, Excel-, PowerPoint- and text-like documents → another format.
    #[serde(rename = "office.convert")]
    OfficeConvert(OfficeConvertOpts),
    /// Word pages / PowerPoint slides → images.
    #[serde(rename = "office.to_images")]
    OfficeToImages(OfficeToImagesOpts),
    #[serde(rename = "video.convert")]
    VideoConvert(VideoConvertOpts),
    #[serde(rename = "video.compress")]
    VideoCompress(VideoCompressOpts),
    /// The sound track as its own file; same options as Audio → Convert.
    #[serde(rename = "video.extract_audio")]
    VideoExtractAudio(AudioConvertOpts),
    #[serde(rename = "video.to_gif")]
    VideoToGif(VideoToGifOpts),
    #[serde(rename = "audio.convert")]
    AudioConvert(AudioConvertOpts),
}

/// Which worker queue runs a task (SPEC §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Queue {
    /// Thread pool: images, PDF, CSV/XLSX, native text conversions.
    Cpu,
    /// ffmpeg.exe children, 1 at a time by default (`JobConfig::ffmpeg_jobs`, 1–2).
    Ffmpeg,
    /// One Office/LibreOffice conversion at a time (anything that needs an engine).
    Office,
}

impl Action {
    pub fn id(&self) -> &'static str {
        match self {
            Action::ImageConvert(_) => actions::IMAGE_CONVERT,
            Action::ImageCompress(_) => actions::IMAGE_COMPRESS,
            Action::ImageResize(_) => actions::IMAGE_RESIZE,
            Action::ImageClean(_) => actions::IMAGE_CLEAN,
            Action::ImageToPdf(_) => actions::IMAGE_TO_PDF,
            Action::PdfCompress(_) => actions::PDF_COMPRESS,
            Action::PdfToImages(_) => actions::PDF_TO_IMAGES,
            Action::PdfToWord(_) => actions::PDF_TO_WORD,
            Action::PdfToText(_) => actions::PDF_TO_TEXT,
            Action::PdfToMarkdown(_) => actions::PDF_TO_MARKDOWN,
            Action::PdfSplit(_) => actions::PDF_SPLIT,
            Action::PdfMerge(_) => actions::PDF_MERGE,
            Action::OfficeConvert(_) => actions::OFFICE_CONVERT,
            Action::OfficeToImages(_) => actions::OFFICE_TO_IMAGES,
            Action::VideoConvert(_) => actions::VIDEO_CONVERT,
            Action::VideoCompress(_) => actions::VIDEO_COMPRESS,
            Action::VideoExtractAudio(_) => actions::VIDEO_EXTRACT_AUDIO,
            Action::VideoToGif(_) => actions::VIDEO_TO_GIF,
            Action::AudioConvert(_) => actions::AUDIO_CONVERT,
        }
    }

    /// Default options for an action ID.
    pub fn default_for(id: &str) -> Option<Action> {
        Some(match id {
            actions::IMAGE_CONVERT => Action::ImageConvert(Default::default()),
            actions::IMAGE_COMPRESS => Action::ImageCompress(Default::default()),
            actions::IMAGE_RESIZE => Action::ImageResize(Default::default()),
            actions::IMAGE_CLEAN => Action::ImageClean(Default::default()),
            actions::IMAGE_TO_PDF => Action::ImageToPdf(Default::default()),
            actions::PDF_COMPRESS => Action::PdfCompress(Default::default()),
            actions::PDF_TO_IMAGES => Action::PdfToImages(Default::default()),
            actions::PDF_TO_WORD => Action::PdfToWord(Default::default()),
            actions::PDF_TO_TEXT => Action::PdfToText(Default::default()),
            actions::PDF_TO_MARKDOWN => Action::PdfToMarkdown(Default::default()),
            actions::PDF_SPLIT => Action::PdfSplit(Default::default()),
            actions::PDF_MERGE => Action::PdfMerge(Default::default()),
            actions::OFFICE_CONVERT => Action::OfficeConvert(Default::default()),
            actions::OFFICE_TO_IMAGES => Action::OfficeToImages(Default::default()),
            actions::VIDEO_CONVERT => Action::VideoConvert(Default::default()),
            actions::VIDEO_COMPRESS => Action::VideoCompress(Default::default()),
            actions::VIDEO_EXTRACT_AUDIO => Action::VideoExtractAudio(Default::default()),
            actions::VIDEO_TO_GIF => Action::VideoToGif(Default::default()),
            actions::AUDIO_CONVERT => Action::AudioConvert(Default::default()),
            _ => return None,
        })
    }

    /// Defaults for `id`, then `overrides` (e.g. a menu preset `{"format":"png"}`
    /// or remembered settings) merged on top, key by key.
    pub fn from_id(id: &str, overrides: &Map<String, Value>) -> Result<Action, String> {
        let default = Action::default_for(id).ok_or_else(|| format!("unknown action '{id}'"))?;
        let mut value = serde_json::to_value(&default).map_err(|e| e.to_string())?;
        if let Some(Value::Object(options)) = value.get_mut("options") {
            for (k, v) in overrides {
                // Reject typos like "qualty" instead of silently using the default.
                if !options.contains_key(k) {
                    let mut known: Vec<&str> = options.keys().map(String::as_str).collect();
                    known.sort_unstable();
                    return Err(format!(
                        "unknown option '{k}' for '{id}' (known: {})",
                        known.join(", ")
                    ));
                }
                options.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(value).map_err(|e| format!("bad options for '{id}': {e}"))
    }

    /// Options as a JSON object (for remembering settings).
    pub fn options_json(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(mut obj)) => match obj.remove("options") {
                Some(Value::Object(options)) => options,
                _ => Map::new(),
            },
            _ => Map::new(),
        }
    }

    /// Whether files of `category` can go through this action.
    pub fn accepts(&self, category: Category) -> bool {
        match self {
            Action::ImageConvert(_)
            | Action::ImageCompress(_)
            | Action::ImageResize(_)
            | Action::ImageClean(_)
            | Action::ImageToPdf(_) => category == Category::Image,
            Action::PdfCompress(_)
            | Action::PdfToImages(_)
            | Action::PdfToWord(_)
            | Action::PdfToText(_)
            | Action::PdfToMarkdown(_)
            | Action::PdfSplit(_) => category == Category::Pdf,
            Action::PdfMerge(_) => matches!(category, Category::Pdf | Category::Image),
            Action::OfficeConvert(_) => category.is_document(),
            Action::OfficeToImages(_) => matches!(category, Category::Word | Category::PowerPoint),
            Action::VideoConvert(_)
            | Action::VideoCompress(_)
            | Action::VideoExtractAudio(_)
            | Action::VideoToGif(_) => category == Category::Video,
            Action::AudioConvert(_) => category == Category::Audio,
        }
    }

    /// One task for all inputs (merge) instead of one per file. The task's
    /// `inputs` keep the order the files were given in (the window's list order).
    pub fn is_multi_input(&self) -> bool {
        matches!(self, Action::PdfMerge(_))
    }

    /// The queue for this action on `input`. Office engine work (COM or
    /// soffice) runs one at a time on its own thread, next to the CPU pool.
    pub fn queue(&self, input: &Path) -> Queue {
        match self {
            Action::OfficeConvert(o) => match crate::office::plan(input, o.format) {
                Ok(plan) if plan.needs_engine() => Queue::Office,
                // Native, or an error that is reported right away.
                _ => Queue::Cpu,
            },
            Action::OfficeToImages(_) | Action::PdfToWord(_) => Queue::Office,
            Action::VideoConvert(_)
            | Action::VideoCompress(_)
            | Action::VideoExtractAudio(_)
            | Action::VideoToGif(_)
            | Action::AudioConvert(_) => Queue::Ffmpeg,
            _ => Queue::Cpu,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::options::OutputFormat;

    #[test]
    fn menu_preset_overrides_defaults() {
        let mut preset = Map::new();
        preset.insert("format".into(), Value::from("png"));
        let action = Action::from_id(actions::IMAGE_CONVERT, &preset).unwrap();
        let Action::ImageConvert(opts) = action else {
            panic!("wrong variant")
        };
        assert_eq!(opts.format, OutputFormat::Png);
        assert!(opts.keep_metadata, "other fields keep their defaults");
    }

    #[test]
    fn round_trips_through_json() {
        for id in [
            actions::IMAGE_CONVERT,
            actions::IMAGE_COMPRESS,
            actions::IMAGE_RESIZE,
            actions::IMAGE_CLEAN,
            actions::IMAGE_TO_PDF,
            actions::PDF_COMPRESS,
            actions::PDF_TO_IMAGES,
            actions::PDF_TO_WORD,
            actions::PDF_TO_TEXT,
            actions::PDF_TO_MARKDOWN,
            actions::PDF_SPLIT,
            actions::PDF_MERGE,
            actions::OFFICE_CONVERT,
            actions::OFFICE_TO_IMAGES,
            actions::VIDEO_CONVERT,
            actions::VIDEO_COMPRESS,
            actions::VIDEO_EXTRACT_AUDIO,
            actions::VIDEO_TO_GIF,
            actions::AUDIO_CONVERT,
        ] {
            let a = Action::default_for(id).unwrap();
            assert_eq!(a.id(), id);
            let back = Action::from_id(id, &a.options_json()).unwrap();
            assert_eq!(back, a);
        }
    }

    #[test]
    fn office_actions_accept_and_queue() {
        use crate::office::DocFormat;
        let mut preset = Map::new();
        preset.insert("format".into(), Value::from("xlsx"));
        let to_xlsx = Action::from_id(actions::OFFICE_CONVERT, &preset).unwrap();
        let Action::OfficeConvert(o) = &to_xlsx else {
            panic!("wrong variant")
        };
        assert_eq!(o.format, DocFormat::Xlsx);
        assert!(to_xlsx.accepts(Category::Text) && to_xlsx.accepts(Category::Excel));
        assert!(!to_xlsx.accepts(Category::Pdf));
        assert_eq!(to_xlsx.queue(Path::new("a.csv")), Queue::Cpu, "native");
        assert_eq!(to_xlsx.queue(Path::new("a.xls")), Queue::Office);
        let images = Action::default_for(actions::OFFICE_TO_IMAGES).unwrap();
        assert!(images.accepts(Category::PowerPoint) && !images.accepts(Category::Excel));
        assert_eq!(images.queue(Path::new("a.pptx")), Queue::Office);
        let word = Action::default_for(actions::PDF_TO_WORD).unwrap();
        assert!(word.accepts(Category::Pdf));
        assert!(word.options_json().is_empty());
        let mut md = Map::new();
        md.insert("format".into(), Value::from("md"));
        let to_md = Action::from_id(actions::OFFICE_CONVERT, &md).unwrap();
        assert_eq!(to_md.queue(Path::new("a.html")), Queue::Cpu);
        assert_eq!(
            to_md.queue(Path::new("a.docx")),
            Queue::Cpu,
            "native reader"
        );
        assert_eq!(to_md.queue(Path::new("a.odt")), Queue::Office);
        assert_eq!(to_md.queue(Path::new("a.xls")), Queue::Cpu);
        assert_eq!(to_md.queue(Path::new("a.pptx")), Queue::Cpu);
        assert_eq!(to_md.queue(Path::new("a.ppt")), Queue::Office);
        let pdf_md = Action::default_for(actions::PDF_TO_MARKDOWN).unwrap();
        assert!(pdf_md.accepts(Category::Pdf) && !pdf_md.accepts(Category::Image));
        assert_eq!(pdf_md.queue(Path::new("a.pdf")), Queue::Cpu);
        let img_pdf = Action::default_for(actions::IMAGE_TO_PDF).unwrap();
        assert!(img_pdf.accepts(Category::Image) && !img_pdf.accepts(Category::Pdf));
        assert!(!img_pdf.is_multi_input());
    }

    #[test]
    fn media_actions_accept_and_queue() {
        use crate::media::options::{AudioFormat, VideoContainer};
        let mut preset = Map::new();
        preset.insert("format".into(), Value::from("webm"));
        let webm = Action::from_id(actions::VIDEO_CONVERT, &preset).unwrap();
        let Action::VideoConvert(o) = &webm else {
            panic!("wrong variant")
        };
        assert_eq!(o.format, VideoContainer::Webm);
        assert!(webm.accepts(Category::Video) && !webm.accepts(Category::Audio));
        assert_eq!(webm.queue(Path::new("a.mp4")), Queue::Ffmpeg);
        let mut m4a = Map::new();
        m4a.insert("format".into(), Value::from("m4a"));
        let audio = Action::from_id(actions::AUDIO_CONVERT, &m4a).unwrap();
        assert!(audio.accepts(Category::Audio) && !audio.accepts(Category::Video));
        assert_eq!(audio.queue(Path::new("a.flac")), Queue::Ffmpeg);
        let extract = Action::default_for(actions::VIDEO_EXTRACT_AUDIO).unwrap();
        let Action::VideoExtractAudio(o) = &extract else {
            panic!("wrong variant")
        };
        assert_eq!(o.format, AudioFormat::Mp3);
        assert!(extract.accepts(Category::Video));
        let gif = Action::default_for(actions::VIDEO_TO_GIF).unwrap();
        assert_eq!(gif.options_json()["width"], 480);
    }

    #[test]
    fn unknown_action_and_bad_option_are_errors() {
        assert!(Action::from_id("nope", &Map::new()).is_err());
        let mut bad = Map::new();
        bad.insert("format".into(), Value::from("psd"));
        assert!(Action::from_id(actions::IMAGE_CONVERT, &bad).is_err());
        let mut typo = Map::new();
        typo.insert("qualty".into(), Value::from(5));
        let err = Action::from_id(actions::IMAGE_CONVERT, &typo).unwrap_err();
        assert!(err.contains("unknown option 'qualty'"), "{err}");
    }
}
