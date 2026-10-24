// SPDX-License-Identifier: GPL-3.0-or-later
//! Regenerates the PDF test fixtures in `tests/fixtures/` (see the README there).
//!
//!     cargo run -p morph-core --example make_pdf_fixtures
//!
//! Everything is built from code (lopdf + the `image` JPEG encoder, fixed
//! pseudo-random seeds), so the output is the same on every run.

use image::codecs::jpeg::JpegEncoder;
use image::{GrayImage, RgbImage};
use lopdf::content::{Content, Operation};
use lopdf::{
    Document, EncryptionState, EncryptionVersion, Object, Permissions, Stream, dictionary,
};
use std::error::Error;
use std::path::{Path, PathBuf};

type Res<T> = Result<T, Box<dyn Error>>;

const LETTER: (f32, f32) = (612.0, 792.0);

/// Tiny deterministic PRNG (xorshift32).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// -amp..=amp
    fn noise(&mut self, amp: i32) -> i32 {
        (self.next() % (2 * amp as u32 + 1)) as i32 - amp
    }
}

struct Img {
    jpeg: Vec<u8>,
    width: u32,
    height: u32,
    gray: bool,
}

struct Placed {
    img: Img,
    /// x, y, width, height in points.
    rect: (f32, f32, f32, f32),
}

#[derive(Default)]
struct PageSpec {
    /// Lines of text (WinAnsi bytes), drawn from the top.
    lines: Vec<Vec<u8>>,
    images: Vec<Placed>,
}

fn text(s: &str) -> Vec<u8> {
    // Latin-1 subset of WinAnsiEncoding is enough for the fixtures (é, ã, ó).
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

fn build(pages: Vec<PageSpec>) -> Res<Document> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for spec in pages {
        let mut ops = Vec::new();
        let mut xobjects = lopdf::Dictionary::new();
        for (i, placed) in spec.images.into_iter().enumerate() {
            let name = format!("Im{i}");
            let dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => i64::from(placed.img.width),
                "Height" => i64::from(placed.img.height),
                "ColorSpace" => if placed.img.gray { "DeviceGray" } else { "DeviceRGB" },
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            };
            let id = doc.add_object(Stream::new(dict, placed.img.jpeg).with_compression(false));
            xobjects.set(name.as_bytes(), id);
            let (x, y, w, h) = placed.rect;
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "cm",
                vec![w.into(), 0.into(), 0.into(), h.into(), x.into(), y.into()],
            ));
            ops.push(Operation::new("Do", vec![Object::Name(name.into_bytes())]));
            ops.push(Operation::new("Q", vec![]));
        }
        if !spec.lines.is_empty() {
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), 12.into()]));
            ops.push(Operation::new("TL", vec![16.into()]));
            ops.push(Operation::new("Td", vec![72.into(), 720.into()]));
            for line in spec.lines {
                ops.push(Operation::new("Tj", vec![Object::string_literal(line)]));
                ops.push(Operation::new("T*", vec![]));
            }
            ops.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations: ops }.encode()?;
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), LETTER.0.into(), LETTER.1.into()],
            "Contents" => content_id,
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
                "XObject" => xobjects,
            },
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    // Fixed file ID (needed for encryption; keeps output reproducible).
    let id = Object::String(
        b"morph-fixture-id".to_vec(),
        lopdf::StringFormat::Hexadecimal,
    );
    doc.trailer.set("ID", vec![id.clone(), id]);
    doc.compress();
    Ok(doc)
}

fn save(doc: &mut Document, path: &Path) -> Res<()> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    std::fs::write(path, &bytes)?;
    println!("{:>9} bytes  {}", bytes.len(), path.display());
    Ok(())
}

fn jpeg_rgb(img: &RgbImage, quality: u8) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(img)?;
    Ok(out)
}

fn jpeg_gray(img: &GrayImage, quality: u8) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(img)?;
    Ok(out)
}

/// A photo-like picture: smooth colour gradients, soft discs, fine grain.
fn photo(width: u32, height: u32, seed: u32) -> RgbImage {
    let mut rng = Rng(seed.max(1));
    let discs: Vec<(f32, f32, f32, [f32; 3])> = (0..12)
        .map(|_| {
            let x = (rng.next() % width) as f32;
            let y = (rng.next() % height) as f32;
            let r = (width / 12 + rng.next() % (width / 5)) as f32;
            let c = [
                (rng.next() % 256) as f32,
                (rng.next() % 256) as f32,
                (rng.next() % 256) as f32,
            ];
            (x, y, r, c)
        })
        .collect();
    RgbImage::from_fn(width, height, |x, y| {
        let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
        let mut c = [
            60.0 + 150.0 * fx,
            80.0 + 120.0 * fy,
            200.0 - 120.0 * fx * fy,
        ];
        for (dx, dy, r, dc) in &discs {
            let d = ((x as f32 - dx).powi(2) + (y as f32 - dy).powi(2)).sqrt();
            let t = (1.0 - d / r).clamp(0.0, 1.0);
            for i in 0..3 {
                c[i] = c[i] * (1.0 - t * 0.7) + dc[i] * t * 0.7;
            }
        }
        let grain = rng.noise(6) as f32;
        image::Rgb(c.map(|v| (v + grain).clamp(0.0, 255.0) as u8))
    })
}

/// A "scanned" text page: grey paper, dark text-like bars, scanner noise. No real text.
fn scan(width: u32, height: u32, seed: u32) -> GrayImage {
    let mut rng = Rng(seed.max(1));
    let line_h = height / 40;
    let words: Vec<Vec<(u32, u32)>> = (0..32)
        .map(|_| {
            let mut x = width / 10;
            let mut row = Vec::new();
            while x < width * 9 / 10 {
                let w = width / 40 + rng.next() % (width / 12);
                row.push((x, (x + w).min(width * 9 / 10)));
                x += w + width / 80;
            }
            row
        })
        .collect();
    GrayImage::from_fn(width, height, |x, y| {
        let mut v = 235i32;
        let top = height / 10;
        if y >= top {
            let row = ((y - top) / line_h) as usize;
            let in_line = (y - top) % line_h;
            if row < words.len()
                && in_line > line_h / 4
                && in_line < line_h * 3 / 4
                && words[row].iter().any(|(a, b)| x >= *a && x < *b)
            {
                v = 40;
            }
        }
        image::Luma([(v + rng.noise(4)).clamp(0, 255) as u8])
    })
}

fn text_page(title: &str, n: u32, total: u32) -> PageSpec {
    PageSpec {
        lines: vec![
            text(&format!("{title} - Page {n} of {total}")),
            text(""),
            text("Morph test fixture. This line is real text that PDFium can extract."),
            text("Accented words: café, relatório, ação."),
            text(&format!(
                "The quick brown fox jumps over the lazy dog ({n})."
            )),
        ],
        images: Vec::new(),
    }
}

fn main() -> Res<()> {
    let out: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    std::fs::create_dir_all(&out)?;

    // 1. Text only, 3 pages.
    let mut doc = build((1..=3).map(|n| text_page("Text", n, 3)).collect())?;
    save(&mut doc, &out.join("text-3pages.pdf"))?;
    let text_bytes = std::fs::read(out.join("text-3pages.pdf"))?;

    // 2. Image-heavy: 3 pages, each a 2000x1500 photo at 300 DPI (q92), plus a caption.
    let mut pages = Vec::new();
    for n in 1..=3u32 {
        let pic = photo(2000, 1500, 0x5EED_0000 + n);
        pages.push(PageSpec {
            lines: vec![text(&format!("Image page {n} of 3"))],
            images: vec![Placed {
                img: Img {
                    jpeg: jpeg_rgb(&pic, 92)?,
                    width: 2000,
                    height: 1500,
                    gray: false,
                },
                // 2000 px over 480 pt (6.67 in) = 300 DPI.
                rect: (66.0, 200.0, 480.0, 360.0),
            }],
        });
    }
    save(&mut build(pages)?, &out.join("images.pdf"))?;

    // 3. Password-protected (user password "morph"), RC4 128-bit.
    let mut doc = build(vec![text_page("Secret", 1, 1)])?;
    let version = EncryptionVersion::V2 {
        document: &doc,
        owner_password: "morph-owner",
        user_password: "morph",
        key_length: 128,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version)?;
    doc.encrypt(&state)?;
    save(&mut doc, &out.join("password.pdf"))?;

    // 4. Scanned-like: 2 pages that are only a grey-scale picture of text, no text layer.
    let pages = (1..=2u32)
        .map(|n| -> Res<PageSpec> {
            let img = scan(1275, 1650, 0xACE0_0000 + n);
            Ok(PageSpec {
                lines: Vec::new(),
                images: vec![Placed {
                    img: Img {
                        jpeg: jpeg_gray(&img, 60)?,
                        width: 1275,
                        height: 1650,
                        gray: true,
                    },
                    rect: (0.0, 0.0, LETTER.0, LETTER.1),
                }],
            })
        })
        .collect::<Res<Vec<_>>>()?;
    save(&mut build(pages)?, &out.join("scanned.pdf"))?;

    // 5. Unicode file name (accent, CJK, space).
    let mut doc = build(vec![text_page("Relatório", 1, 1)])?;
    save(&mut doc, &out.join("Relatório 報告.pdf"))?;

    // 6. 100 text pages (performance: PDF -> PNG at 150 DPI under 30 s).
    let mut doc = build((1..=100).map(|n| text_page("Long", n, 100)).collect())?;
    save(&mut doc, &out.join("pages-100.pdf"))?;

    // 7. Damaged: the first 300 bytes of the text PDF.
    let cut = &text_bytes[..text_bytes.len().min(300)];
    std::fs::write(out.join("broken.pdf"), cut)?;
    println!(
        "{:>9} bytes  {}",
        cut.len(),
        out.join("broken.pdf").display()
    );
    Ok(())
}
