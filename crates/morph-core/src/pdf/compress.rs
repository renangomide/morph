// SPDX-License-Identifier: GPL-3.0-or-later
//! Compress (SPEC §4.2), with lopdf: downscale embedded raster images to the
//! preset's DPI and re-encode them as JPEG (skip images that would get bigger),
//! compress uncompressed streams, prune unused objects, use object streams.
//! Not smaller → no output, note "Already optimized".
//!
//! Steps:
//! 1. Load with lopdf. A PDF that opens with the empty password is decrypted;
//!    if its owner set usage restrictions (no printing, copying, …) it is left
//!    alone with a note, since the copy would be saved without them. Any other
//!    password → error. Digitally signed PDFs are left alone with a note too
//!    (rewriting them would invalidate the signature).
//! 2. [`usage`]: walk every page (and the forms it draws) to find the largest
//!    size each image is shown at.
//! 3. [`images`]: per image, in parallel (bounded batches): downscale above the
//!    target DPI, re-encode (JPEG for photos, Flate for soft masks and flat
//!    graphics), keep only if smaller. Unsupported images (JPX, JBIG2, CCITT,
//!    CMYK, indexed, 16-bit, stencil masks, …) are left as they are.
//! 4. Deflate uncompressed streams, re-deflate plain Flate streams at the best
//!    level, prune unused objects, save with object and xref streams.
//! 5. Check the result opens in PDFium with the same page count, then write it
//!    (temp file + rename) only if it is smaller than the input.

mod images;
mod usage;

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::image::compress::ALREADY_OPTIMIZED_NOTE;
use crate::job::Task;
use crate::naming::{self, OutputFile, Suffix};
use crate::pdf::pdfium;
use images::{Outcome, Role, Target};
use lopdf::{Document, LoadOptions, Object, ObjectId, Permissions, SaveOptions, Stream};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;

pub struct PdfCompress;

/// Streams that decode to more than this are left as they are.
const MAX_STREAM_BYTES: usize = 256 * 1024 * 1024;

/// Flate streams that inflate to more than this aren't re-deflated (left as
/// they are), so one stream holds at most this plus its new copy.
const REDEFLATE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Streams re-deflated at once: with [`REDEFLATE_MAX_BYTES`] this bounds the
/// pass to about 4 × 2 × 16 MB = 128 MB in flight, within the PDF estimate.
const REDEFLATE_THREADS: usize = 4;

/// Pixels decoded at once across parallel image jobs (bounds memory: about
/// 3 bytes per pixel, a few copies each). A larger image runs alone.
const BATCH_PIXELS: u64 = 24_000_000;

/// The result must be at least this much smaller than the input to be kept.
const MIN_GAIN: f64 = 0.99;

/// Note for a PDF that has usage restrictions: Morph doesn't strip them.
pub const RESTRICTED_NOTE: &str = "This PDF has usage restrictions, so it was left unchanged.";

/// What the image pass did, for tests and diagnostics.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Stats {
    replaced: usize,
    unchanged: usize,
    /// Why each left-alone image was skipped.
    skipped: Vec<&'static str>,
}

fn load(input: &Path) -> Result<Document, ConvertError> {
    let options = LoadOptions {
        max_decompressed_size: Some(MAX_STREAM_BYTES),
        ..LoadOptions::default()
    };
    let mut doc = Document::load_with_options(input, options).map_err(|e| match e {
        lopdf::Error::IO(io) => ConvertError::from(io),
        lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_) => {
            pdfium::password_protected(format!("lopdf: {e}"))
        }
        other => pdfium::damaged_pdf(format!("lopdf: {other}")),
    })?;
    // lopdf decrypts on load when the empty user password works; otherwise the
    // document is still encrypted here.
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Err(pdfium::password_protected("lopdf: needs a password"));
    }
    if doc.get_pages().is_empty() {
        return Err(pdfium::damaged_pdf("lopdf: no pages found"));
    }
    Ok(doc)
}

/// The PDF was encrypted (opened with the empty password) and its owner
/// withheld some permission. The compressed copy would be saved unencrypted,
/// dropping the restrictions, so such files are skipped.
fn has_restrictions(doc: &Document) -> bool {
    doc.encryption_state
        .as_ref()
        .is_some_and(|s| !s.permissions().contains(Permissions::all()))
}

/// Note for a digitally signed PDF: rewriting it would break the signature.
pub const SIGNED_NOTE: &str =
    "This PDF is digitally signed, so it was left unchanged to keep the signature valid.";

/// Signatures exist (`/AcroForm /SigFlags` bit 1) or the catalog has `/Perms`
/// (DocMDP / UR signatures).
fn is_signed(doc: &Document) -> bool {
    let Ok(catalog) = doc.catalog() else {
        return false;
    };
    if catalog.has(b"Perms") {
        return true;
    }
    catalog
        .get(b"AcroForm")
        .and_then(|o| doc.dereference(o))
        .and_then(|(_, o)| o.as_dict())
        .and_then(|form| form.get(b"SigFlags"))
        .and_then(|o| doc.dereference(o))
        .and_then(|(_, o)| o.as_i64())
        .is_ok_and(|flags| flags & 1 != 0)
}

/// An image job: the object, its largest placement (None = unknown) and role.
type ImageJob = (ObjectId, Option<(f32, f32)>, Role);

/// Placement and role per image object.
type JobMap = BTreeMap<ObjectId, (Option<(f32, f32)>, Role)>;

fn image_jobs(doc: &Document, usage: &usage::Usage) -> Vec<ImageJob> {
    let mut jobs: JobMap = BTreeMap::new();
    let merge = |jobs: &mut JobMap, id: ObjectId, placed: Option<(f32, f32)>, role: Role| {
        let entry = jobs.entry(id).or_insert((placed, role));
        entry.0 = match (entry.0, placed) {
            (Some(a), Some(b)) => Some((a.0.max(b.0), a.1.max(b.1))),
            _ => None,
        };
        if role == Role::SoftMask {
            entry.1 = Role::SoftMask;
        }
    };
    for (&id, &size) in &usage.placed {
        let placed = if usage.incomplete || usage.unknown.contains(&id) {
            None
        } else {
            Some(size)
        };
        merge(&mut jobs, id, placed, Role::Image);
        // A soft mask covers the same area as its image.
        if let Ok(smask) = doc
            .get_object(id)
            .and_then(Object::as_stream)
            .and_then(|s| s.dict.get(b"SMask"))
            .and_then(Object::as_reference)
        {
            merge(&mut jobs, smask, placed, Role::SoftMask);
        }
    }
    // Unknown-size images that were never drawn directly still get a lossless/JPEG
    // pass without downscaling.
    for &id in &usage.unknown {
        if !jobs.contains_key(&id) {
            merge(&mut jobs, id, None, Role::Image);
        }
    }
    jobs.into_iter().map(|(id, (p, r))| (id, p, r)).collect()
}

fn pixels_of(doc: &Document, id: ObjectId) -> u64 {
    let dim = |k: &[u8]| {
        doc.get_object(id)
            .and_then(Object::as_stream)
            .and_then(|s| s.dict.get(k))
            .and_then(Object::as_i64)
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    };
    dim(b"Width").saturating_mul(dim(b"Height"))
}

/// Re-encode the images. Progress goes from 0.05 to 0.8.
fn rewrite_images(
    doc: &mut Document,
    target: Target,
    ctx: &TaskCtx<'_>,
) -> Result<Stats, ConvertError> {
    let usage = usage::collect(doc, || ctx.cancel.is_cancelled())?;
    ctx.check_cancel()?;
    let jobs = image_jobs(doc, &usage);
    let total = jobs.len().max(1);
    let mut stats = Stats::default();
    let mut done = 0usize;
    let mut replacements: Vec<(ObjectId, Stream)> = Vec::new();

    let mut rest = jobs.as_slice();
    while !rest.is_empty() {
        // Take images until the batch's pixels reach the budget (at least one).
        let mut pixels = 0u64;
        let mut n = 0;
        for job in rest {
            let p = pixels_of(doc, job.0);
            if n > 0 && pixels + p > BATCH_PIXELS {
                break;
            }
            pixels += p;
            n += 1;
        }
        let (batch, tail) = rest.split_at(n);
        rest = tail;

        let shared: &Document = doc;
        let outcomes: Vec<(ObjectId, Outcome)> = batch
            .par_iter()
            .map(|&(id, placed, role)| {
                if ctx.cancel.is_cancelled() {
                    return (id, Outcome::Skipped("cancelled"));
                }
                let outcome = match shared.get_object(id).and_then(Object::as_stream) {
                    Ok(stream)
                        if stream.dict.get(b"Subtype").and_then(Object::as_name).ok()
                            == Some(b"Image") =>
                    {
                        images::recompress(shared, stream, placed, role, target)
                    }
                    _ => Outcome::Skipped("not an image"),
                };
                (id, outcome)
            })
            .collect();
        ctx.check_cancel()?;
        for (id, outcome) in outcomes {
            match outcome {
                Outcome::Replaced(s) => {
                    stats.replaced += 1;
                    replacements.push((id, s));
                }
                Outcome::Unchanged => stats.unchanged += 1,
                Outcome::Skipped(why) => stats.skipped.push(why),
            }
        }
        done += n;
        ctx.progress(0.05 + 0.75 * done as f32 / total as f32, None);
    }
    for (id, s) in replacements {
        doc.objects.insert(id, Object::Stream(s));
    }
    Ok(stats)
}

/// Deflate an uncompressed stream, or re-deflate a plain Flate stream at the best
/// level when that is smaller. Other filters are left alone.
fn recompress_stream(s: &mut Stream) {
    if !s.allows_compression
        || s.dict.get(b"Type").and_then(Object::as_name).ok() == Some(b"Metadata")
    {
        return;
    }
    if !s.dict.has(b"Filter") {
        if s.dict.has(b"DecodeParms") {
            return;
        }
        let _ = s.compress();
        return;
    }
    let single_flate = s
        .filters()
        .is_ok_and(|f| f.len() == 1 && f[0] == b"FlateDecode");
    if !single_flate || s.dict.has(b"DecodeParms") {
        return;
    }
    let Ok(plain) = s.decompressed_content_with_limit(REDEFLATE_MAX_BYTES) else {
        return;
    };
    let mut fresh = Stream::new(lopdf::Dictionary::new(), plain);
    if fresh.compress().is_ok()
        && fresh.dict.has(b"Filter")
        && fresh.content.len() < s.content.len()
    {
        s.set_content(fresh.content);
    }
}

fn optimize_streams(doc: &mut Document, ctx: &TaskCtx<'_>) -> Result<(), ConvertError> {
    let streams: Vec<&mut Stream> = doc
        .objects
        .values_mut()
        .filter_map(|o| match o {
            Object::Stream(s) => Some(s),
            _ => None,
        })
        .collect();
    let one = |s: &mut Stream| {
        if !ctx.cancel.is_cancelled() {
            recompress_stream(s);
        }
    };
    // A small pool of its own bounds the bytes in flight (see REDEFLATE_THREADS);
    // without one, go one stream at a time.
    match rayon::ThreadPoolBuilder::new()
        .num_threads(REDEFLATE_THREADS)
        .build()
    {
        Ok(pool) => pool.install(|| streams.into_par_iter().for_each(one)),
        Err(_) => streams.into_iter().for_each(one),
    }
    ctx.check_cancel()
}

/// The whole pass on a loaded document, in memory.
fn optimize(doc: &mut Document, target: Target, ctx: &TaskCtx<'_>) -> Result<Stats, ConvertError> {
    let stats = rewrite_images(doc, target, ctx)?;
    optimize_streams(doc, ctx)?;
    ctx.progress(0.9, None);
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.prune_objects();
    doc.renumber_objects();
    Ok(stats)
}

fn save(doc: &mut Document) -> Result<Vec<u8>, ConvertError> {
    let options = SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .compression_level(9)
        .build();
    let mut out = Vec::new();
    doc.save_with_options(&mut out, options).map_err(|e| {
        ConvertError::other("Couldn't compress this PDF.", format!("lopdf save: {e}"))
    })?;
    Ok(out)
}

/// The written result must open in PDFium with the input's page count.
fn verify(path: &Path, pages: usize) -> Result<(), ConvertError> {
    let doc = pdfium::open(path).map_err(|e| {
        ConvertError::other(
            "Couldn't compress this PDF safely; the original was kept as it is.",
            format!("result doesn't open: {}", e.details),
        )
    })?;
    let got = pdfium::page_count(&doc) as usize;
    if got != pages {
        return Err(ConvertError::other(
            "Couldn't compress this PDF safely; the original was kept as it is.",
            format!("result has {got} pages instead of {pages}"),
        ));
    }
    Ok(())
}

impl Converter for PdfCompress {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfCompress(opts) = &task.action else {
            return Err(super::wrong_action("PdfCompress"));
        };
        let input = task
            .input()
            .ok_or_else(|| ConvertError::other("No file to convert.", "task has no input"))?;
        let (max_dpi, quality) = opts.target();
        let target = Target {
            max_dpi: max_dpi.max(1) as f32,
            quality,
        };
        ctx.check_cancel()?;
        let input_len = std::fs::metadata(input)?.len();
        let mut doc = load(input)?;
        let leave_alone = if is_signed(&doc) {
            Some(SIGNED_NOTE)
        } else if has_restrictions(&doc) {
            Some(RESTRICTED_NOTE)
        } else {
            None
        };
        if let Some(note) = leave_alone {
            ctx.progress(1.0, None);
            return Ok(TaskOutput {
                outputs: Vec::new(),
                note: Some(note.to_owned()),
            });
        }
        let pages = doc.get_pages().len();
        ctx.progress(0.05, None);

        optimize(&mut doc, target, ctx)?;
        let bytes = save(&mut doc)?;
        drop(doc);
        ctx.check_cancel()?;

        if bytes.len() as f64 >= input_len as f64 * MIN_GAIN {
            ctx.progress(1.0, None);
            return Ok(TaskOutput {
                outputs: Vec::new(),
                note: Some(ALREADY_OPTIMIZED_NOTE.to_owned()),
            });
        }
        let folder = task.output.folder_for(input);
        let mut out = OutputFile::create(
            &folder,
            &naming::output_name(input, "pdf", Suffix::Compressed),
        )?;
        out.write_all(&bytes)?;
        out.close_handle();
        verify(out.temp_path(), pages)?;
        ctx.check_cancel()?;
        let path = out.commit()?;
        ctx.progress(1.0, None);
        Ok(TaskOutput::single(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancelToken;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn images_fixture_rewrites_every_photo() {
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let mut doc = load(&fixture("images.pdf")).unwrap();
        let stats = optimize(
            &mut doc,
            Target {
                max_dpi: 150.0,
                quality: 75,
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(
            stats,
            Stats {
                replaced: 3,
                unchanged: 0,
                skipped: vec![]
            }
        );
    }

    #[test]
    fn text_fixture_has_no_images() {
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let mut doc = load(&fixture("text-3pages.pdf")).unwrap();
        let stats = optimize(
            &mut doc,
            Target {
                max_dpi: 150.0,
                quality: 75,
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(stats, Stats::default());
    }

    #[test]
    fn password_and_broken_files() {
        let err = load(&fixture("password.pdf")).unwrap_err();
        assert_eq!(err.message, "This PDF is password-protected.", "{err}");
        let err = load(&fixture("broken.pdf")).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::BadInput, "{err}");
    }
}
