// SPDX-License-Identifier: GPL-3.0-or-later
//! PDF Compress (SPEC §4.2, §12 milestone 4): acceptance tests.

use image::{DynamicImage, RgbImage};
use lopdf::{Document, Object, Stream, dictionary};
use morph_core::error::ErrorKind;
use morph_core::pdf::compress::PdfCompress;
use morph_core::pdf::options::{CompressPreset, PdfCompressOpts};
use morph_core::pdf::pdfium;
use morph_core::{
    Action, CancelToken, ConvertError, Converter, OutputSpec, Task, TaskCtx, TaskId, TaskOutput,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn opts(preset: CompressPreset) -> PdfCompressOpts {
    PdfCompressOpts {
        preset,
        ..Default::default()
    }
}

/// Run Compress on `input`, writing into `out_dir`.
fn run_with(
    input: &Path,
    out_dir: &Path,
    opts: PdfCompressOpts,
    cancel: &CancelToken,
) -> Result<TaskOutput, ConvertError> {
    let task = Task {
        id: TaskId(0),
        inputs: vec![input.to_path_buf()],
        action: Action::PdfCompress(opts),
        output: OutputSpec {
            folder: Some(out_dir.to_path_buf()),
        },
    };
    let scratch = tempfile::tempdir().unwrap();
    let progress = |_: f32, _: Option<std::time::Duration>| {};
    let ctx = TaskCtx::new(cancel, &progress, scratch.path());
    PdfCompress.run(&task, &ctx)
}

fn run(input: &Path, out_dir: &Path, opts: PdfCompressOpts) -> Result<TaskOutput, ConvertError> {
    run_with(input, out_dir, opts, &CancelToken::new())
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

/// Opens in PDFium with `pages` pages, and every page renders.
fn assert_valid(path: &Path, pages: u32) {
    let doc = pdfium::open(path).unwrap();
    assert_eq!(pdfium::page_count(&doc), pages, "{}", path.display());
    for i in 0..pages {
        let page = pdfium::page(&doc, i).unwrap();
        let img = pdfium::render_rgba(&page, 36).unwrap();
        assert!(img.width() > 0 && img.height() > 0);
    }
}

/// Mean RGB of page `index` rendered at 36 DPI.
fn mean_color(path: &Path, index: u32) -> [f64; 3] {
    let doc = pdfium::open(path).unwrap();
    let page = pdfium::page(&doc, index).unwrap();
    let img = pdfium::render_rgba(&page, 36).unwrap();
    let mut sum = [0f64; 3];
    for p in img.pixels() {
        for (s, v) in sum.iter_mut().zip(p.0) {
            *s += f64::from(v);
        }
    }
    let n = f64::from(img.width() * img.height());
    sum.map(|s| s / n)
}

#[test]
fn images_fixture_medium_is_40_percent_smaller_and_valid() {
    let input = fixture("images.pdf");
    let before = std::fs::read(&input).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let out = run(&input, dir.path(), opts(CompressPreset::Medium)).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(out.outputs.len(), 1, "{out:?}");
    let result = &out.outputs[0];
    assert_eq!(result.file_name().unwrap(), "images (compressed).pdf");
    let ratio = len(result) as f64 / before.len() as f64;
    println!(
        "images.pdf Medium: {} -> {} bytes ({:.1}% smaller) in {elapsed:?}",
        before.len(),
        len(result),
        (1.0 - ratio) * 100.0
    );
    assert!(ratio <= 0.6, "only {:.1}% smaller", (1.0 - ratio) * 100.0);
    assert_valid(result, 3);
    // Same picture: page colours barely move.
    for i in 0..3 {
        let (a, b) = (mean_color(&input, i), mean_color(result, i));
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 4.0, "page {i}: {a:?} vs {b:?}");
        }
    }
    assert_eq!(std::fs::read(&input).unwrap(), before, "input changed");
    // No temp files left behind.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn presets_get_stronger() {
    let input = fixture("images.pdf");
    let mut sizes = Vec::new();
    for preset in [
        CompressPreset::Low,
        CompressPreset::Medium,
        CompressPreset::High,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run(&input, dir.path(), opts(preset)).unwrap();
        let size = len(&out.outputs[0]);
        println!(
            "images.pdf {preset:?}: {} -> {size} bytes ({:.1}% smaller) in {:?}",
            len(&input),
            (1.0 - size as f64 / len(&input) as f64) * 100.0,
            start.elapsed()
        );
        assert_valid(&out.outputs[0], 3);
        sizes.push(size);
    }
    assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "{sizes:?}");

    // Custom: 72 DPI, q40 beats High.
    let dir = tempfile::tempdir().unwrap();
    let custom = PdfCompressOpts {
        preset: CompressPreset::Custom,
        dpi: 72,
        quality: 40,
    };
    let out = run(&input, dir.path(), custom).unwrap();
    assert!(len(&out.outputs[0]) < sizes[2]);
}

#[test]
fn text_only_is_already_optimized_or_valid() {
    for name in ["text-3pages.pdf", "Relatório 報告.pdf", "pages-100.pdf"] {
        let input = fixture(name);
        let before = std::fs::read(&input).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = run(&input, dir.path(), opts(CompressPreset::Medium)).unwrap();
        let pages = pdfium::page_count(&pdfium::open(&input).unwrap());
        match out.outputs.as_slice() {
            [] => {
                assert!(
                    out.note
                        .as_deref()
                        .unwrap()
                        .starts_with("Already optimized"),
                    "{out:?}"
                );
                assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
            }
            [result] => {
                assert!(len(result) < before.len() as u64);
                assert_valid(result, pages);
                let doc = pdfium::open(result).unwrap();
                let text = pdfium::page_text(&pdfium::page(&doc, 0).unwrap()).unwrap();
                assert!(!text.trim().is_empty(), "{name}: text lost");
            }
            more => panic!("{name}: unexpected outputs {more:?}"),
        }
        println!("{name}: {} bytes -> {:?}", before.len(), out);
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }
}

#[test]
fn scanned_gray_jpeg_stays_valid() {
    let input = fixture("scanned.pdf");
    let dir = tempfile::tempdir().unwrap();
    let out = run(&input, dir.path(), opts(CompressPreset::Medium)).unwrap();
    println!("scanned.pdf: {} bytes -> {out:?}", len(&input));
    for result in &out.outputs {
        assert_valid(result, 2);
    }
}

#[test]
fn password_and_broken_fail_in_plain_english() {
    let dir = tempfile::tempdir().unwrap();
    let err = run(
        &fixture("password.pdf"),
        dir.path(),
        opts(CompressPreset::Medium),
    )
    .unwrap_err();
    assert_eq!(err.message, "This PDF is password-protected.", "{err}");
    let err = run(
        &fixture("broken.pdf"),
        dir.path(),
        opts(CompressPreset::Medium),
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput, "{err}");
    assert!(err.message.contains("damaged"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn cancel_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = run_with(
        &fixture("images.pdf"),
        dir.path(),
        opts(CompressPreset::Medium),
        &cancel,
    )
    .unwrap_err();
    assert!(err.is_cancelled(), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

// ---------------------------------------------------------------- synthetic PDFs

/// Smooth photo-like RGB.
fn photo(w: u32, h: u32) -> RgbImage {
    let mut seed = 7u32;
    RgbImage::from_fn(w, h, |x, y| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let n = ((seed >> 16) % 9) as i32 - 4;
        let r = ((x * 255 / w) as i32 + n).clamp(0, 255) as u8;
        let g = ((y * 255 / h) as i32 + n).clamp(0, 255) as u8;
        let b = (((x + y) * 128 / (w + h)) as i32 + 60 + n).clamp(0, 255) as u8;
        image::Rgb([r, g, b])
    })
}

fn flate(dict: lopdf::Dictionary, raw: Vec<u8>) -> Stream {
    let mut s = Stream::new(dict, raw);
    s.compress().unwrap();
    s
}

/// One page (US Letter) drawing each `(name, id, rect)`.
fn one_page_pdf(doc: &mut Document, draws: &[(&str, lopdf::ObjectId, [f32; 4])]) {
    let pages_id = doc.new_object_id();
    let mut xobjects = lopdf::Dictionary::new();
    let mut ops = String::new();
    for (name, id, [x, y, w, h]) in draws {
        xobjects.set(name.as_bytes(), *id);
        ops.push_str(&format!("q {w} 0 0 {h} {x} {y} cm /{name} Do Q\n"));
    }
    let content = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => dictionary! { "XObject" => xobjects },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
        ),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
}

/// A Flate RGB photo with a soft mask, an indexed image, a CMYK image and a flat
/// graphic, all at ~600 DPI.
#[test]
fn flate_images_soft_masks_and_unsupported_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("mixed.pdf");
    let (w, h) = (1200u32, 900u32);
    let mut doc = Document::with_version("1.5");

    let alpha: Vec<u8> = (0..w * h)
        .map(|i| if (i % w) < w / 2 { 255 } else { 128 })
        .collect();
    let smask = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8,
        },
        alpha,
    ));
    let photo_id = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "SMask" => smask,
        },
        photo(w, h).into_raw(),
    ));
    let palette = Object::String(
        (0..=255u8).flat_map(|v| [v, 255 - v, v / 2]).collect(),
        lopdf::StringFormat::Hexadecimal,
    );
    let indexed_raw: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();
    let indexed = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => vec![Object::Name(b"Indexed".to_vec()), "DeviceRGB".into(), 255.into(), palette],
            "BitsPerComponent" => 8,
        },
        indexed_raw,
    ));
    let cmyk = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 400, "Height" => 300,
            "ColorSpace" => "DeviceCMYK", "BitsPerComponent" => 8,
        },
        (0..400 * 300 * 4).map(|i| (i % 251) as u8).collect(),
    ));
    let graphic = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb(if (x / 100 + y / 100) % 2 == 0 {
            [20, 40, 200]
        } else {
            [250, 250, 250]
        })
    }));
    let graphic_id = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        graphic.to_rgb8().into_raw(),
    ));
    // 1200 px over 144 pt (2 in) = 600 DPI.
    one_page_pdf(
        &mut doc,
        &[
            ("P", photo_id, [36.0, 600.0, 144.0, 108.0]),
            ("X", indexed, [216.0, 600.0, 144.0, 108.0]),
            ("C", cmyk, [396.0, 600.0, 48.0, 36.0]),
            ("G", graphic_id, [36.0, 400.0, 144.0, 108.0]),
        ],
    );
    doc.save(&input).unwrap();
    let before = std::fs::read(&input).unwrap();

    let out_dir = dir.path().join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let out = run(&input, &out_dir, opts(CompressPreset::Medium)).unwrap();
    let result = &out.outputs[0];
    println!("mixed.pdf: {} -> {} bytes", before.len(), len(result));
    assert!(len(result) < before.len() as u64 / 2);
    assert_valid(result, 1);
    assert_eq!(std::fs::read(&input).unwrap(), before);

    // Inspect the rewritten images.
    let doc = Document::load(result).unwrap();
    let mut seen = Vec::new();
    for obj in doc.objects.values() {
        let Ok(s) = obj.as_stream() else { continue };
        if s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image") {
            continue;
        }
        let w = s.dict.get(b"Width").unwrap().as_i64().unwrap();
        let filter = s
            .filters()
            .unwrap_or_default()
            .first()
            .map(|f| String::from_utf8_lossy(f).into_owned());
        let cs = s.dict.get(b"ColorSpace").unwrap();
        let kind = match cs {
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            Object::Array(a) => String::from_utf8_lossy(a[0].as_name().unwrap()).into_owned(),
            _ => "?".into(),
        };
        if s.dict.has(b"SMask") {
            // Photo: JPEG, 150 DPI over 2 in = 300 px; its soft mask stays a separate gray image.
            assert_eq!((w, filter.as_deref()), (300, Some("DCTDecode")));
            let m = doc
                .get_object(s.dict.get(b"SMask").unwrap().as_reference().unwrap())
                .unwrap()
                .as_stream()
                .unwrap();
            assert_eq!(m.dict.get(b"Width").unwrap().as_i64().unwrap(), 300);
            assert_eq!(m.filters().unwrap(), vec![b"FlateDecode".as_slice()]);
            seen.push("photo");
        } else if kind == "Indexed" {
            assert_eq!(w, 1200, "indexed images are left alone");
            seen.push("indexed");
        } else if kind == "DeviceCMYK" {
            assert_eq!(w, 400, "CMYK images are left alone");
            seen.push("cmyk");
        } else if kind == "DeviceRGB" {
            // Flat graphic: downscaled but lossless.
            assert_eq!((w, filter.as_deref()), (300, Some("FlateDecode")));
            seen.push("graphic");
        } else if kind == "DeviceGray" {
            seen.push("mask");
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, ["cmyk", "graphic", "indexed", "mask", "photo"]);
}

/// PNG "Sub" prediction on every row (as `/Predictor 15` expects).
fn png_sub(raw: &[u8], stride: usize, bpp: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / stride);
    for row in raw.chunks(stride) {
        out.push(1);
        for i in 0..stride {
            let left = if i >= bpp { row[i - bpp] } else { 0 };
            out.push(row[i].wrapping_sub(left));
        }
    }
    out
}

/// `/DecodeParms` given as a one-element array or as a reference must still
/// undo the predictor (lopdf alone only reads the inline-dictionary form).
#[test]
fn predictor_parms_in_array_or_reference() {
    let (w, h) = (1200u32, 900u32);
    let predicted = png_sub(&photo(w, h).into_raw(), w as usize * 3, 3);
    for form in ["array", "reference"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join(format!("{form}.pdf"));
        let mut doc = Document::with_version("1.5");
        let parms = dictionary! {
            "Predictor" => 15, "Colors" => 3, "BitsPerComponent" => 8, "Columns" => w as i64,
        };
        let parms_obj: Object = if form == "array" {
            vec![Object::Dictionary(parms)].into()
        } else {
            doc.add_object(parms).into()
        };
        let mut s = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64,
                "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            predicted.clone(),
        );
        s.compress().unwrap();
        s.dict
            .set("Filter", vec![Object::Name(b"FlateDecode".to_vec())]);
        s.dict.set("DecodeParms", parms_obj);
        let id = doc.add_object(s);
        one_page_pdf(&mut doc, &[("P", id, [36.0, 600.0, 144.0, 108.0])]);
        doc.save(&input).unwrap();

        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        let out = run(&input, &out_dir, opts(CompressPreset::Medium)).unwrap();
        let [result] = out.outputs.as_slice() else {
            panic!("{form}: {out:?}");
        };
        assert_valid(result, 1);
        let (a, b) = (mean_color(&input, 0), mean_color(result, 0));
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 2.0, "{form}: {a:?} vs {b:?}");
        }
        // The photo was really rewritten (downscaled to 300 px), not left alone.
        let doc = Document::load(result).unwrap();
        let widths: Vec<i64> = doc
            .objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"))
            .map(|s| s.dict.get(b"Width").unwrap().as_i64().unwrap())
            .collect();
        assert_eq!(widths, [300], "{form}");
    }
}

/// Signed PDFs (`/SigFlags` with signatures, or `/Perms`) are left unchanged.
#[test]
fn signed_pdf_is_left_unchanged() {
    for form in ["sigflags", "perms", "unsigned"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join(format!("{form}.pdf"));
        let mut doc = Document::with_version("1.5");
        let (w, h) = (1200u32, 900u32);
        let id = doc.add_object(flate(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64,
                "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            photo(w, h).into_raw(),
        ));
        one_page_pdf(&mut doc, &[("P", id, [36.0, 600.0, 144.0, 108.0])]);
        let catalog_id = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let (key, value): (&str, Object) = match form {
            "sigflags" => {
                let acro = doc.add_object(dictionary! { "Fields" => vec![], "SigFlags" => 3 });
                ("AcroForm", acro.into())
            }
            "perms" => ("Perms", dictionary! { "DocMDP" => Object::Null }.into()),
            // Only "append only" (bit 2) without bit 1: not signed.
            _ => (
                "AcroForm",
                dictionary! { "Fields" => vec![], "SigFlags" => 2 }.into(),
            ),
        };
        doc.get_dictionary_mut(catalog_id).unwrap().set(key, value);
        doc.save(&input).unwrap();
        let before = std::fs::read(&input).unwrap();

        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        let out = run(&input, &out_dir, opts(CompressPreset::Medium)).unwrap();
        if form == "unsigned" {
            assert_eq!(out.outputs.len(), 1, "{out:?}");
            continue;
        }
        assert!(out.outputs.is_empty(), "{form}: {out:?}");
        assert_eq!(
            out.note.as_deref(),
            Some(
                "This PDF is digitally signed, so it was left unchanged to keep the signature valid."
            )
        );
        assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0);
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }
}

/// A one-page photo PDF encrypted with an empty user password, an owner
/// password and `permissions` (RC4 128-bit).
fn encrypted_photo_pdf(path: &Path, permissions: lopdf::Permissions) {
    let mut doc = Document::with_version("1.5");
    let (w, h) = (1200u32, 900u32);
    let id = doc.add_object(flate(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        photo(w, h).into_raw(),
    ));
    one_page_pdf(&mut doc, &[("P", id, [36.0, 600.0, 144.0, 108.0])]);
    let file_id = Object::String(b"morph-test-id-01".to_vec(), lopdf::StringFormat::Literal);
    doc.trailer.set("ID", vec![file_id.clone(), file_id]);
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions,
    })
    .unwrap();
    doc.encrypt(&state).unwrap();
    doc.save(path).unwrap();
}

#[test]
fn restricted_pdf_is_left_unchanged() {
    use lopdf::Permissions;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("restricted.pdf");
    encrypted_photo_pdf(
        &input,
        Permissions::all() - Permissions::PRINTABLE - Permissions::COPYABLE,
    );
    // PDFium opens it without a password.
    assert_valid(&input, 1);
    let before = std::fs::read(&input).unwrap();
    let out_dir = dir.path().join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let out = run(&input, &out_dir, opts(CompressPreset::Medium)).unwrap();
    assert!(out.outputs.is_empty(), "{out:?}");
    assert_eq!(
        out.note.as_deref(),
        Some("This PDF has usage restrictions, so it was left unchanged.")
    );
    assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0);
    assert_eq!(std::fs::read(&input).unwrap(), before);

    // Encrypted but with every permission: nothing to lose, so it is compressed.
    let open = dir.path().join("open.pdf");
    encrypted_photo_pdf(&open, Permissions::all());
    let out = run(&open, &out_dir, opts(CompressPreset::Medium)).unwrap();
    assert_eq!(out.outputs.len(), 1, "{out:?}");
    assert_valid(&out.outputs[0], 1);
}
