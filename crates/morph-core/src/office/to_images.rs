// SPDX-License-Identifier: GPL-3.0-or-later
//! `office.to_images`: Word pages / PowerPoint slides → PNG/JPG.
//!
//! SPEC §4.5: Word → PDF in the temp folder ([`super::engine_convert::export`]),
//! then render the pages with PDFium (`crate::pdf::pdfium`, as PDF → images).
//! PowerPoint: Microsoft PowerPoint's own `Slide.Export` (one file per slide) or
//! LibreOffice → PDF → PDFium. Output folder `name (slides)/name-01.png` for
//! presentations (`MultiOutput::Slides`), `name (pages)/name-01.png` for
//! documents (names from [`naming::page_file_name`], as PDF → images). The
//! folder is removed again on error or cancel ([`OutputDir`]).

use super::detect::EngineChoice;
use super::engine_convert;
use super::options::{DocFormat, Family, OfficeToImagesOpts};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::io::{self, EncodeOptions};
use crate::job::Task;
use crate::naming::{self, MultiOutput, OutputDir};
use crate::pdf::pdfium;
use crate::pdf::to_images::{MAX_DPI, MIN_DPI};
use image::DynamicImage;
use std::path::{Path, PathBuf};

/// Engine share of the progress bar (the rest is rendering).
const ENGINE_SHARE: f32 = 0.5;
/// Same oxipng preset as PDF → images (fast; pages are flat colour).
const PNG_LEVEL: u8 = 1;

pub fn convert(
    task: &Task,
    input: &Path,
    family: Family,
    opts: &OfficeToImagesOpts,
    engine: &EngineChoice,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    if !family.has_images() {
        return Err(ConvertError::unsupported(
            "Only documents and presentations can be exported as images.",
            input.display(),
        ));
    }
    if !(MIN_DPI..=MAX_DPI).contains(&opts.dpi) {
        return Err(ConvertError::unsupported(
            format!("Choose a DPI between {MIN_DPI} and {MAX_DPI}."),
            format!("dpi {}", opts.dpi),
        ));
    }
    engine_convert::check_not_encrypted(input)?;
    ctx.progress(0.02, None);
    let kind = if family == Family::PowerPoint {
        MultiOutput::Slides
    } else {
        MultiOutput::Pages
    };
    let folder = task.output.folder_for(input);

    let outputs = match engine {
        EngineChoice::MsOffice(app) if family == Family::PowerPoint => {
            let slides =
                engine_convert::ms_office::export_slides(app, input, opts.format, opts.dpi, ctx)?;
            ctx.check_cancel()?;
            publish_all(input, &slides, &folder, kind, opts, ctx)?
        }
        _ => {
            let pdf = engine_convert::export(input, family, DocFormat::Pdf, engine, ctx)?;
            ctx.check_cancel()?;
            ctx.progress(ENGINE_SHARE, None);
            render_pdf(input, &pdf, &folder, kind, opts, ctx)?
        }
    };
    let out = TaskOutput {
        outputs,
        note: None,
    };
    Ok(
        match engine_convert::fallback_note(ctx.office_engine(), engine) {
            Some(note) => out.with_note(note),
            None => out,
        },
    )
}

/// Render every page of the engine's PDF into a new `name (pages|slides)` folder.
fn render_pdf(
    input: &Path,
    pdf: &Path,
    folder: &Path,
    kind: MultiOutput,
    opts: &OfficeToImagesOpts,
    ctx: &TaskCtx<'_>,
) -> Result<Vec<PathBuf>, ConvertError> {
    let doc = pdfium::open(pdf)?;
    let count = pdfium::page_count(&doc);
    if count == 0 {
        return Err(ConvertError::other(
            "This file has no pages to export.",
            "the engine's PDF has 0 pages",
        ));
    }
    let dir = OutputDir::create(folder, &naming::multi_output_dir_name(input, kind))?;
    let format = opts.format.output_format();
    let enc = EncodeOptions {
        png_level: PNG_LEVEL,
        ..EncodeOptions::default()
    };
    let mut files = Vec::with_capacity(count as usize);
    for n in 1..=count {
        ctx.check_cancel()?;
        let rgba = {
            let page = pdfium::page(&doc, n - 1)?;
            pdfium::render_rgba(&page, opts.dpi).map_err(|e| with_page(e, n))?
        };
        // PDFium renders onto white: drop alpha (smaller PNG, JPG needs none).
        let rgb = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).into_rgb8());
        let bytes = io::encode(&rgb, format, &enc, None).map_err(|e| with_page(e, n))?;
        let name = naming::page_file_name(input, n, count, format.extension());
        files.push(naming::write_output(dir.path(), &name, &bytes)?);
        ctx.progress(
            ENGINE_SHARE + (1.0 - ENGINE_SHARE) * n as f32 / count as f32,
            None,
        );
    }
    drop(doc);
    ctx.check_cancel()?;
    dir.commit();
    Ok(files)
}

/// Publish pictures the engine already wrote (PowerPoint slides), in order.
fn publish_all(
    input: &Path,
    pictures: &[PathBuf],
    folder: &Path,
    kind: MultiOutput,
    opts: &OfficeToImagesOpts,
    ctx: &TaskCtx<'_>,
) -> Result<Vec<PathBuf>, ConvertError> {
    if pictures.is_empty() {
        return Err(ConvertError::other(
            "This file has no pages to export.",
            "no slides exported",
        ));
    }
    let dir = OutputDir::create(folder, &naming::multi_output_dir_name(input, kind))?;
    let total = u32::try_from(pictures.len()).unwrap_or(u32::MAX);
    let mut files = Vec::with_capacity(pictures.len());
    for (i, src) in pictures.iter().enumerate() {
        ctx.check_cancel()?;
        let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let name = naming::page_file_name(input, n, total, opts.format.extension());
        files.push(naming::publish_file(src, dir.path(), &name)?);
        ctx.progress(
            ENGINE_SHARE + (1.0 - ENGINE_SHARE) * n as f32 / total as f32,
            None,
        );
    }
    dir.commit();
    Ok(files)
}

fn with_page(mut e: ConvertError, page: u32) -> ConvertError {
    e.details = format!("page {page}: {}", e.details);
    e
}
