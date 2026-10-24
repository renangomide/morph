// SPDX-License-Identifier: GPL-3.0-or-later
//! Lines in reading order → Markdown blocks.
//!
//! - Tables: ≥ 3 consecutive lines with ≥ 2 segments whose cells fall into ≥ 2
//!   shared columns (the line with the most segments sets the columns).
//! - Headings: font size ≥ 1.15 × the body (modal) size, levels by size rank
//!   (largest = `#`, at most `###`); short all-bold lines at body size one level
//!   below; a line matching a PDF outline entry takes that entry's depth.
//! - Lists: a line starting with a bullet or `1.` / `a)` / `iv.`; nesting by the
//!   marker's indent; lines aligned with the item text continue the item.
//! - Paragraphs: lines joined while the line spacing stays normal, the line
//!   isn't indented and the previous line didn't end a sentence well short of the
//!   right edge. Across pages/columns only when the sentence clearly goes on.
//! - De-hyphenation: `environ-` + `mental` → `environmental` (lower-case next).

use super::extract::{BULLETS, Word, letter_marker, ordered_marker};
use super::layout::{Line, modal_size};
use crate::markdown::{Inline, ListKind, MarkdownWriter, Style, escape_text};
use std::collections::HashMap;

/// One item of the document stream.
pub(super) enum Item {
    Text(Line),
    /// Ready-made image Markdown.
    Image(String),
}

/// Text run of a paragraph or list item.
struct Tok {
    text: String,
    bold: bool,
    italic: bool,
}

enum Block {
    Heading(u8, String),
    Para(Vec<Tok>),
    Item(usize, ListKind, Vec<Tok>),
    Table(Vec<Vec<String>>),
    Image(String),
}

/// Where the open paragraph / list item stands.
struct Open {
    page: usize,
    base: f32,
    x0: f32,
    x1: f32,
    left: f32,
    right: f32,
    size: f32,
    /// List item: x of the item's text (continuation lines align with it).
    item_text_x: Option<f32>,
}

enum Kind {
    Heading(u8),
    /// Size-based heading candidate; level set once all sizes are known.
    SizeHeading(f32),
    BoldHeading,
    Other,
}

pub(super) fn to_markdown(items: Vec<Item>, outline: &HashMap<String, u8>) -> String {
    let lines = items.iter().filter_map(|i| match i {
        Item::Text(l) => Some(l),
        Item::Image(_) => None,
    });
    let body = modal_size(lines.clone().flat_map(Line::words));
    let leading = body_leading(lines, body);

    // Classify; tables first.
    let tables = find_tables(&items);
    let mut in_table = vec![false; items.len()];
    for t in &tables {
        in_table[t.0..t.1].fill(true);
    }
    let kinds: Vec<Kind> = items
        .iter()
        .zip(&in_table)
        .map(|(item, &in_table)| match item {
            Item::Text(l) if !in_table => classify(l, body, outline),
            _ => Kind::Other,
        })
        .collect();
    let mut sizes: Vec<i32> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::SizeHeading(s) => Some((s * 2.0).round() as i32),
            _ => None,
        })
        .collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    let level_of = |s: f32| {
        let s = (s * 2.0).round() as i32;
        let rank = sizes.iter().position(|x| *x == s).unwrap_or(0);
        (rank as u8 + 1).min(3)
    };
    let bold_level = (sizes.len() as u8 + 1).min(3);

    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Open> = None;
    // Marker x and last number (0: bullet) of the open list at each depth.
    let mut list_stack: Vec<(f32, u32)> = Vec::new();
    // Tables are sorted and never overlap: the next one is always `tables[t]`.
    let mut t = 0;
    let mut i = 0;
    while i < items.len() {
        if let Some((_, end, rows)) = tables.get(t).filter(|tb| tb.0 == i) {
            t += 1;
            let end = *end;
            blocks.push(Block::Table(rows.clone()));
            open = None;
            list_stack.clear();
            i = end;
            continue;
        }
        let line = match &items[i] {
            Item::Image(md) => {
                blocks.push(Block::Image(md.clone()));
                open = None;
                list_stack.clear();
                i += 1;
                continue;
            }
            Item::Text(l) => l,
        };
        let level = match kinds[i] {
            Kind::Heading(l) => Some(l),
            Kind::SizeHeading(s) => Some(level_of(s)),
            Kind::BoldHeading => Some(bold_level),
            Kind::Other => None,
        };
        i += 1;
        let size = line.size();
        if let Some(level) = level {
            let text = line.text();
            // A heading wrapped over two lines.
            if let (Some(Block::Heading(l, prev)), Some(o)) = (blocks.last_mut(), &open)
                && *l == level
                && o.item_text_x.is_none()
                && o.page == line.page
                && (o.size - size).abs() < 0.5
                && o.base - line.base() > 0.0
                && o.base - line.base() < 1.6 * size
            {
                prev.push(' ');
                prev.push_str(&text);
            } else {
                blocks.push(Block::Heading(level, text));
            }
            open = Some(Open {
                page: line.page,
                base: line.base(),
                x0: line.x0(),
                x1: line.x1(),
                left: line.x0(),
                right: line.x1(),
                size,
                item_text_x: None,
            });
            list_stack.clear();
            continue;
        }
        let words: Vec<&Word> = line.words().collect();
        if let Some((mut kind, skip)) = list_marker(&words) {
            let marker_x = line.x0();
            while list_stack
                .last()
                .is_some_and(|(x, _)| marker_x < x - 0.5 * size)
            {
                list_stack.pop();
            }
            if list_stack
                .last()
                .is_none_or(|(x, _)| marker_x > x + 0.5 * size)
            {
                list_stack.push((marker_x, 0));
            }
            if let ListKind::Ordered(n) = &mut kind {
                // `i.` after `h.` (or `v.` after `u.`, `x.` after `w.`) is a letter.
                if let Some(letter) = words.first().and_then(|w| letter_marker(&w.text))
                    && letter != *n
                    && list_stack
                        .last()
                        .is_some_and(|&(_, prev)| prev + 1 == letter)
                {
                    *n = letter;
                }
            }
            if let Some(top) = list_stack.last_mut() {
                top.1 = match kind {
                    ListKind::Ordered(n) => n,
                    _ => 0,
                };
            }
            let text_x = words.get(skip).map_or(marker_x, |w| w.x0);
            let mut toks = Vec::new();
            push_words(&mut toks, words.get(skip..).unwrap_or(&[]));
            if skip == 0
                && let Some(t) = toks.first_mut()
            {
                t.text = t.text.chars().skip(1).collect();
            }
            blocks.push(Block::Item(list_stack.len() - 1, kind, toks));
            open = Some(open_for(line, Some(text_x)));
            continue;
        }
        let continues = match (&open, blocks.last()) {
            (Some(o), Some(Block::Para(toks) | Block::Item(_, _, toks))) => {
                continues(o, line, toks, leading)
            }
            _ => false,
        };
        if continues {
            if let Some(Block::Para(toks) | Block::Item(_, _, toks)) = blocks.last_mut() {
                push_words(toks, &words);
            }
            if let Some(o) = open.as_mut() {
                o.page = line.page;
                o.base = line.base();
                o.x0 = line.x0();
                o.x1 = line.x1();
                o.left = o.left.min(line.x0());
                o.right = o.right.max(line.x1());
            }
            continue;
        }
        let mut toks = Vec::new();
        push_words(&mut toks, &words);
        blocks.push(Block::Para(toks));
        open = Some(open_for(line, None));
        list_stack.clear();
    }
    render(blocks)
}

fn open_for(line: &Line, item_text_x: Option<f32>) -> Open {
    Open {
        page: line.page,
        base: line.base(),
        x0: line.x0(),
        x1: line.x1(),
        left: item_text_x.unwrap_or(line.x0()),
        right: line.x1(),
        size: line.size(),
        item_text_x,
    }
}

/// Does `line` go on with the open paragraph / list item?
fn continues(o: &Open, line: &Line, toks: &[Tok], leading: f32) -> bool {
    let size = line.size();
    let ends_sentence = toks
        .last()
        .and_then(|t| t.text.chars().last())
        .is_some_and(|c| ".!?:".contains(c));
    let starts_lower = line
        .first()
        .and_then(|w| w.text.chars().next())
        .is_some_and(char::is_lowercase);
    if (o.size - size).abs() > 0.15 * size.max(o.size) {
        return false;
    }
    let delta = o.base - line.base();
    let same_flow = line.page == o.page && delta > 0.0;
    if !same_flow {
        // Next page or column: only when the sentence clearly goes on.
        return !ends_sentence && starts_lower;
    }
    if delta > leading * 1.2 + 0.5 {
        return false;
    }
    let x0 = line.x0();
    if let Some(text_x) = o.item_text_x {
        return x0 >= text_x - 0.3 * size;
    }
    // First-line indent starts a new paragraph.
    if x0 > o.x0 + 0.8 * size && x0 > o.left + 0.8 * size {
        return false;
    }
    // A sentence ending well short of the right edge ended the paragraph.
    let width = o.right - o.left;
    !(ends_sentence && width > 0.0 && o.x1 < o.right - 0.15 * width)
}

/// Usual distance between baselines of body lines (default 1.2 × body size).
fn body_leading<'a>(lines: impl Iterator<Item = &'a Line>, body: f32) -> f32 {
    let mut counts: HashMap<i32, usize> = HashMap::new();
    let mut prev: Option<&Line> = None;
    for l in lines {
        if let Some(p) = prev
            && p.page == l.page
            && (p.size() - body).abs() < 0.5
            && (l.size() - body).abs() < 0.5
        {
            let d = p.base() - l.base();
            if d > 0.8 * body && d < 2.5 * body {
                *counts.entry((d * 2.0).round() as i32).or_default() += 1;
            }
        }
        prev = Some(l);
    }
    counts
        .into_iter()
        .max_by_key(|&(d, n)| (n, -d))
        .map_or(body * 1.2, |(d, _)| d as f32 / 2.0)
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn outline_key(title: &str) -> String {
    normalize(title)
}

fn classify(line: &Line, body: f32, outline: &HashMap<String, u8>) -> Kind {
    let text = line.text();
    let chars = text.chars().count();
    if chars > 200 || !text.chars().any(char::is_alphabetic) {
        return Kind::Other;
    }
    if let Some(level) = outline.get(&normalize(&text)) {
        return Kind::Heading(*level);
    }
    let size = line.size();
    if size >= body * 1.15 && chars <= 150 {
        return Kind::SizeHeading(size);
    }
    let words = line.words().count();
    let last = text.chars().last().unwrap_or(' ');
    if line.bold()
        && size >= body * 0.95
        && chars <= 80
        && words <= 12
        && !".,;".contains(last)
        && list_marker(&line.words().collect::<Vec<_>>()).is_none()
    {
        return Kind::BoldHeading;
    }
    Kind::Other
}

/// List kind and how many words the marker takes (0: a bullet glued to the
/// first word, `•Text`, which the caller strips).
fn list_marker(words: &[&Word]) -> Option<(ListKind, usize)> {
    let first = words.first()?;
    let mut cs = first.text.chars();
    let c = cs.next()?;
    let rest = cs.as_str();
    if BULLETS.contains(&c) {
        if rest.is_empty() {
            return (words.len() >= 2).then_some((ListKind::Bullet, 1));
        }
        let glued = !"-*–—".contains(c) && rest.chars().next().is_some_and(char::is_alphabetic);
        return glued.then_some((ListKind::Bullet, 0));
    }
    if words.len() < 2 {
        return None;
    }
    ordered_marker(&first.text).map(|n| (ListKind::Ordered(n), 1))
}

/// Append `words` to `toks`, joining a hyphenated line end with a lower-case
/// continuation.
fn push_words(toks: &mut Vec<Tok>, words: &[&Word]) {
    for (i, w) in words.iter().enumerate() {
        if i == 0
            && let Some(prev) = toks.last_mut()
            && w.text.chars().next().is_some_and(char::is_lowercase)
            && let Some(stem) = prev.text.strip_suffix('-')
            && stem.chars().last().is_some_and(char::is_alphabetic)
        {
            prev.text = format!("{stem}{}", w.text);
            continue;
        }
        toks.push(Tok {
            text: w.text.clone(),
            bold: w.bold,
            italic: w.italic,
        });
    }
}

fn inline(toks: &[Tok]) -> String {
    let mut out = Inline::new();
    for (i, t) in toks.iter().enumerate() {
        if i > 0 {
            let prev = &toks[i - 1];
            let space = Style {
                bold: prev.bold && t.bold,
                italic: prev.italic && t.italic,
                ..Style::plain()
            };
            out.push_text(" ", &space);
        }
        let style = Style {
            bold: t.bold,
            italic: t.italic,
            ..Style::plain()
        };
        out.push_text(&t.text, &style);
    }
    out.finish()
}

/// Tables as `(start, end, rows)` item ranges.
fn find_tables(items: &[Item]) -> Vec<(usize, usize, Vec<Vec<String>>)> {
    let multi = |i: usize| match &items[i] {
        Item::Text(l) if l.segs.len() >= 2 => Some(l),
        _ => None,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let Some(first) = multi(i) else {
            i += 1;
            continue;
        };
        let mut end = i + 1;
        while end < items.len() && multi(end).is_some_and(|l| l.page == first.page) {
            end += 1;
        }
        let run: Vec<&Line> = (i..end).filter_map(multi).collect();
        if let Some(rows) = table_rows(&run) {
            out.push((i, end, rows));
        }
        i = end;
    }
    out
}

fn table_rows(run: &[&Line]) -> Option<Vec<Vec<String>>> {
    if run.len() < 3 {
        return None;
    }
    let template = run.iter().max_by_key(|l| l.segs.len())?;
    let cols: Vec<(f32, f32)> = template.segs.iter().map(|s| (s.x0(), s.x1())).collect();
    let mut rows = Vec::with_capacity(run.len());
    for line in run {
        let mut cells = vec![String::new(); cols.len()];
        for seg in &line.segs {
            let (a, b) = (seg.x0(), seg.x1());
            let col = (0..cols.len())
                .max_by(|&x, &y| {
                    let score = |c: usize| {
                        let (c0, c1) = cols[c];
                        let overlap = b.min(c1) - a.max(c0);
                        let dist = ((a + b) - (c0 + c1)).abs() / 2.0;
                        if overlap > 0.0 { overlap } else { -dist }
                    };
                    score(x).total_cmp(&score(y))
                })
                .unwrap_or(0);
            if !cells[col].is_empty() {
                cells[col].push(' ');
            }
            cells[col].push_str(&escape_text(&seg.text()));
        }
        rows.push(cells);
    }
    let filled = (0..cols.len())
        .filter(|&c| rows.iter().filter(|r| !r[c].is_empty()).count() >= 3)
        .count();
    (filled >= 2).then_some(rows)
}

fn render(blocks: Vec<Block>) -> String {
    let mut w = MarkdownWriter::new();
    for b in blocks {
        match b {
            Block::Heading(level, text) => w.heading(level, &escape_text(&text)),
            Block::Para(toks) => w.paragraph(&inline(&toks)),
            Block::Item(depth, kind, toks) => w.list_item(depth, kind, &inline(&toks)),
            Block::Table(rows) => w.table(&rows),
            Block::Image(md) => w.paragraph(&md),
        }
    }
    w.finish()
}
