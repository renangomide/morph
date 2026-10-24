// SPDX-License-Identifier: GPL-3.0-or-later
//! Document conversions through the Office engine.
//!
//! - [`convert`]: the `office.convert` action for [`super::Plan::Engine`]
//!   (Word/Excel/PowerPoint-like → PDF/DOCX/ODT/RTF/TXT/HTML/XLSX/ODS/PPTX/ODP).
//! - [`export`]: one engine conversion into the task's temp folder. Other modules
//!   use it too (text-like HTML → DOCX/PDF/ODT, Word → HTML for Markdown).
//!
//! Engines:
//! - LibreOffice ([`export_libreoffice`], filters in [`libreoffice_target`]):
//!   `soffice --convert-to` on a staged copy of the input.
//! - Microsoft Office ([`ms_office`]): Word `Documents.Open` +
//!   `ExportAsFixedFormat`/`SaveAs2`, Excel `Workbooks.Open` +
//!   `ExportAsFixedFormat`/`SaveAs`, PowerPoint `Presentations.Open` + `SaveAs`,
//!   all through [`super::com::run_office`].
//!
//! Before any engine runs, [`check_not_encrypted`] spots password-protected
//! OOXML (an OLE container with an `EncryptedPackage` stream) and ODF files
//! (`encryption-data` in the manifest), and old `.doc`/`.ppt` with a password
//! ([`super::ole`]), so no engine ever sits on a password
//! prompt. Engine output names (`input.pdf`, `output.pdf`) never reach the
//! user: [`convert`] publishes as `name.<ext>` / `name (converted).<ext>`.

pub mod ms_office;

use super::detect::{Engine, EngineChoice, EnginePreference, LibreOfficeInfo};
use super::options::{CsvDelimiter, DocFormat, Family, OfficeConvertOpts};
use super::soffice::{self, SofficeJob};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::{self, Suffix};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `office.convert` through the engine: export, then publish next to the input
/// (or in the chosen folder) as `name.<ext>` / `name (converted).<ext>`.
pub fn convert(
    task: &Task,
    input: &Path,
    family: Family,
    opts: &OfficeConvertOpts,
    engine: &EngineChoice,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    ctx.progress(0.05, None);
    // The published HTML is one self-contained file (pictures as data: URIs).
    let exported = export_with(
        input,
        family,
        opts.format,
        engine,
        true,
        opts.delimiter,
        ctx,
    )?;
    ctx.check_cancel()?;
    ctx.progress(0.95, None);
    let folder = task.output.folder_for(input);
    let name = naming::output_name(input, opts.format.extension(), Suffix::Converted);
    let path = naming::publish_file(&exported, &folder, &name)?;
    ctx.progress(1.0, None);
    let out = TaskOutput::single(path);
    Ok(match fallback_note(ctx.office_engine(), engine) {
        Some(note) => out.with_note(note),
        None => out,
    })
}

/// Convert `input` (of `family`) to `format` with `engine`. Returns the result
/// file inside the task's temp folder; the caller publishes it with
/// [`naming::publish_file`]. Never writes next to the input.
///
/// HTML: LibreOffice writes pictures as separate files next to the HTML (in
/// the temp folder), Word into a `name_files` folder; [`convert`] embeds them.
pub fn export(
    input: &Path,
    family: Family,
    format: DocFormat,
    engine: &EngineChoice,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    export_with(
        input,
        family,
        format,
        engine,
        false,
        CsvDelimiter::Auto,
        ctx,
    )
}

fn export_with(
    input: &Path,
    family: Family,
    format: DocFormat,
    engine: &EngineChoice,
    embed_images: bool,
    csv_delimiter: CsvDelimiter,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    if !family.can_export(format) {
        return Err(cant_convert(family, format));
    }
    check_not_encrypted(input)?;
    ctx.check_cancel()?;
    // A CSV goes to the engine as Morph's own XLSX (`csv_as_temp_xlsx`): no
    // formulas evaluated, no retyped cells, any encoding.
    let staged;
    let input = if DocFormat::of_path(input) == Some(DocFormat::Csv) {
        staged = super::csv_xlsx::csv_as_temp_xlsx(input, csv_delimiter, ctx)?;
        staged.as_path()
    } else {
        input
    };
    match engine {
        EngineChoice::LibreOffice(lo) => {
            export_libreoffice_impl(lo, input, family, format, embed_images, None, ctx)
        }
        EngineChoice::MsOffice(app) => {
            let out = ms_office::export(app, input, family, format, ctx)?;
            if embed_images && format == DocFormat::Html {
                inline_html_images(&out)?;
            }
            Ok(out)
        }
    }
}

/// "Microsoft Office isn't installed, so LibreOffice was used." when Settings
/// forces an engine that this machine doesn't have (design decision: fall back
/// to the other engine and say so).
pub fn fallback_note(preference: EnginePreference, used: &EngineChoice) -> Option<&'static str> {
    match (preference, used.engine()) {
        (EnginePreference::MicrosoftOffice, Engine::LibreOffice) => {
            Some("Microsoft Office isn't installed, so LibreOffice was used.")
        }
        (EnginePreference::LibreOffice, Engine::MsOffice) => {
            Some("LibreOffice isn't installed, so Microsoft Office was used.")
        }
        _ => None,
    }
}

fn cant_convert(family: Family, format: DocFormat) -> ConvertError {
    ConvertError::unsupported(
        format!(
            "A {} can't be converted to {}.",
            family.noun(),
            format.label()
        ),
        format!("{family:?} → {format:?}"),
    )
}

// ---------------------------------------------------------------- LibreOffice

/// `--convert-to` value for LibreOffice. `None`: the family can't produce it.
///
/// - PDF: the module's own export filter (whole document / every sheet with
///   the document's print setup / every slide).
/// - TXT: UTF-8 (with BOM, CRLF on Windows), so Notepad and Excel read it right.
/// - HTML: Writer's HTML export (UTF-8). [`convert`] adds `:EmbedImages`.
pub fn libreoffice_target(family: Family, format: DocFormat) -> Option<&'static str> {
    use DocFormat as F;
    let writer = matches!(family, Family::Word | Family::Text);
    Some(match format {
        F::Pdf => match family {
            Family::Word | Family::Text => "pdf:writer_pdf_Export",
            Family::Excel => "pdf:calc_pdf_Export",
            Family::PowerPoint => "pdf:impress_pdf_Export",
        },
        F::Docx if writer => "docx:MS Word 2007 XML",
        F::Odt if writer => "odt:writer8",
        F::Rtf if writer => "rtf:Rich Text Format",
        F::Txt if writer => "txt:Text (encoded):UTF8",
        F::Html if writer => "html:HTML (StarWriter)",
        F::Xlsx if family == Family::Excel => "xlsx:Calc MS Excel 2007 XML",
        F::Ods if family == Family::Excel => "ods:calc8",
        F::Csv if family == Family::Excel => "csv:Text - txt - csv (StarCalc):44,34,76",
        F::Pptx if family == Family::PowerPoint => "pptx:Impress MS PowerPoint 2007 XML",
        F::Odp if family == Family::PowerPoint => "odp:impress8",
        _ => return None,
    })
}

/// HTML export with every picture embedded as a `data:` URI (one file).
const LIBREOFFICE_HTML_EMBEDDED: &str = "html:HTML (StarWriter):EmbedImages";

/// `--infilter` for inputs LibreOffice would otherwise open in the wrong module:
/// HTML opens in Writer/Web (which can't export DOCX/PDF/ODT). CSV inputs never
/// reach LibreOffice (they are staged as XLSX, see [`export`]).
pub fn libreoffice_infilter(input: &Path, family: Family) -> Option<String> {
    match DocFormat::of_path(input) {
        Some(DocFormat::Html) if matches!(family, Family::Text | Family::Word) => {
            Some("HTML (StarWriter)".to_owned())
        }
        _ => None,
    }
}

/// LibreOffice conversion into the temp folder; returns the main output file
/// (the one with the target extension; HTML exports may also write images).
pub fn export_libreoffice(
    lo: &LibreOfficeInfo,
    input: &Path,
    family: Family,
    format: DocFormat,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    export_libreoffice_impl(lo, input, family, format, false, None, ctx)
}

fn export_libreoffice_impl(
    lo: &LibreOfficeInfo,
    input: &Path,
    family: Family,
    format: DocFormat,
    embed_images: bool,
    timeout: Option<Duration>,
    ctx: &TaskCtx<'_>,
) -> Result<PathBuf, ConvertError> {
    let target = libreoffice_target(family, format).ok_or_else(|| cant_convert(family, format))?;
    let target = if embed_images && format == DocFormat::Html {
        LIBREOFFICE_HTML_EMBEDDED
    } else {
        target
    };
    let infilter = libreoffice_infilter(input, family);
    let mut job = SofficeJob::new(input, target);
    job.infilter = infilter.as_deref();
    if let Some(t) = timeout {
        job.timeout = t;
    }
    let out = soffice::convert(lo, &job, ctx)?;
    let ext = format.extension();
    out.files
        .iter()
        .find(|f| {
            f.extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
        })
        .cloned()
        .ok_or_else(|| soffice::convert_failed(&out.log))
}

// ---------------------------------------------------------------- password check

use super::ole::OLE_MAGIC;
/// Files bigger than this aren't scanned (the check is a shortcut, not a gate).
const SCAN_LIMIT: u64 = 256 * 1024 * 1024;

/// Password-protected OOXML (Office "Encrypt with password" stores the package
/// in an OLE file with an `EncryptedPackage` stream) or ODF (manifest with
/// `encryption-data`) → [`super::password_protected`]. Anything unreadable
/// passes: the engine reports it.
pub fn check_not_encrypted(input: &Path) -> Result<(), ConvertError> {
    if is_encrypted(input) {
        let mut e = super::password_protected();
        e.details = format!("{}: encrypted document", input.display());
        return Err(e);
    }
    Ok(())
}

fn is_encrypted(input: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(input) else {
        return false;
    };
    if meta.len() > SCAN_LIMIT {
        return false;
    }
    let Ok(bytes) = std::fs::read(input) else {
        return false;
    };
    if bytes.starts_with(&OLE_MAGIC) {
        // Old .doc / .ppt with a password (an .xls FILEPASS may be Excel's
        // built-in default password, so that one is left to the engine).
        if super::ole::legacy_encryption_of(&bytes) == Some(super::ole::Encryption::Yes) {
            return true;
        }
        let name: Vec<u8> = "EncryptedPackage"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        return bytes.windows(name.len()).any(|w| w == name.as_slice());
    }
    if bytes.starts_with(b"PK") {
        let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(&bytes)) else {
            return false;
        };
        let Ok(mut manifest) = zip.by_name("META-INF/manifest.xml") else {
            return false;
        };
        let mut text = String::new();
        if manifest.read_to_string(&mut text).is_ok() {
            return text.contains("encryption-data");
        }
    }
    false
}

// ---------------------------------------------------------------- HTML pictures

/// Replace `src="relative/picture.png"` in an exported HTML file with `data:`
/// URIs, so the published HTML is one file (Word's filtered HTML writes its
/// pictures into `name_files\`). Only files inside the HTML's own folder
/// (the task's temp folder) are embedded.
fn inline_html_images(html: &Path) -> Result<(), ConvertError> {
    let bytes = std::fs::read(html).map_err(ConvertError::from)?;
    let Some(base) = html.parent() else {
        return Ok(());
    };
    let rewritten = embed_srcs(&bytes, base);
    if rewritten != bytes {
        std::fs::write(html, rewritten).map_err(ConvertError::from)?;
    }
    Ok(())
}

fn embed_srcs(html: &[u8], base: &Path) -> Vec<u8> {
    let mut out = Vec::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = find_ci(rest, b"src=\"") {
        let (head, tail) = rest.split_at(i + 5);
        out.extend_from_slice(head);
        let Some(end) = tail.iter().position(|&b| b == b'"') else {
            rest = tail;
            break;
        };
        let value = &tail[..end];
        match picture_data_uri(value, base) {
            Some(uri) => out.extend_from_slice(uri.as_bytes()),
            None => out.extend_from_slice(value),
        }
        rest = &tail[end..];
    }
    out.extend_from_slice(rest);
    out
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

fn picture_data_uri(src: &[u8], base: &Path) -> Option<String> {
    let src = percent_decode(&String::from_utf8_lossy(src));
    if src.contains(':') || src.starts_with('/') || src.starts_with('\\') || src.contains("..") {
        return None; // absolute, a URL scheme, or outside the temp folder
    }
    let mime = match Path::new(&src)
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    let data = std::fs::read(base.join(&src)).ok()?;
    Some(format!("data:{mime};base64,{}", base64(&data)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancelToken;
    use crate::office::detect;
    use std::io::Write;

    #[test]
    fn every_family_output_has_a_libreoffice_filter() {
        for family in [
            Family::Word,
            Family::Excel,
            Family::PowerPoint,
            Family::Text,
        ] {
            for &format in family.outputs() {
                // Markdown is HTML + htmd; text-like natives never reach the engine.
                let native = format == DocFormat::Md
                    || (family == Family::Text
                        && !matches!(format, DocFormat::Docx | DocFormat::Pdf | DocFormat::Odt));
                if !native {
                    assert!(
                        libreoffice_target(family, format).is_some(),
                        "{family:?} → {format:?}"
                    );
                }
            }
        }
        assert!(libreoffice_target(Family::Excel, DocFormat::Docx).is_none());
        assert_eq!(
            libreoffice_infilter(Path::new("a.HTM"), Family::Text).as_deref(),
            Some("HTML (StarWriter)")
        );
        assert_eq!(
            libreoffice_infilter(Path::new("a.docx"), Family::Word),
            None
        );
    }

    #[test]
    fn infilters() {
        let html = Path::new("a.html");
        assert_eq!(
            libreoffice_infilter(html, Family::Text).as_deref(),
            Some("HTML (StarWriter)")
        );
        // CSV is staged as XLSX, never read by LibreOffice's CSV filter.
        assert_eq!(
            libreoffice_infilter(Path::new("a.csv"), Family::Excel),
            None
        );
    }

    #[test]
    fn encrypted_documents_are_spotted() {
        let dir = tempfile::tempdir().unwrap();
        // OOXML with a password: an OLE file with an EncryptedPackage stream.
        let ole = dir.path().join("locked.docx");
        let mut bytes = OLE_MAGIC.to_vec();
        bytes.resize(512, 0);
        bytes.extend("EncryptedPackage".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&ole, &bytes).unwrap();
        let e = check_not_encrypted(&ole).unwrap_err();
        assert_eq!(e.message, "This file is password-protected.");
        // ODF with a password: encryption-data in the manifest.
        let odt = dir.path().join("locked.odt");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&odt).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("mimetype", o).unwrap();
        z.write_all(b"application/vnd.oasis.opendocument.text")
            .unwrap();
        z.start_file("META-INF/manifest.xml", o).unwrap();
        z.write_all(b"<manifest:file-entry><manifest:encryption-data/></manifest:file-entry>")
            .unwrap();
        z.finish().unwrap();
        assert!(check_not_encrypted(&odt).is_err());
        // Ordinary fixtures pass.
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        for name in ["sample.docx", "sample.odt", "sample.pptx", "broken.docx"] {
            assert!(check_not_encrypted(&fixtures.join(name)).is_ok(), "{name}");
        }
        // Old binary files: a .doc with a password is refused up front; an
        // encrypted .xls may use Excel's default password, so the engine decides.
        let e = check_not_encrypted(&fixtures.join("password.doc")).unwrap_err();
        assert_eq!(e.message, "This file is password-protected.");
        for name in ["sample.doc", "sample.xls", "password.xls"] {
            assert!(check_not_encrypted(&fixtures.join(name)).is_ok(), "{name}");
        }
    }

    #[test]
    fn html_pictures_are_embedded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("out_files")).unwrap();
        std::fs::write(dir.path().join("out_files").join("image 1.png"), b"abc").unwrap();
        let html = dir.path().join("out.html");
        std::fs::write(
            &html,
            "<img src=\"out_files/image%201.png\"><img SRC=\"http://x/y.png\"><img src=\"../z.png\">",
        )
        .unwrap();
        inline_html_images(&html).unwrap();
        let text = std::fs::read_to_string(&html).unwrap();
        assert!(
            text.contains("src=\"data:image/png;base64,YWJj\""),
            "{text}"
        );
        assert!(text.contains("SRC=\"http://x/y.png\""));
        assert!(text.contains("src=\"../z.png\""));
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn forced_engine_fallback_is_noted() {
        let lo = EngineChoice::LibreOffice(LibreOfficeInfo {
            soffice: PathBuf::from("soffice.com"),
            version: None,
        });
        assert!(fallback_note(EnginePreference::MicrosoftOffice, &lo).is_some());
        assert!(fallback_note(EnginePreference::Auto, &lo).is_none());
        assert!(fallback_note(EnginePreference::LibreOffice, &lo).is_none());
    }

    /// Timeout through the engine path: the engine is stopped, plain message.
    #[test]
    fn libreoffice_timeout_is_plain() {
        let Some(lo) = detect::engines().libreoffice.clone() else {
            println!("SKIPPED: LibreOffice not installed");
            return;
        };
        let cancel = CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.docx");
        let e = export_libreoffice_impl(
            &lo,
            &input,
            Family::Word,
            DocFormat::Pdf,
            false,
            Some(Duration::from_millis(300)),
            &ctx,
        )
        .unwrap_err();
        assert_eq!(e.message, "LibreOffice took too long and was stopped.");
        // From LibreOffice's start, kill included; not the wait for a free profile slot.
        let ran: f64 = e
            .details
            .split_once("stopped after ")
            .and_then(|(_, rest)| rest.split_once('s'))
            .and_then(|(secs, _)| secs.parse().ok())
            .unwrap_or_else(|| panic!("no run time in details: {e}"));
        assert!(ran < 10.0, "killed promptly: {}", e.details);
    }
}
