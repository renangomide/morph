// SPDX-License-Identifier: GPL-3.0-or-later
//! PDFium access: loaded once per process, from a fixed list of full paths.
//!
//! Thread safety: PDFium itself is not thread safe. pdfium-render's `thread_safe`
//! feature (enabled in Cargo.toml) puts every PDFium call behind one process-wide
//! mutex, so any thread may use [`pdfium()`] and the documents it opens. Calls are
//! serialized, so there is no speed-up from rendering on several threads: keep
//! PDFium work for one document on one thread and parallelize what comes after
//! (image encoding, file writing). No call here takes a lock of our own across a
//! user callback, so using rayon inside a task is safe.
//!
//! Where `pdfium.dll` is looked for, in order:
//! 1. next to the running exe (installed package, `layout\`);
//! 2. debug builds only: `vendor\pdfium\pdfium.dll` in the nearest ancestor folder
//!    of the exe that has a `Cargo.toml` (dev builds and `cargo test`; run
//!    `scripts\fetch-deps.ps1`). Release builds need the DLL next to the exe.
//!
//! Never by bare name (that would search the current directory and PATH).

use crate::error::{ConvertError, ErrorKind};
use crate::naming::OutputFile;
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const DLL_NAME: &str = "pdfium.dll";

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Serializes document loading: pdfium-render reads PDFium's thread-global "last
/// error" in a second call after a failed load, which another thread's load could
/// overwrite in between.
static LOAD_LOCK: Mutex<()> = Mutex::new(());

/// Rendered pages larger than this fail instead of exhausting memory (1 GB of RGBA).
pub const MAX_RENDER_PIXELS: u64 = 250_000_000;

/// Where `pdfium.dll` is looked for, in order (see the module docs).
pub fn library_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return out;
    };
    out.push(exe_dir.join(DLL_NAME));
    // Dev fallback only: a release build never loads a DLL from outside its folder.
    #[cfg(debug_assertions)]
    if let Some(repo) = exe_dir
        .ancestors()
        .find(|dir| dir.join("Cargo.toml").is_file())
    {
        out.push(repo.join("vendor").join("pdfium").join(DLL_NAME));
    }
    out
}

fn load() -> Result<Pdfium, String> {
    let candidates = library_candidates();
    let mut tried = Vec::new();
    for path in &candidates {
        if !path.is_file() {
            tried.push(format!("{}: not found", path.display()));
            continue;
        }
        match Pdfium::bind_to_library(path) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(e) => tried.push(format!("{}: {e}", path.display())),
        }
    }
    Err(format!(
        "pdfium.dll could not be loaded. Tried: {}. Developers: run scripts\\fetch-deps.ps1.",
        if tried.is_empty() {
            "no location (exe folder unknown)".to_owned()
        } else {
            tried.join("; ")
        }
    ))
}

/// The process-wide PDFium instance, loaded on first use.
pub fn pdfium() -> Result<&'static Pdfium, ConvertError> {
    match PDFIUM.get_or_init(load) {
        Ok(p) => Ok(p),
        Err(details) => Err(missing_engine(details)),
    }
}

fn missing_engine(details: &str) -> ConvertError {
    ConvertError::other(
        "The PDF engine (pdfium.dll) is missing. Reinstall the app to fix this.",
        details,
    )
}

/// "This PDF is password-protected."
pub fn password_protected(details: impl ToString) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This PDF is password-protected.",
        details,
    )
}

/// "This PDF seems to be damaged."
pub fn damaged_pdf(details: impl ToString) -> ConvertError {
    ConvertError::new(
        ErrorKind::BadInput,
        "This PDF seems to be damaged or isn't a PDF.",
        details,
    )
}

/// Turn a PDFium error into a plain-English one. `what` says what was being
/// done, for the details (e.g. "render page 3").
pub fn map_error(e: PdfiumError, what: &str) -> ConvertError {
    let details = format!("{what}: {e:?}");
    match e {
        PdfiumError::PdfiumLibraryInternalError(
            PdfiumInternalError::PasswordError | PdfiumInternalError::SecurityError,
        ) => password_protected(details),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            damaged_pdf(details)
        }
        PdfiumError::IoError(io) => {
            let mut err = ConvertError::from(io);
            err.details = details;
            err
        }
        _ => ConvertError::other("The PDF engine failed on this file.", details),
    }
}

/// Open a PDF for reading. The file is read lazily and kept open (read-only)
/// while the document lives; it is never written. A password-protected file
/// gives [`password_protected`] (Morph never asks for passwords in v1).
pub fn open(path: &Path) -> Result<PdfDocument<'static>, ConvertError> {
    let pdfium = pdfium()?;
    let _guard = LOAD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| map_error(e, &format!("open {}", path.display())))
}

/// A new, empty document (for split parts and merge).
pub fn new_document() -> Result<PdfDocument<'static>, ConvertError> {
    let pdfium = pdfium()?;
    let _guard = LOAD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    pdfium
        .create_new_pdf()
        .map_err(|e| map_error(e, "create document"))
}

/// Number of pages.
pub fn page_count(doc: &PdfDocument<'_>) -> u32 {
    u32::try_from(doc.pages().len()).unwrap_or(0)
}

/// Load page `index` (0-based).
pub fn page<'a>(doc: &PdfDocument<'a>, index: u32) -> Result<PdfPage<'a>, ConvertError> {
    let i = PdfPageIndex::try_from(index)
        .map_err(|e| ConvertError::other("The PDF engine failed on this file.", e))?;
    doc.pages()
        .get(i)
        .map_err(|e| map_error(e, &format!("load page {}", index + 1)))
}

/// Pixel size of `page` rendered at `dpi` (page rotation included).
pub fn render_size(page: &PdfPage<'_>, dpi: u32) -> (u32, u32) {
    let scale = dpi.max(1) as f32 / 72.0;
    let px = |pt: f32| (pt * scale).round().max(1.0) as u32;
    (px(page.width().value), px(page.height().value))
}

/// Render `page` at `dpi` to RGBA on a white background, with annotations and
/// form fields. Fails cleanly when the result would exceed [`MAX_RENDER_PIXELS`].
pub fn render_rgba(page: &PdfPage<'_>, dpi: u32) -> Result<image::RgbaImage, ConvertError> {
    let (w, h) = render_size(page, dpi);
    if u64::from(w) * u64::from(h) > MAX_RENDER_PIXELS {
        return Err(ConvertError::unsupported(
            "This page is too large to render at this DPI. Try a lower DPI.",
            format!("{w}×{h} at {dpi} DPI"),
        ));
    }
    let to_px = |v: u32| Pixels::try_from(v).unwrap_or(Pixels::MAX);
    let config = PdfRenderConfig::new()
        .set_target_size(to_px(w), to_px(h))
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| map_error(e, "render page"))?;
    let (bw, bh) = (bitmap.width(), bitmap.height());
    let bytes = bitmap.as_rgba_bytes();
    let (bw, bh) = (
        u32::try_from(bw).unwrap_or(0),
        u32::try_from(bh).unwrap_or(0),
    );
    image::RgbaImage::from_raw(bw, bh, bytes).ok_or_else(|| {
        ConvertError::other(
            "The PDF engine failed on this file.",
            format!("bitmap {bw}×{bh}: unexpected buffer size"),
        )
    })
}

/// All text on `page`, in PDFium's reading order. Empty for scanned pages.
pub fn page_text(page: &PdfPage<'_>) -> Result<String, ConvertError> {
    page.text()
        .map(|t| t.all())
        .map_err(|e| map_error(e, "extract text"))
}

/// Serialize `doc` and save it safely as `folder/name` (temp file + rename,
/// ` (1)` on collisions). Returns the final path.
pub fn save(doc: &PdfDocument<'_>, folder: &Path, name: &str) -> Result<PathBuf, ConvertError> {
    let bytes = doc
        .save_to_bytes()
        .map_err(|e| map_error(e, "save document"))?;
    let mut out = OutputFile::create(folder, name)?;
    out.write_all(&bytes)?;
    out.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn loads_and_reads_text_pdf() {
        let doc = open(&fixture("text-3pages.pdf")).unwrap();
        assert_eq!(page_count(&doc), 3);
        let p = page(&doc, 1).unwrap();
        let text = page_text(&p).unwrap();
        assert!(text.contains("Page 2") && text.contains("café"), "{text}");
        let img = render_rgba(&p, 72).unwrap();
        assert_eq!((img.width(), img.height()), render_size(&p, 72));
        assert!(page(&doc, 3).is_err());
    }

    #[test]
    fn password_and_damage_are_plain_english() {
        let err = open(&fixture("password.pdf")).err().unwrap();
        assert_eq!(err.message, "This PDF is password-protected.", "{err}");
        let err = open(&fixture("broken.pdf")).err().unwrap();
        assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
    }

    #[test]
    fn every_fixture_opens() {
        for (name, pages) in [
            ("images.pdf", 3),
            ("scanned.pdf", 2),
            ("pages-100.pdf", 100),
        ] {
            assert_eq!(page_count(&open(&fixture(name)).unwrap()), pages, "{name}");
        }
    }

    #[test]
    fn scanned_pdf_has_no_text() {
        let doc = open(&fixture("scanned.pdf")).unwrap();
        let p = page(&doc, 0).unwrap();
        assert!(page_text(&p).unwrap().trim().is_empty());
    }

    #[test]
    fn unicode_name_opens() {
        let doc = open(&fixture("Relatório 報告.pdf")).unwrap();
        assert_eq!(page_count(&doc), 1);
    }

    #[test]
    fn save_round_trip_and_import_pages() {
        let src = open(&fixture("text-3pages.pdf")).unwrap();
        let mut dst = new_document().unwrap();
        dst.pages_mut()
            .copy_page_range_from_document(&src, 1..=2, 0)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = save(&dst, dir.path(), "part.pdf").unwrap();
        assert_eq!(page_count(&open(&out).unwrap()), 2);
    }
}
