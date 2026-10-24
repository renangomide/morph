// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF → images (SPEC §4.2): render the selected pages with PDFium into
//! `name (pages)/name-01.png`.
//!
//! Pipeline: the task thread does every PDFium call (open, load page, render),
//! one page at a time, and hands each bitmap to a few encoder threads through a
//! bounded channel. Encoding (PNG/JPG/WebP) and writing run in parallel with the
//! next render. The number of pages in flight is capped by
//! [`crate::memory::PDF_ESTIMATE`], so huge pages at a high DPI never queue up.
//!
//! The output folder is deleted again if anything fails or the task is cancelled
//! ([`OutputDir`]). `TaskOutput.outputs` lists the page files in page order; the
//! folder is their parent.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::io::{self, EncodeOptions};
use crate::image::options::OutputFormat;
use crate::job::Task;
use crate::memory::PDF_ESTIMATE;
use crate::naming::{MultiOutput, OutputDir, multi_output_dir_name, page_file_name, write_output};
use crate::pdf::options::PdfToImagesOpts;
use crate::pdf::{pages, pdfium};
use image::{DynamicImage, RgbaImage};
use pdfium_render::prelude::{PdfDocument, PdfPageIndex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// oxipng preset for rendered pages. Higher presets try many more filter and
/// compression combinations for a few percent; on 100 pages that breaks the
/// SPEC §10 target (100 pages at 150 DPI in < 30 s).
const PNG_LEVEL: u8 = 1;

/// Lowest and highest DPI accepted (the window offers 72/150/300/custom).
pub const MIN_DPI: u32 = 10;
pub const MAX_DPI: u32 = 2400;

/// Most encoder threads per task (the scheduler may run several tasks at once).
const MAX_ENCODERS: usize = 6;

pub struct PdfToImages;

impl Converter for PdfToImages {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfToImages(opts) = &task.action else {
            return Err(super::wrong_action("PdfToImages"));
        };
        let input = task
            .input()
            .ok_or_else(|| super::wrong_action("PdfToImages"))?;
        if !(MIN_DPI..=MAX_DPI).contains(&opts.dpi) {
            return Err(ConvertError::unsupported(
                format!("Choose a DPI between {MIN_DPI} and {MAX_DPI}."),
                format!("dpi {}", opts.dpi),
            ));
        }
        let doc = pdfium::open(input)?;
        let count = pdfium::page_count(&doc);
        let selected = pages::unique_pages(&pages::parse_ranges(&opts.pages, count)?);
        ctx.check_cancel()?;

        let dir = OutputDir::create(
            &task.output.folder_for(input),
            &multi_output_dir_name(input, MultiOutput::Pages),
        )?;
        let page_bytes = largest_page_bytes(&doc, &selected, opts.dpi);
        let job = Render {
            doc: &doc,
            input,
            opts,
            count,
            folder: dir.path(),
        };
        let outputs = job.run(&selected, page_bytes, ctx)?;
        drop(doc);
        dir.commit();
        Ok(TaskOutput {
            outputs,
            note: None,
        })
    }
}

/// RGBA bytes of the largest selected page at `dpi`, so a huge page anywhere in
/// the selection sizes the pipeline (not just the first one). Uses the page
/// sizes PDFium reads without loading pages; a page whose size can't be read
/// counts as the largest renderable one. Pages over the render cap fail when
/// rendered, so they never count for more than that.
fn largest_page_bytes(doc: &PdfDocument<'_>, selected: &[u32], dpi: u32) -> u64 {
    let scale = f64::from(dpi.max(1)) / 72.0;
    let px = |pt: f32| (f64::from(pt) * scale).round().max(1.0) as u64;
    let pages = doc.pages();
    selected
        .iter()
        .map(|&n| {
            let pixels = PdfPageIndex::try_from(n.saturating_sub(1))
                .ok()
                .and_then(|i| pages.page_size(i).ok())
                .map_or(pdfium::MAX_RENDER_PIXELS, |r| {
                    px(r.width().value).saturating_mul(px(r.height().value))
                });
            pixels.min(pdfium::MAX_RENDER_PIXELS) * 4
        })
        .max()
        .unwrap_or(0)
}

/// `(encoder threads, extra queued pages)` for pages of `page_bytes` RGBA each:
/// everything in flight (rendering + queued + encoding) fits in [`PDF_ESTIMATE`].
fn pipeline_shape(page_bytes: u64, pages: usize) -> (usize, usize) {
    let fit = usize::try_from(PDF_ESTIMATE / page_bytes.max(1)).unwrap_or(usize::MAX);
    // One page is being rendered on the task thread, the rest may be in the queue
    // or with an encoder.
    let in_flight = fit.saturating_sub(1).max(1);
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let encoders = cores.min(MAX_ENCODERS).min(in_flight).min(pages).max(1);
    let queue = (in_flight - encoders.min(in_flight)).min(encoders);
    (encoders, queue)
}

struct Render<'d, 'a> {
    doc: &'d pdfium_render::prelude::PdfDocument<'a>,
    input: &'d Path,
    opts: &'d PdfToImagesOpts,
    count: u32,
    folder: &'d Path,
}

impl Render<'_, '_> {
    /// Render `selected` (1-based) and write them all. Returns the files in page order.
    fn run(
        &self,
        selected: &[u32],
        page_bytes: u64,
        ctx: &TaskCtx<'_>,
    ) -> Result<Vec<PathBuf>, ConvertError> {
        let (encoders, queue) = pipeline_shape(page_bytes, selected.len());
        let format = self.opts.format.output_format();
        let enc = EncodeOptions {
            png_level: PNG_LEVEL,
            ..EncodeOptions::default()
        };
        let total = selected.len().max(1) as f32;
        let done = AtomicU32::new(0);
        let stop = AtomicBool::new(false);
        let (page_tx, page_rx) = crossbeam_channel::bounded::<(u32, RgbaImage)>(queue);
        let (out_tx, out_rx) =
            crossbeam_channel::unbounded::<Result<(u32, PathBuf), ConvertError>>();

        let rendered = std::thread::scope(|s| {
            for _ in 0..encoders {
                let (page_rx, out_tx) = (page_rx.clone(), out_tx.clone());
                let (enc, done, stop) = (&enc, &done, &stop);
                let (input, count, folder) = (self.input, self.count, self.folder);
                s.spawn(move || {
                    for (n, img) in page_rx {
                        if stop.load(Ordering::Relaxed) || ctx.cancel.is_cancelled() {
                            break;
                        }
                        let name = page_file_name(input, n, count, format.extension());
                        let result = encode_page(img, format, enc, folder, &name)
                            .map_err(|e| with_page(e, n));
                        let failed = result.is_err();
                        let _ = out_tx.send(result.map(|path| (n, path)));
                        if failed {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                        ctx.progress(d as f32 / total, None);
                    }
                });
            }
            drop(page_rx);
            drop(out_tx);
            let result = self.render_pages(selected, &page_tx, &stop, ctx);
            if result.is_err() {
                stop.store(true, Ordering::Relaxed);
            }
            drop(page_tx);
            result
            // The scope joins the encoders here, after the channel is closed.
        });

        let mut files = Vec::with_capacity(selected.len());
        let mut first_error = None;
        for r in out_rx.iter() {
            match r {
                Ok(f) => files.push(f),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        ctx.check_cancel()?;
        rendered?;
        if let Some(e) = first_error {
            return Err(e);
        }
        if files.len() != selected.len() {
            return Err(ConvertError::other(
                "Something went wrong while converting this file.",
                format!("{} of {} pages written", files.len(), selected.len()),
            ));
        }
        files.sort_by_key(|(n, _)| *n);
        Ok(files.into_iter().map(|(_, p)| p).collect())
    }

    /// The task-thread half: every PDFium call happens here.
    fn render_pages(
        &self,
        selected: &[u32],
        page_tx: &crossbeam_channel::Sender<(u32, RgbaImage)>,
        stop: &AtomicBool,
        ctx: &TaskCtx<'_>,
    ) -> Result<(), ConvertError> {
        for &n in selected {
            ctx.check_cancel()?;
            if stop.load(Ordering::Relaxed) {
                break; // an encoder failed; its error is reported
            }
            let img = {
                let page = pdfium::page(self.doc, n - 1)?;
                pdfium::render_rgba(&page, self.opts.dpi).map_err(|e| with_page(e, n))?
            };
            if page_tx.send((n, img)).is_err() {
                break; // every encoder stopped (error); reported by the caller
            }
        }
        Ok(())
    }
}

/// Encode one rendered page and write it safely into `folder`.
fn encode_page(
    img: RgbaImage,
    format: OutputFormat,
    enc: &EncodeOptions,
    folder: &Path,
    name: &str,
) -> Result<PathBuf, ConvertError> {
    // PDFium renders onto white, so the page is opaque: drop the alpha channel
    // (smaller PNGs, less work, and JPG needs no flattening).
    let rgb = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(img).into_rgb8());
    let bytes = io::encode(&rgb, format, enc, None)?;
    write_output(folder, name, &bytes)
}

fn with_page(mut e: ConvertError, page: u32) -> ConvertError {
    e.details = format!("page {page}: {}", e.details);
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_fits_the_memory_estimate() {
        // A letter page at 150 DPI (~8 MB): several encoders, a short queue.
        let (enc, queue) = pipeline_shape(1275 * 1650 * 4, 100);
        assert!((1..=MAX_ENCODERS).contains(&enc) && queue <= enc);
        // A page bigger than the whole estimate: one at a time, no queue.
        assert_eq!(pipeline_shape(PDF_ESTIMATE * 2, 100), (1, 0));
        // One page: one encoder.
        assert_eq!(pipeline_shape(1000, 1).0, 1);
    }

    /// A small first page and a huge second one: the huge one sizes the pipeline.
    #[test]
    fn largest_selected_page_sizes_the_pipeline() {
        use lopdf::{Document, Object, dictionary};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed-sizes.pdf");
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let kids: Vec<Object> = [(612, 792), (5000, 4000)]
            .iter()
            .map(|&(w, h)| {
                doc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                })
                .into()
            })
            .collect();
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        doc.save(&path).unwrap();

        let pdf = pdfium::open(&path).unwrap();
        assert_eq!(largest_page_bytes(&pdf, &[1], 72), 612 * 792 * 4);
        assert_eq!(largest_page_bytes(&pdf, &[1, 2], 72), 5000 * 4000 * 4);
        // Capped at what can be rendered at all.
        assert_eq!(
            largest_page_bytes(&pdf, &[1, 2], 2400),
            pdfium::MAX_RENDER_PIXELS * 4
        );
        let (encoders, queue) = pipeline_shape(largest_page_bytes(&pdf, &[1, 2], 300), 2);
        assert_eq!((encoders, queue), (1, 0));
    }
}
