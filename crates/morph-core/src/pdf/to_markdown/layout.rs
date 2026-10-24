// SPDX-License-Identifier: GPL-3.0-or-later
//! Page rows → lines in reading order.
//!
//! - Running headers/footers: a row in the top or bottom 12 % of the page whose
//!   text (digits ignored) shows up in the same band on more than half the
//!   pages is dropped; so are bare page numbers (`3`, `Page 3 of 9`, `iv`) there.
//! - Two columns: a vertical strip in the middle 40 % of the page that no row
//!   crosses, with ≥ 3 rows of prose on each side, splits that run of rows:
//!   the left side is read first, then the right side.

use super::extract::{PageText, Row, Seg, Word, roman};
use std::collections::HashMap;

/// A line of text in reading order. More than one segment: table row candidate.
#[derive(Debug, Clone)]
pub(super) struct Line {
    pub page: usize,
    pub segs: Vec<Seg>,
}

impl Line {
    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.segs.iter().flat_map(|s| s.words.iter())
    }
    pub fn first(&self) -> Option<&Word> {
        self.words().next()
    }
    pub fn x0(&self) -> f32 {
        self.first().map_or(0.0, |w| w.x0)
    }
    pub fn x1(&self) -> f32 {
        self.segs.last().map_or(0.0, Seg::x1)
    }
    pub fn base(&self) -> f32 {
        self.first().map_or(0.0, |w| w.base)
    }
    pub fn top(&self) -> f32 {
        self.words().map(|w| w.top).fold(f32::MIN, f32::max)
    }
    /// Font size most of the characters use.
    pub fn size(&self) -> f32 {
        modal_size(self.words())
    }
    pub fn bold(&self) -> bool {
        self.words().all(|w| w.bold)
    }
    pub fn text(&self) -> String {
        self.segs
            .iter()
            .map(Seg::text)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Font size of most characters (rounded to 0.5 pt).
pub(super) fn modal_size<'a>(words: impl Iterator<Item = &'a Word>) -> f32 {
    let mut counts: HashMap<i32, usize> = HashMap::new();
    for w in words {
        *counts.entry((w.size * 2.0).round() as i32).or_default() += w.text.chars().count();
    }
    counts
        .into_iter()
        .max_by_key(|&(size, n)| (n, size))
        .map_or(0.0, |(size, _)| size as f32 / 2.0)
}

const BAND: f32 = 0.12;

/// Which band a row is in: `Some(true)` top, `Some(false)` bottom.
fn band(row: &Row, height: f32) -> Option<bool> {
    if row.bottom() >= height * (1.0 - BAND) {
        Some(true)
    } else if row.top() <= height * BAND {
        Some(false)
    } else {
        None
    }
}

fn band_key(row: &Row, top: bool) -> String {
    let text: String = row
        .text()
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| {
            if c.is_ascii_digit() {
                '#'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    format!("{top}{text}")
}

/// `3`, `- 3 -`, `Page 3`, `3 of 9`, `3/9`, `iv`.
pub(super) fn is_page_number(text: &str) -> bool {
    let t = text.trim().trim_matches(|c: char| "-–—|[]() ".contains(c));
    let lower = t.to_lowercase();
    let t = lower.strip_prefix("page").unwrap_or(&lower).trim();
    let num = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    if num(t) {
        return true;
    }
    if let Some((a, b)) = t.split_once(" of ").or_else(|| t.split_once('/')) {
        return num(a.trim()) && num(b.trim());
    }
    roman(t).is_some()
}

/// Drop running headers/footers and page numbers from every page.
pub(super) fn remove_furniture(pages: &mut [PageText]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for page in pages.iter() {
        let mut keys: Vec<String> = page
            .rows
            .iter()
            .filter_map(|r| band(r, page.height).map(|top| band_key(r, top)))
            .collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            *seen.entry(k).or_default() += 1;
        }
    }
    let n = pages.len();
    let body = modal_size(
        pages
            .iter()
            .flat_map(|p| p.rows.iter().flat_map(Row::words)),
    );
    for (i, page) in pages.iter_mut().enumerate() {
        let height = page.height;
        page.rows.retain(|r| {
            let Some(top) = band(r, height) else {
                return true;
            };
            let repeated = n >= 2 && seen.get(&band_key(r, top)).is_some_and(|c| c * 2 > n);
            // A repeated title in large type stays once, on the first page.
            let title = i == 0 && modal_size(r.words()) >= 1.15 * body;
            !((repeated && !title) || is_page_number(&r.text()))
        });
    }
}

/// Lines of one page in reading order.
pub(super) fn page_lines(page_index: usize, page: &PageText) -> Vec<Line> {
    let rows = &page.rows;
    let line = |segs: Vec<Seg>| Line {
        page: page_index,
        segs,
    };
    let Some((gutter, runs)) = find_gutter(rows, page.width) else {
        return rows.iter().map(|r| line(r.segs.clone())).collect();
    };
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        let Some(&(start, end)) = runs.iter().find(|(s, _)| *s == i) else {
            out.push(line(rows[i].segs.clone()));
            i += 1;
            continue;
        };
        for left in [true, false] {
            for row in &rows[start..end] {
                let segs: Vec<Seg> = row
                    .segs
                    .iter()
                    .filter(|s| (s.x1() <= gutter) == left)
                    .cloned()
                    .collect();
                if !segs.is_empty() {
                    out.push(line(segs));
                }
            }
        }
        i = end;
    }
    out
}

/// The gutter x and the row ranges `[start, end)` read as two columns.
fn find_gutter(rows: &[Row], width: f32) -> Option<(f32, Vec<(usize, usize)>)> {
    type Runs = Vec<(usize, usize)>;
    // Absurd page sizes (MediaBox 0 0 50000000 800): no column search.
    if !width.is_finite() || width <= 0.0 || width > 20_000.0 {
        return None;
    }
    let step = (0.4 * width / 200.0).max(2.0);
    let steps = (0.4 * width / step) as usize;
    let mut best: Option<(usize, f32, Runs)> = None;
    for k in 0..=steps {
        let x = width * 0.3 + k as f32 * step;
        let runs = column_runs(rows, x);
        let score: usize = runs.iter().map(|(s, e)| e - s).sum();
        if score > 0 && best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, x, runs));
        }
    }
    best.map(|(_, x, runs)| (x, runs))
}

fn column_runs(rows: &[Row], x: f32) -> Vec<(usize, usize)> {
    let crosses = |r: &Row| r.segs.iter().any(|s| s.x0() < x && s.x1() > x);
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 0..=rows.len() {
        if i == rows.len() || crosses(&rows[i]) {
            if is_two_columns(&rows[start..i], x) {
                runs.push((start, i));
            }
            start = i + 1;
        }
    }
    runs
}

/// ≥ 3 rows of prose (≥ 3 words per segment on average) on each side.
fn is_two_columns(rows: &[Row], x: f32) -> bool {
    let side = |left: bool| {
        let segs: Vec<&Seg> = rows
            .iter()
            .flat_map(|r| r.segs.iter())
            .filter(|s| (s.x1() <= x) == left)
            .collect();
        let rows_with = rows
            .iter()
            .filter(|r| r.segs.iter().any(|s| (s.x1() <= x) == left))
            .count();
        let words: usize = segs.iter().map(|s| s.words.len()).sum();
        rows_with >= 3 && words >= 3 * segs.len()
    };
    side(true) && side(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_numbers() {
        for t in ["3", "- 12 -", "Page 4", "page 2 of 9", "3/10", "iv"] {
            assert!(is_page_number(t), "{t}");
        }
        for t in ["Results", "2026 report", "12345", "", "civil", "ill", "Lic"] {
            assert!(!is_page_number(t), "{t}");
        }
    }
}
