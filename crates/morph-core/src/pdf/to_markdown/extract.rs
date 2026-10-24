// SPDX-License-Identifier: GPL-3.0-or-later
//! PDFium → words → rows → segments, one page at a time.
//!
//! - Every character comes with its loose box, baseline, font size and font
//!   name (bold / italic from the name or weight). Whitespace PDFium reports
//!   (real or generated) only marks a word break; gaps are measured from the boxes.
//! - Rows: characters whose baselines are within ~0.45 em, left to right.
//! - Words: split at gaps wider than 0.15 em. Segments: split at gaps wider than
//!   1.5 em (table cells, the two sides of a column gutter).

use crate::error::ConvertError;
use crate::pdf::pdfium;
use pdfium_render::prelude::*;

#[derive(Debug, Clone)]
pub(super) struct Word {
    pub text: String,
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub bottom: f32,
    pub base: f32,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

/// Words on one row with no wide gap between them.
#[derive(Debug, Clone)]
pub(super) struct Seg {
    pub words: Vec<Word>,
}

impl Seg {
    pub fn x0(&self) -> f32 {
        self.words.first().map_or(0.0, |w| w.x0)
    }
    pub fn x1(&self) -> f32 {
        self.words.last().map_or(0.0, |w| w.x1)
    }
    pub fn text(&self) -> String {
        join_words(&self.words)
    }
}

/// One visual row (same baseline) of a page.
#[derive(Debug, Clone)]
pub(super) struct Row {
    pub segs: Vec<Seg>,
}

impl Row {
    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.segs.iter().flat_map(|s| s.words.iter())
    }
    pub fn top(&self) -> f32 {
        self.words().map(|w| w.top).fold(f32::MIN, f32::max)
    }
    pub fn bottom(&self) -> f32 {
        self.words().map(|w| w.bottom).fold(f32::MAX, f32::min)
    }
    pub fn text(&self) -> String {
        let words: Vec<Word> = self.words().cloned().collect();
        join_words(&words)
    }
}

pub(super) fn join_words(words: &[Word]) -> String {
    let mut s = String::new();
    for w in words {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&w.text);
    }
    s
}

pub(super) struct PageText {
    pub width: f32,
    pub height: f32,
    /// Top to bottom.
    pub rows: Vec<Row>,
}

struct Glyph {
    c: char,
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
    base: f32,
    size: f32,
    bold: bool,
    italic: bool,
    /// PDFium reported whitespace right before this character.
    brk: bool,
}

/// Read the characters of `page` and group them into rows.
pub(super) fn read_page(page: &PdfPage<'_>) -> Result<PageText, ConvertError> {
    let width = page.width().value;
    let height = page.height().value;
    let text = page
        .text()
        .map_err(|e| pdfium::map_error(e, "read page text"))?;
    let mut glyphs = Vec::new();
    let mut brk = false;
    // Font name → (bold, italic); names repeat on every character.
    let mut fonts: Vec<(String, bool, bool)> = Vec::new();
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else {
            brk = true;
            continue;
        };
        // PDFium marks a hyphen at a line end as U+0002.
        let c = if c == '\u{2}' { '-' } else { c };
        if c.is_whitespace() || c.is_control() || c == '\u{FFFE}' || c == '\u{FFFF}' {
            brk = true;
            continue;
        }
        let Ok(r) = ch.loose_bounds() else {
            continue;
        };
        let (x0, x1, top, bottom) = (
            r.left().value,
            r.right().value,
            r.top().value,
            r.bottom().value,
        );
        let base = ch.origin_y().map(|v| v.value).unwrap_or(bottom);
        let mut size = ch.scaled_font_size().value;
        if !size.is_finite() || size <= 0.5 {
            size = (top - bottom).max(1.0);
        }
        let name = ch.font_name();
        let (bold, italic) = match fonts.iter().find(|f| f.0 == name) {
            Some(f) => (f.1, f.2),
            None => {
                let lower = name.to_ascii_lowercase();
                let weight_bold = matches!(
                    ch.font_weight(),
                    Some(
                        PdfFontWeight::Weight600
                            | PdfFontWeight::Weight700Bold
                            | PdfFontWeight::Weight800
                            | PdfFontWeight::Weight900
                    )
                );
                let bold = weight_bold
                    || ["bold", "black", "heavy", "semibold", "demi"]
                        .iter()
                        .any(|k| lower.contains(k));
                let italic = lower.contains("italic") || lower.contains("oblique");
                fonts.push((name, bold, italic));
                (bold, italic)
            }
        };
        glyphs.push(Glyph {
            c,
            x0,
            x1,
            top,
            bottom,
            base,
            size,
            bold,
            italic,
            brk,
        });
        brk = false;
    }
    Ok(PageText {
        width,
        height,
        rows: group_rows(glyphs),
    })
}

fn group_rows(mut glyphs: Vec<Glyph>) -> Vec<Row> {
    glyphs.sort_by(|a, b| b.base.total_cmp(&a.base));
    let mut rows = Vec::new();
    let mut current: Vec<Glyph> = Vec::new();
    let mut row_base = 0.0f32;
    let mut row_size = 0.0f32;
    for g in glyphs {
        if !current.is_empty() && (row_base - g.base).abs() > 0.45 * row_size.min(g.size) {
            rows.push(make_row(std::mem::take(&mut current)));
        }
        if current.is_empty() {
            row_base = g.base;
            row_size = g.size;
        }
        current.push(g);
    }
    if !current.is_empty() {
        rows.push(make_row(current));
    }
    rows.retain(|r| !r.segs.is_empty());
    rows
}

fn make_row(mut glyphs: Vec<Glyph>) -> Row {
    glyphs.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    // Text drawn twice for a fake bold effect.
    glyphs
        .dedup_by(|b, a| a.c == b.c && (a.x0 - b.x0).abs() < 0.5 && (a.base - b.base).abs() < 0.5);
    let mut words: Vec<Word> = Vec::new();
    let mut prev_x1 = f32::MIN;
    for g in &glyphs {
        let gap = g.x0 - prev_x1;
        let new_word = words.is_empty() || gap > 0.15 * g.size || (g.brk && gap > 0.05 * g.size);
        prev_x1 = prev_x1.max(g.x1);
        if new_word {
            words.push(Word {
                text: String::new(),
                x0: g.x0,
                x1: g.x1,
                top: g.top,
                bottom: g.bottom,
                base: g.base,
                size: g.size,
                bold: g.bold,
                italic: g.italic,
            });
        }
        if let Some(w) = words.last_mut() {
            push_char(&mut w.text, g.c);
            w.x1 = w.x1.max(g.x1);
            w.top = w.top.max(g.top);
            w.bottom = w.bottom.min(g.bottom);
            w.size = w.size.max(g.size);
            w.bold &= g.bold;
            w.italic &= g.italic;
        }
    }
    let mut segs: Vec<Seg> = Vec::new();
    for w in words {
        match segs.last_mut() {
            Some(s) if w.x0 - s.x1() <= 1.5 * w.size => s.words.push(w),
            _ => segs.push(Seg { words: vec![w] }),
        }
    }
    // A list marker set apart from its text stays with it.
    if segs.len() >= 2 && segs[0].words.len() == 1 && is_marker_word(&segs[0].words[0].text) {
        let rest = segs.remove(1);
        segs[0].words.extend(rest.words);
    }
    Row { segs }
}

fn push_char(s: &mut String, c: char) {
    match c {
        '\u{FB00}' => s.push_str("ff"),
        '\u{FB01}' => s.push_str("fi"),
        '\u{FB02}' => s.push_str("fl"),
        '\u{FB03}' => s.push_str("ffi"),
        '\u{FB04}' => s.push_str("ffl"),
        '\u{AD}' => s.push('-'),
        c => s.push(c),
    }
}

/// Bullet characters that start a list item.
pub(super) const BULLETS: &[char] = &[
    '•', '◦', '▪', '▫', '■', '□', '●', '○', '–', '—', '-', '*', '·', '‣', '➢', '►', '▶', '❖',
    '\u{F0B7}', '\u{F0A7}', '\u{F076}', '\u{F0D8}',
];

/// `•`, `1.`, `a)`, `iv.`, `(2)`: a word that can start a list item.
pub(super) fn is_marker_word(w: &str) -> bool {
    let mut cs = w.chars();
    (matches!((cs.next(), cs.next()), (Some(c), None) if BULLETS.contains(&c)))
        || ordered_marker(w).is_some()
}

/// The number of an ordered list marker: `3.` → 3, `b)` → 2, `iv.` → 4, `(2)` → 2.
pub(super) fn ordered_marker(w: &str) -> Option<u32> {
    let inner = if let Some(s) = w.strip_prefix('(') {
        s.strip_suffix(')')?
    } else {
        w.strip_suffix('.').or_else(|| w.strip_suffix(')'))?
    };
    if inner.is_empty() {
        return None;
    }
    if inner.len() <= 3 && inner.bytes().all(|b| b.is_ascii_digit()) {
        return inner.parse().ok();
    }
    // `i.`, `v.`, `x.` read as roman here; the caller picks the letter when
    // the previous item was `h.`, `u.`, `w.` (see [`letter_marker`]).
    if let Some(n) = roman(inner) {
        return Some(n);
    }
    letter_marker(w)
}

/// A one-letter marker's place in the alphabet: `b)` → 2, `i.` → 9.
pub(super) fn letter_marker(w: &str) -> Option<u32> {
    let inner = if let Some(s) = w.strip_prefix('(') {
        s.strip_suffix(')')?
    } else {
        w.strip_suffix('.').or_else(|| w.strip_suffix(')'))?
    };
    let mut cs = inner.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => {
            Some(u32::from(c.to_ascii_lowercase()) - u32::from('a') + 1)
        }
        _ => None,
    }
}

pub(super) fn roman(s: &str) -> Option<u32> {
    if s.len() > 6
        || !(s.chars().all(|c| "ivxlc".contains(c)) || s.chars().all(|c| "IVXLC".contains(c)))
    {
        return None;
    }
    let val = |c: char| match c.to_ascii_lowercase() {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        'l' => 50,
        _ => 100,
    };
    let digits: Vec<u32> = s.chars().map(val).collect();
    let mut total = 0;
    for (i, d) in digits.iter().enumerate() {
        if digits.get(i + 1).is_some_and(|n| n > d) {
            total -= *d as i64;
        } else {
            total += *d as i64;
        }
    }
    // Only well-formed numerals ("civil" is a word, not 1+100+…).
    let n = u32::try_from(total).ok().filter(|n| (1..40).contains(n))?;
    let tens = ["", "x", "xx", "xxx"][(n / 10) as usize];
    let ones = ["", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix"][(n % 10) as usize];
    (format!("{tens}{ones}") == s.to_ascii_lowercase()).then_some(n)
}

/// A picture placed on a page.
pub(super) struct PageImage {
    pub top: f32,
    pub bytes: Vec<u8>,
}

/// Pictures on `page` worth keeping (not tiny decorations), top to bottom.
/// Pictures that can't be read are skipped.
pub(super) fn read_images(page: &PdfPage<'_>) -> Vec<PageImage> {
    let mut out = Vec::new();
    for object in page.objects().iter() {
        let Some(img) = object.as_image_object() else {
            continue;
        };
        let Ok(b) = object.bounds() else {
            continue;
        };
        if b.width().value < 24.0 || b.height().value < 24.0 {
            continue;
        }
        let jpeg =
            img.filters().len() == 1 && img.filters().iter().all(|f| f.name() == "DCTDecode");
        let bytes = if jpeg {
            img.get_raw_image_data().ok()
        } else {
            img.get_raw_image().ok().and_then(|im| {
                if im.width() < 16 || im.height() < 16 {
                    return None;
                }
                let mut buf = std::io::Cursor::new(Vec::new());
                im.write_to(&mut buf, image::ImageFormat::Png).ok()?;
                Some(buf.into_inner())
            })
        };
        if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
            out.push(PageImage {
                top: b.top().value,
                bytes,
            });
        }
    }
    out.sort_by(|a, b| b.top.total_cmp(&a.top));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_markers() {
        assert_eq!(ordered_marker("3."), Some(3));
        assert_eq!(ordered_marker("b)"), Some(2));
        assert_eq!(ordered_marker("iv."), Some(4));
        assert_eq!(ordered_marker("(12)"), Some(12));
        assert_eq!(ordered_marker("2026."), None);
        assert_eq!(ordered_marker("word."), None);
        assert_eq!(ordered_marker("civil."), None);
        assert_eq!(ordered_marker("xii)"), Some(12));
        assert_eq!(ordered_marker("i."), Some(1));
        assert_eq!(ordered_marker("I."), Some(1));
        assert_eq!(ordered_marker("c)"), Some(3));
        assert_eq!(letter_marker("i."), Some(9));
        assert_eq!(letter_marker("ii."), None);
        assert!(is_marker_word("•") && is_marker_word("-") && !is_marker_word("--"));
    }
}
