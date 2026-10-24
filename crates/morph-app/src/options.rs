// SPDX-License-Identifier: GPL-3.0-or-later
//! Image option panels (SPEC §3.2, §4.1) as plain Rust, apart from Slint.
//!
//! [`ImageForm`] keeps one options struct per image action, so switching the
//! action selector back and forth keeps what the user typed. [`FormValues`] is
//! the flat, widget-shaped view of it (indexes, numbers, text) that the Slint
//! window binds to; [`FormValues::apply`] turns it back into real options.
//! Options without a widget (e.g. PNG optimization level, a Settings default)
//! keep their value from the form.

use crate::pdf_options::{MAX_MARGIN_MM, PAGE_SIZES};
use crate::strings;
use morph_core::Action;
use morph_core::image::options::{
    CompressMode, DEFAULT_ICO_SIZES, ImageCleanOpts, ImageCompressOpts, ImageConvertOpts,
    ImageResizeOpts, ImageToPdfOpts, OutputFormat, ResizeMode, Rotation, StripMode,
};
use morph_core::state::State;
use morph_formats::actions;
use serde_json::{Map, Value};

/// The image actions. The selector shows the first four ([`ImageKind::SELECTOR`]);
/// `ToPdf` is Convert with the "PDF" target ([`PDF_FORMAT`]) chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageKind {
    #[default]
    Convert,
    Compress,
    Resize,
    Clean,
    ToPdf,
}

impl ImageKind {
    pub const ALL: [ImageKind; 5] = [
        ImageKind::Convert,
        ImageKind::Compress,
        ImageKind::Resize,
        ImageKind::Clean,
        ImageKind::ToPdf,
    ];
    /// The action selector buttons, in display order.
    pub const SELECTOR: [ImageKind; 4] = [
        ImageKind::Convert,
        ImageKind::Compress,
        ImageKind::Resize,
        ImageKind::Clean,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ImageKind::Convert => actions::IMAGE_CONVERT,
            ImageKind::Compress => actions::IMAGE_COMPRESS,
            ImageKind::Resize => actions::IMAGE_RESIZE,
            ImageKind::Clean => actions::IMAGE_CLEAN,
            ImageKind::ToPdf => actions::IMAGE_TO_PDF,
        }
    }

    pub fn from_id(id: &str) -> Option<ImageKind> {
        ImageKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn index(self) -> i32 {
        match self {
            ImageKind::Convert | ImageKind::ToPdf => 0,
            ImageKind::Compress => 1,
            ImageKind::Resize => 2,
            ImageKind::Clean => 3,
        }
    }

    /// Out-of-range indexes mean Convert.
    pub fn from_index(index: i32) -> ImageKind {
        usize::try_from(index)
            .ok()
            .and_then(|i| ImageKind::SELECTOR.get(i).copied())
            .unwrap_or(ImageKind::Convert)
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageKind::Convert | ImageKind::ToPdf => strings::ACTION_CONVERT,
            ImageKind::Compress => strings::ACTION_COMPRESS,
            ImageKind::Resize => strings::ACTION_RESIZE,
            ImageKind::Clean => strings::ACTION_CLEAN,
        }
    }
}

/// Options of every image action, plus the selected one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageForm {
    pub kind: ImageKind,
    pub convert: ImageConvertOpts,
    pub compress: ImageCompressOpts,
    pub resize: ImageResizeOpts,
    pub clean: ImageCleanOpts,
    pub to_pdf: ImageToPdfOpts,
}

impl ImageForm {
    /// Pre-fill the window: `kind` is selected; every action gets defaults →
    /// remembered options (`state`) → and, for `kind` only, the menu preset on top.
    pub fn prefill(state: &State, kind: ImageKind, preset: &Map<String, Value>) -> ImageForm {
        let mut form = ImageForm {
            kind,
            ..ImageForm::default()
        };
        let empty = Map::new();
        for k in ImageKind::ALL {
            let preset = if k == kind { preset } else { &empty };
            if let Some(action) = state.action_for(k.id(), preset) {
                form.set_action(action);
            }
        }
        form
    }

    /// The selected action with its options.
    pub fn action(&self) -> Action {
        match self.kind {
            ImageKind::Convert => Action::ImageConvert(self.convert.clone()),
            ImageKind::Compress => Action::ImageCompress(self.compress.clone()),
            ImageKind::Resize => Action::ImageResize(self.resize.clone()),
            ImageKind::Clean => Action::ImageClean(self.clean.clone()),
            ImageKind::ToPdf => Action::ImageToPdf(self.to_pdf.clone()),
        }
    }

    /// Store `action`'s options (does not change the selected kind).
    pub fn set_action(&mut self, action: Action) {
        match action {
            Action::ImageConvert(o) => self.convert = o,
            Action::ImageCompress(o) => self.compress = o,
            Action::ImageResize(o) => self.resize = o,
            Action::ImageClean(o) => self.clean = o,
            Action::ImageToPdf(o) => self.to_pdf = o,
            // Not an image action: nothing to store in the image form.
            _ => {}
        }
    }
}

// Widget indexes. Kept here so the Slint lists and this mapping can't drift.
pub const BACKGROUNDS: [[u8; 3]; 2] = [[255, 255, 255], [0, 0, 0]];
pub const ROTATIONS: [Rotation; 4] = [
    Rotation::None,
    Rotation::R90,
    Rotation::R180,
    Rotation::R270,
];
pub const STRIPS: [StripMode; 3] = [StripMode::All, StripMode::GpsOnly, StripMode::None];
const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;
const DEFAULT_TARGET_BYTES: u64 = 500 * KB;
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;
/// Slider position shown while Compress uses each format's default quality.
const COMPRESS_DISPLAY_QUALITY: i32 = 80;

/// Index of "PDF" in the Convert format list (after `OutputFormat::ALL`).
pub const PDF_FORMAT: i32 = OutputFormat::ALL.len() as i32;

/// Compress mode radio buttons, in display order (Best first, the default).
pub const COMPRESS_BEST: i32 = 0;
pub const COMPRESS_QUALITY: i32 = 1;
pub const COMPRESS_TARGET: i32 = 2;
pub const UNIT_KB: i32 = 0;
pub const UNIT_MB: i32 = 1;
pub const RESIZE_PERCENT: i32 = 0;
pub const RESIZE_MAX_WIDTH: i32 = 1;
pub const RESIZE_MAX_HEIGHT: i32 = 2;
pub const RESIZE_EXACT: i32 = 3;

/// Flat values of the option widgets.
#[derive(Debug, Clone, PartialEq)]
pub struct FormValues {
    pub kind: i32,

    // Convert
    /// Index in `OutputFormat::ALL`.
    pub format: i32,
    pub quality: i32,
    /// `false`: the format's default quality (`quality: None`).
    pub quality_custom: bool,
    pub lossless: bool,
    /// Index in [`BACKGROUNDS`].
    pub background: i32,
    /// One flag per entry of `DEFAULT_ICO_SIZES`.
    pub ico_sizes: Vec<bool>,
    pub svg_width: i32,
    pub convert_keep_metadata: bool,
    pub convert_remove_gps: bool,

    // Convert → PDF
    /// Index in [`PAGE_SIZES`].
    pub pdf_page_size: i32,
    pub pdf_margin_mm: i32,

    // Compress
    pub compress_mode: i32,
    pub compress_quality: i32,
    pub compress_quality_custom: bool,
    pub target_value: String,
    pub target_unit: i32,
    pub convert_to_webp: bool,
    pub png_lossy: bool,
    pub compress_keep_metadata: bool,
    pub compress_remove_gps: bool,

    // Resize
    pub resize_mode: i32,
    pub percent: i32,
    pub width: i32,
    pub height: i32,
    pub keep_aspect: bool,
    pub no_enlarge: bool,
    pub resize_keep_metadata: bool,
    pub resize_remove_gps: bool,

    // Clean
    /// Index in [`STRIPS`].
    pub strip: i32,
    /// Index in [`ROTATIONS`].
    pub rotate: i32,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

fn index_of<T: PartialEq>(list: &[T], value: &T) -> i32 {
    list.iter()
        .position(|v| v == value)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(0)
}

fn pick<T: Copy>(list: &[T], index: i32) -> Option<T> {
    usize::try_from(index)
        .ok()
        .and_then(|i| list.get(i).copied())
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

fn dimension(v: i32) -> Result<u32, &'static str> {
    match u32::try_from(v) {
        Ok(v @ 1..=65535) => Ok(v),
        _ => Err(strings::BAD_DIMENSION),
    }
}

impl FormValues {
    pub fn from_form(form: &ImageForm) -> FormValues {
        let c = &form.convert;
        let p = &form.compress;
        let r = &form.resize;
        let k = &form.clean;

        let (compress_mode, compress_quality, compress_quality_custom, target_bytes) = match p.mode
        {
            CompressMode::Best => (
                COMPRESS_BEST,
                COMPRESS_DISPLAY_QUALITY,
                false,
                DEFAULT_TARGET_BYTES,
            ),
            CompressMode::Quality { quality: Some(q) } => {
                (COMPRESS_QUALITY, i32::from(q), true, DEFAULT_TARGET_BYTES)
            }
            CompressMode::Quality { quality: None } => (
                COMPRESS_QUALITY,
                COMPRESS_DISPLAY_QUALITY,
                false,
                DEFAULT_TARGET_BYTES,
            ),
            CompressMode::TargetSize { bytes } => {
                (COMPRESS_TARGET, COMPRESS_DISPLAY_QUALITY, false, bytes)
            }
        };
        let (target_value, target_unit) = format_target(target_bytes);

        let (resize_mode, percent, width, height) = match r.mode {
            ResizeMode::Percent { percent } => (
                RESIZE_PERCENT,
                percent.round() as i32,
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
            ),
            ResizeMode::MaxWidth { width } => (RESIZE_MAX_WIDTH, 50, width, DEFAULT_HEIGHT),
            ResizeMode::MaxHeight { height } => (RESIZE_MAX_HEIGHT, 50, DEFAULT_WIDTH, height),
            ResizeMode::Exact { width, height } => (RESIZE_EXACT, 50, width, height),
        };

        FormValues {
            kind: form.kind.index(),
            format: if form.kind == ImageKind::ToPdf {
                PDF_FORMAT
            } else {
                index_of(&OutputFormat::ALL, &c.format)
            },
            quality: i32::from(c.quality.unwrap_or_else(|| c.format.default_quality())),
            quality_custom: c.quality.is_some(),
            lossless: c.lossless,
            background: index_of(&BACKGROUNDS, &c.background),
            ico_sizes: DEFAULT_ICO_SIZES
                .iter()
                .map(|s| c.ico_sizes.contains(s))
                .collect(),
            svg_width: to_i32(c.svg_width),
            convert_keep_metadata: c.keep_metadata,
            convert_remove_gps: c.remove_gps,

            pdf_page_size: index_of(&PAGE_SIZES, &form.to_pdf.page_size),
            pdf_margin_mm: to_i32(form.to_pdf.margin_mm),

            compress_mode,
            compress_quality,
            compress_quality_custom,
            target_value,
            target_unit,
            convert_to_webp: p.convert_to_webp,
            png_lossy: p.png_lossy,
            compress_keep_metadata: p.keep_metadata,
            compress_remove_gps: p.remove_gps,

            resize_mode,
            percent,
            width: to_i32(width),
            height: to_i32(height),
            keep_aspect: r.keep_aspect,
            no_enlarge: r.no_enlarge,
            resize_keep_metadata: r.keep_metadata,
            resize_remove_gps: r.remove_gps,

            strip: index_of(&STRIPS, &k.strip),
            rotate: index_of(&ROTATIONS, &k.rotate),
            flip_horizontal: k.flip_horizontal,
            flip_vertical: k.flip_vertical,
        }
    }

    /// Write the widget values into `form`. Only the selected action's values
    /// are checked; an error is a plain-English message for the window.
    /// The other actions are updated best effort (invalid values keep the old option).
    pub fn apply(&self, form: &mut ImageForm) -> Result<(), &'static str> {
        form.kind = match ImageKind::from_index(self.kind) {
            ImageKind::Convert if self.format == PDF_FORMAT => ImageKind::ToPdf,
            kind => kind,
        };
        let convert = self.convert_opts(&form.convert);
        let to_pdf = self.to_pdf_opts(&form.to_pdf);
        let compress = self.compress_opts(&form.compress);
        let resize = self.resize_opts();
        form.clean = self.clean_opts(&form.clean);
        let check = match form.kind {
            ImageKind::Convert => convert.as_ref().err().copied(),
            ImageKind::Compress => compress.as_ref().err().copied(),
            ImageKind::Resize => resize.as_ref().err().copied(),
            ImageKind::Clean => None,
            ImageKind::ToPdf => to_pdf.as_ref().err().copied(),
        };
        if let Ok(o) = to_pdf {
            form.to_pdf = o;
        }
        if let Ok(o) = convert {
            form.convert = o;
        }
        if let Ok(o) = compress {
            form.compress = o;
        }
        if let Ok(o) = resize {
            form.resize = o;
        }
        check.map_or(Ok(()), Err)
    }

    fn convert_opts(&self, base: &ImageConvertOpts) -> Result<ImageConvertOpts, &'static str> {
        let format = pick(&OutputFormat::ALL, self.format).unwrap_or(base.format);
        let ico_sizes: Vec<u32> = DEFAULT_ICO_SIZES
            .iter()
            .zip(&self.ico_sizes)
            .filter(|(_, on)| **on)
            .map(|(s, _)| *s)
            .collect();
        if format == OutputFormat::Ico && ico_sizes.is_empty() {
            return Err(strings::BAD_ICO_SIZES);
        }
        Ok(ImageConvertOpts {
            format,
            quality: if self.quality_custom {
                Some(quality(self.quality))
            } else {
                None
            },
            lossless: self.lossless,
            background: pick(&BACKGROUNDS, self.background).unwrap_or(base.background),
            ico_sizes: if ico_sizes.is_empty() {
                base.ico_sizes.clone()
            } else {
                ico_sizes
            },
            svg_width: dimension(self.svg_width)?,
            keep_metadata: self.convert_keep_metadata,
            remove_gps: self.convert_remove_gps,
        })
    }

    fn to_pdf_opts(&self, base: &ImageToPdfOpts) -> Result<ImageToPdfOpts, &'static str> {
        Ok(ImageToPdfOpts {
            page_size: pick(&PAGE_SIZES, self.pdf_page_size).unwrap_or(base.page_size),
            margin_mm: match u32::try_from(self.pdf_margin_mm) {
                Ok(v) if v <= MAX_MARGIN_MM => v,
                _ => return Err(strings::BAD_MARGIN),
            },
        })
    }

    fn compress_opts(&self, base: &ImageCompressOpts) -> Result<ImageCompressOpts, &'static str> {
        let mode = match self.compress_mode {
            COMPRESS_TARGET => CompressMode::TargetSize {
                bytes: parse_target(&self.target_value, self.target_unit)
                    .ok_or(strings::BAD_TARGET_SIZE)?,
            },
            COMPRESS_QUALITY => CompressMode::Quality {
                quality: self
                    .compress_quality_custom
                    .then(|| quality(self.compress_quality)),
            },
            _ => CompressMode::Best,
        };
        Ok(ImageCompressOpts {
            mode,
            convert_to_webp: self.convert_to_webp,
            png_lossy: self.png_lossy,
            keep_metadata: self.compress_keep_metadata,
            remove_gps: self.compress_remove_gps,
            ..base.clone()
        })
    }

    fn resize_opts(&self) -> Result<ImageResizeOpts, &'static str> {
        let mode = match self.resize_mode {
            RESIZE_MAX_WIDTH => ResizeMode::MaxWidth {
                width: dimension(self.width)?,
            },
            RESIZE_MAX_HEIGHT => ResizeMode::MaxHeight {
                height: dimension(self.height)?,
            },
            RESIZE_EXACT => ResizeMode::Exact {
                width: dimension(self.width)?,
                height: dimension(self.height)?,
            },
            _ => {
                if !(1..=1000).contains(&self.percent) {
                    return Err(strings::BAD_PERCENT);
                }
                ResizeMode::Percent {
                    percent: self.percent as f32,
                }
            }
        };
        Ok(ImageResizeOpts {
            mode,
            keep_aspect: self.keep_aspect,
            no_enlarge: self.no_enlarge,
            keep_metadata: self.resize_keep_metadata,
            remove_gps: self.resize_remove_gps,
        })
    }

    fn clean_opts(&self, base: &ImageCleanOpts) -> ImageCleanOpts {
        ImageCleanOpts {
            strip: pick(&STRIPS, self.strip).unwrap_or(base.strip),
            rotate: pick(&ROTATIONS, self.rotate).unwrap_or(base.rotate),
            flip_horizontal: self.flip_horizontal,
            flip_vertical: self.flip_vertical,
        }
    }
}

impl FormValues {
    /// Convert left at "format default": the slider shows `default_for(format)`
    /// (Settings → Defaults), the quality the job will use.
    pub fn show_default_quality(&mut self, default_for: impl Fn(OutputFormat) -> u8) {
        if self.quality_custom {
            return;
        }
        if let Some(format) = usize::try_from(self.format)
            .ok()
            .and_then(|i| OutputFormat::ALL.get(i))
        {
            self.quality = i32::from(default_for(*format));
        }
    }
}

fn quality(v: i32) -> u8 {
    u8::try_from(v.clamp(1, 100)).unwrap_or(80)
}

/// Target size as the text box shows it: 500 KB → ("500", KB), 1.5 MB → ("1.5", MB).
pub fn format_target(bytes: u64) -> (String, i32) {
    let (value, unit) = if bytes >= MB {
        (bytes as f64 / MB as f64, UNIT_MB)
    } else {
        (bytes as f64 / KB as f64, UNIT_KB)
    };
    let mut text = format!("{value:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    (text, unit)
}

/// Parse the target size box ("500", "1.5", "1,5") in `unit`. `None` if not a size > 0.
pub fn parse_target(text: &str, unit: i32) -> Option<u64> {
    let value: f64 = text.trim().replace(',', ".").parse().ok()?;
    let scale = if unit == UNIT_MB { MB } else { KB };
    let bytes = (value * scale as f64).round();
    (value.is_finite() && (1.0..1e15).contains(&bytes)).then_some(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use morph_core::pdf::options::PageSize;
    use morph_core::state::State;

    fn round_trip(form: &ImageForm) -> ImageForm {
        let values = FormValues::from_form(form);
        let mut back = ImageForm::default();
        // Fields without a widget come from the form itself.
        back.compress.png_level = form.compress.png_level;
        values.apply(&mut back).unwrap();
        back
    }

    #[test]
    fn defaults_round_trip() {
        for kind in ImageKind::ALL {
            let form = ImageForm {
                kind,
                ..ImageForm::default()
            };
            assert_eq!(round_trip(&form), form, "{kind:?}");
        }
    }

    #[test]
    fn non_default_options_round_trip() {
        let form = ImageForm {
            kind: ImageKind::Compress,
            convert: ImageConvertOpts {
                format: OutputFormat::Ico,
                quality: Some(42),
                lossless: true,
                background: [0, 0, 0],
                ico_sizes: vec![16, 256],
                svg_width: 2048,
                keep_metadata: false,
                remove_gps: true,
            },
            compress: ImageCompressOpts {
                mode: CompressMode::TargetSize { bytes: 1536 * 1024 },
                convert_to_webp: true,
                png_lossy: true,
                png_level: 5,
                keep_metadata: false,
                remove_gps: true,
            },
            resize: ImageResizeOpts {
                mode: ResizeMode::Exact {
                    width: 800,
                    height: 600,
                },
                keep_aspect: false,
                no_enlarge: false,
                keep_metadata: false,
                remove_gps: true,
            },
            clean: ImageCleanOpts {
                strip: StripMode::GpsOnly,
                rotate: Rotation::R270,
                flip_horizontal: true,
                flip_vertical: true,
            },
            to_pdf: ImageToPdfOpts {
                page_size: PageSize::Letter,
                margin_mm: 12,
            },
        };
        assert_eq!(round_trip(&form), form);
        // PDF hides the image format; the window applies onto its own form,
        // which keeps it.
        let pdf = ImageForm {
            kind: ImageKind::ToPdf,
            ..form.clone()
        };
        let mut back = form.clone();
        FormValues::from_form(&pdf).apply(&mut back).unwrap();
        assert_eq!(back, pdf);

        let mut other = form.clone();
        other.compress.mode = CompressMode::Best;
        assert_eq!(round_trip(&other), other);
        other.compress.mode = CompressMode::Quality { quality: None };
        assert_eq!(round_trip(&other), other);
        other.compress.mode = CompressMode::Quality { quality: Some(55) };
        other.resize.mode = ResizeMode::MaxHeight { height: 720 };
        assert_eq!(round_trip(&other), other);
        other.resize.mode = ResizeMode::MaxWidth { width: 1280 };
        assert_eq!(round_trip(&other), other);
        other.resize.mode = ResizeMode::Percent { percent: 25.0 };
        assert_eq!(round_trip(&other), other);
    }

    #[test]
    fn widget_values_map_to_options() {
        let mut form = ImageForm::default();
        let mut v = FormValues::from_form(&form);
        assert_eq!(v.quality, 0, "PNG has no quality");
        assert_eq!(
            v.compress_mode, COMPRESS_BEST,
            "Best is selected by default"
        );
        v.kind = ImageKind::Convert.index();
        v.format = index_of(&OutputFormat::ALL, &OutputFormat::Webp);
        v.quality = 70;
        v.quality_custom = true;
        v.apply(&mut form).unwrap();
        let Action::ImageConvert(o) = form.action() else {
            panic!("wrong action")
        };
        assert_eq!((o.format, o.quality), (OutputFormat::Webp, Some(70)));

        v.kind = ImageKind::Compress.index();
        v.compress_mode = COMPRESS_TARGET;
        v.target_value = "1,5".into();
        v.target_unit = UNIT_MB;
        v.apply(&mut form).unwrap();
        assert_eq!(
            form.compress.mode,
            CompressMode::TargetSize { bytes: 1_572_864 }
        );
    }

    #[test]
    fn invalid_values_only_block_the_selected_action() {
        let mut form = ImageForm::default();
        let mut v = FormValues::from_form(&form);
        v.compress_mode = COMPRESS_TARGET;
        v.target_value = "abc".into();
        assert_eq!(v.apply(&mut form), Ok(()), "Convert is selected");
        v.kind = ImageKind::Compress.index();
        assert_eq!(v.apply(&mut form), Err(strings::BAD_TARGET_SIZE));
        v.kind = ImageKind::Resize.index();
        v.percent = 0;
        assert_eq!(v.apply(&mut form), Err(strings::BAD_PERCENT));
        v.kind = ImageKind::Convert.index();
        v.format = index_of(&OutputFormat::ALL, &OutputFormat::Ico);
        v.ico_sizes = vec![false; 6];
        assert_eq!(v.apply(&mut form), Err(strings::BAD_ICO_SIZES));
    }

    #[test]
    fn target_size_text() {
        assert_eq!(format_target(500 * 1024), ("500".into(), UNIT_KB));
        assert_eq!(format_target(1536 * 1024), ("1.5".into(), UNIT_MB));
        assert_eq!(format_target(2 * MB), ("2".into(), UNIT_MB));
        assert_eq!(parse_target(" 500 ", UNIT_KB), Some(512_000));
        assert_eq!(parse_target("0", UNIT_KB), None);
        assert_eq!(parse_target("-3", UNIT_MB), None);
        assert_eq!(parse_target("", UNIT_MB), None);
        assert_eq!(parse_target("inf", UNIT_MB), None);
    }

    #[test]
    fn prefill_precedence_defaults_remembered_preset() {
        let mut state = State::default();
        let mut remembered = Map::new();
        remembered.insert("format".into(), Value::from("avif"));
        remembered.insert("quality".into(), Value::from(42));
        state
            .actions
            .insert(actions::IMAGE_CONVERT.into(), remembered);
        let mut resize = Map::new();
        resize.insert(
            "mode".into(),
            serde_json::json!({"type": "max_width", "width": 1280}),
        );
        state.actions.insert(actions::IMAGE_RESIZE.into(), resize);

        // Remembered only.
        let form = ImageForm::prefill(&state, ImageKind::Convert, &Map::new());
        assert_eq!(form.kind, ImageKind::Convert);
        assert_eq!(
            (form.convert.format, form.convert.quality),
            (OutputFormat::Avif, Some(42))
        );
        assert_eq!(form.resize.mode, ResizeMode::MaxWidth { width: 1280 });

        // Menu preset wins for the requested action only.
        let mut preset = Map::new();
        preset.insert("format".into(), Value::from("png"));
        let form = ImageForm::prefill(&state, ImageKind::Convert, &preset);
        assert_eq!(
            (form.convert.format, form.convert.quality),
            (OutputFormat::Png, Some(42))
        );
        let mut strip = Map::new();
        strip.insert("strip".into(), Value::from("gps_only"));
        let form = ImageForm::prefill(&state, ImageKind::Resize, &strip);
        assert_eq!(form.clean.strip, StripMode::All, "preset is for Resize");

        // Remember off → defaults (+ preset).
        state.remember = false;
        let form = ImageForm::prefill(&state, ImageKind::Convert, &preset);
        assert_eq!(
            form.convert,
            ImageConvertOpts {
                format: OutputFormat::Png,
                ..Default::default()
            }
        );
        assert_eq!(form.resize, ImageResizeOpts::default());
    }

    #[test]
    fn pdf_is_a_convert_target() {
        let mut form = ImageForm::default();
        let mut v = FormValues::from_form(&form);
        v.kind = ImageKind::Convert.index();
        v.format = PDF_FORMAT;
        v.pdf_page_size = index_of(&PAGE_SIZES, &PageSize::A4);
        v.pdf_margin_mm = 10;
        v.apply(&mut form).unwrap();
        assert_eq!(
            form.action(),
            Action::ImageToPdf(ImageToPdfOpts {
                page_size: PageSize::A4,
                margin_mm: 10,
            })
        );
        assert_eq!(form.action().id(), actions::IMAGE_TO_PDF);
        v.pdf_margin_mm = 101;
        assert_eq!(v.apply(&mut form), Err(strings::BAD_MARGIN));
        // Another format: back to image.convert.
        v.format = index_of(&OutputFormat::ALL, &OutputFormat::Webp);
        v.apply(&mut form).unwrap();
        assert_eq!(form.action().id(), actions::IMAGE_CONVERT);
        // The menu entry pre-selects PDF in the Convert panel.
        let form = ImageForm::prefill(&State::default(), ImageKind::ToPdf, &Map::new());
        let v = FormValues::from_form(&form);
        assert_eq!((v.kind, v.format), (ImageKind::Convert.index(), PDF_FORMAT));
    }

    #[test]
    fn to_pdf_settings_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let opts = ImageToPdfOpts {
            page_size: PageSize::Letter,
            margin_mm: 7,
        };
        let mut state = State::default();
        state.remember_action(&Action::ImageToPdf(opts.clone()));
        state.save(&path).unwrap();
        let form = ImageForm::prefill(&State::load(&path), ImageKind::ToPdf, &Map::new());
        assert_eq!(form.to_pdf, opts);
        assert_eq!(form.action(), Action::ImageToPdf(opts));
    }

    #[test]
    fn kind_indexes() {
        for k in ImageKind::SELECTOR {
            assert_eq!(ImageKind::from_index(k.index()), k);
        }
        for k in ImageKind::ALL {
            assert_eq!(ImageKind::from_id(k.id()), Some(k));
        }
        assert_eq!(ImageKind::from_index(99), ImageKind::Convert);
        assert_eq!(ImageKind::from_id("pdf.split"), None);
    }

    #[test]
    fn format_default_quality_shows_the_settings_value() {
        let mut settings = morph_core::Settings::default();
        settings.defaults.webp_quality = 70;
        let mut form = ImageForm::default();
        form.convert.format = OutputFormat::Webp;
        let mut v = FormValues::from_form(&form);
        v.show_default_quality(|f| settings.image_quality(f));
        assert_eq!(v.quality, 70);
        // Still "format default": the job fills in the Settings quality.
        form = ImageForm::default();
        v.apply(&mut form).unwrap();
        assert_eq!(form.convert.quality, None);
        // A chosen quality stays.
        form.convert.quality = Some(42);
        let mut v = FormValues::from_form(&form);
        v.show_default_quality(|f| settings.image_quality(f));
        assert_eq!(v.quality, 42);
    }
}
