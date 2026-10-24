// SPDX-License-Identifier: GPL-3.0-or-later
//! Markdown (CommonMark + GFM tables/strikethrough/footnotes) emitters shared by
//! every "to Markdown" reader. Two layers:
//!
//! - **Inline**: [`escape_text`] for raw text, [`code_span`], [`link`], [`image`],
//!   [`strong`]/[`emphasis`]/[`strikethrough`], [`footnote_ref`], and the
//!   [`Inline`] builder that turns styled runs (DOCX/PPTX `w:r`/`a:r`, PDF
//!   chars) into one Markdown string, merging runs that share a style.
//! - **Blocks**: [`MarkdownWriter`] puts headings, paragraphs, (nested) list
//!   items, pipe tables, code blocks, quotes and footnotes together with the
//!   right blank lines and indentation, and protects line starts so plain text
//!   never turns into a heading/list/quote by accident.
//!
//! Convention: block methods take **inline Markdown** (already escaped, e.g. from
//! [`escape_text`] or [`Inline::finish`]). Raw text must go through
//! [`escape_text`] first.

use std::fmt::Write as _;

// ---------------------------------------------------------------- inline

/// Escape raw text so it shows literally inside a paragraph, heading, list item
/// or table cell. Keeps text readable: `_` inside words (`snake_case`), `&` that
/// isn't an entity, `<` that can't start a tag and `|` are left alone (table
/// cells escape `|` themselves). Line-start markers (`#`, `-`, `1.`) are handled
/// by [`MarkdownWriter`].
pub fn escape_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
        let next = chars.get(i + 1).copied();
        let escape = match c {
            '\\' | '`' | '*' | '[' | ']' | '~' => true,
            '_' => {
                !(prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric))
            }
            '<' => next.is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
            '&' => looks_like_entity(&chars[i + 1..]),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `#x41;`, `amp;`, `#65;` (what follows a `&` in an entity).
fn looks_like_entity(rest: &[char]) -> bool {
    let body: Vec<char> = rest
        .iter()
        .copied()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '#')
        .take(32)
        .collect();
    !body.is_empty() && rest.get(body.len()) == Some(&';')
}

/// `` `code` `` with a fence longer than any backtick run inside; padded with a
/// space when the text starts or ends with a backtick. Empty text → "".
pub fn code_span(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    if text.is_empty() {
        return String::new();
    }
    let fence = "`".repeat(longest_run(&text, '`') + 1);
    let pad = text.starts_with('`') || text.ends_with('`') || text.trim().is_empty();
    if pad {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

fn longest_run(text: &str, ch: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == ch { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// A link destination: as is when safe, else `<…>` (spaces, parentheses) with
/// `<`, `>` and line breaks percent-encoded.
pub fn link_destination(url: &str) -> String {
    let url = url.trim();
    let needs_angle = url.is_empty()
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '(' | ')' | '<' | '>'));
    if !needs_angle {
        return url.to_owned();
    }
    let mut out = String::from("<");
    for c in url.chars() {
        match c {
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out.push('>');
    out
}

/// A link target safe to keep: `http`, `https`, `mailto`, `ftp`, or relative
/// (no scheme, not `//host` or `\\host`). `javascript:`, `file:`, `data:`, …
/// are not (also when hidden with case, whitespace or control characters).
pub fn is_safe_link(url: &str) -> bool {
    let url: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    if url.starts_with("//") || url.starts_with('\\') || url.starts_with("/\\") {
        return false;
    }
    let scheme_end = url.find([':', '/', '?', '#']);
    match scheme_end {
        Some(i) if url[i..].starts_with(':') => {
            ["http", "https", "mailto", "ftp"].contains(&&url[..i])
        }
        _ => true,
    }
}

/// `[text](url)`. `text_md` is inline Markdown; empty text shows the URL.
/// An empty or unsafe URL ([`is_safe_link`]) gives just the text.
pub fn link(text_md: &str, url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return text_md.to_owned();
    }
    if !is_safe_link(url) {
        return if text_md.trim().is_empty() {
            escape_text(url)
        } else {
            text_md.to_owned()
        };
    }
    let text = if text_md.trim().is_empty() {
        escape_text(url)
    } else {
        text_md.to_owned()
    };
    format!("[{text}]({})", link_destination(url))
}

/// `![alt](url)`; `alt` is raw text (escaped here, line breaks become spaces).
pub fn image(alt: &str, url: &str) -> String {
    format!(
        "![{}]({})",
        escape_text(&one_line(alt)),
        link_destination(url)
    )
}

/// Line breaks and runs of whitespace → one space, trimmed.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Wrap inline Markdown in `marker`, keeping surrounding whitespace outside
/// (`** a **` is not bold in Markdown). Whitespace-only text is returned as is.
fn wrap(md: &str, marker: &str) -> String {
    let trimmed = md.trim();
    if trimmed.is_empty() {
        return md.to_owned();
    }
    let lead = &md[..md.len() - md.trim_start().len()];
    let trail = &md[md.trim_end().len()..];
    format!("{lead}{marker}{trimmed}{marker}{trail}")
}

/// `**bold**` (inline Markdown in).
pub fn strong(md: &str) -> String {
    wrap(md, "**")
}

/// `*italic*` (inline Markdown in).
pub fn emphasis(md: &str) -> String {
    wrap(md, "*")
}

/// `~~struck~~` (inline Markdown in).
pub fn strikethrough(md: &str) -> String {
    wrap(md, "~~")
}

/// A footnote label made safe: letters, digits, `-` and `_` (others → `-`).
pub fn footnote_label(label: &str) -> String {
    let safe: String = label
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "note".to_owned()
    } else {
        safe
    }
}

/// `[^label]` (GFM footnote reference); pair with [`MarkdownWriter::footnote`].
pub fn footnote_ref(label: &str) -> String {
    format!("[^{}]", footnote_label(label))
}

/// A hard line break inside a paragraph or list item.
pub const HARD_BREAK: &str = "\\\n";

// ---------------------------------------------------------------- Inline builder

/// Character formatting of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Monospace: rendered as a code span (no other escaping inside).
    pub code: bool,
    /// Hyperlink target; consecutive runs with the same link become one link.
    pub link: Option<String>,
}

impl Style {
    pub fn plain() -> Style {
        Style::default()
    }
}

#[derive(Debug, Clone)]
enum Piece {
    Text(String, Style),
    /// Ready-made inline Markdown (an image, a footnote ref, a hard break).
    Md(String),
}

/// Builds one inline Markdown string from styled runs.
///
/// ```
/// use morph_core::markdown::{Inline, Style};
/// let mut line = Inline::new();
/// line.push_text("Total: ", &Style::plain());
/// let bold = Style { bold: true, ..Style::default() };
/// line.push_text("12", &bold);
/// line.push_text(" items", &bold);
/// assert_eq!(line.finish(), "Total: **12 items**");
/// ```
#[derive(Debug, Clone, Default)]
pub struct Inline {
    pieces: Vec<Piece>,
}

impl Inline {
    pub fn new() -> Inline {
        Inline::default()
    }

    /// Add raw text with a style (merged with the previous run if equal).
    pub fn push_text(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        if let Some(Piece::Text(prev, prev_style)) = self.pieces.last_mut()
            && prev_style == style
        {
            prev.push_str(text);
            return;
        }
        self.pieces
            .push(Piece::Text(text.to_owned(), style.clone()));
    }

    /// Add ready-made inline Markdown (e.g. from [`image`] or [`footnote_ref`]).
    pub fn push_md(&mut self, md: &str) {
        if !md.is_empty() {
            self.pieces.push(Piece::Md(md.to_owned()));
        }
    }

    /// A hard line break (dropped at the start and end).
    pub fn push_break(&mut self) {
        self.pieces.push(Piece::Md(HARD_BREAK.to_owned()));
    }

    /// Nothing but whitespace so far.
    pub fn is_blank(&self) -> bool {
        self.pieces.iter().all(|p| match p {
            Piece::Text(t, _) => t.trim().is_empty(),
            Piece::Md(m) => m == HARD_BREAK || m.trim().is_empty(),
        })
    }

    /// The plain text of the runs (no Markdown), e.g. for heading detection.
    pub fn plain_text(&self) -> String {
        self.pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Text(t, _) => Some(t.as_str()),
                Piece::Md(_) => None,
            })
            .collect()
    }

    pub fn finish(self) -> String {
        fn link_of(piece: &Piece) -> Option<&str> {
            match piece {
                Piece::Text(_, s) => s.link.as_deref(),
                Piece::Md(_) => None,
            }
        }
        let mut out = String::new();
        let pieces = self.pieces;
        let mut i = 0;
        while i < pieces.len() {
            // Consecutive runs with the same link become one link.
            let link = link_of(&pieces[i]);
            let mut inner = String::new();
            while i < pieces.len() && link_of(&pieces[i]) == link {
                let prev = inner.chars().last().or_else(|| out.chars().last());
                match &pieces[i] {
                    Piece::Text(text, style) => inner.push_str(&render_run(text, style, prev)),
                    Piece::Md(md) => inner.push_str(md),
                }
                i += 1;
            }
            match link {
                Some(url) => out.push_str(&link_piece(&inner, url)),
                None => out.push_str(&inner),
            }
        }
        // No hard breaks at the edges.
        let mut s = out.as_str();
        loop {
            let t = s.trim_matches(|c: char| c == ' ' || c == '\t');
            let t = t.strip_prefix(HARD_BREAK).unwrap_or(t);
            let t = t.strip_suffix(HARD_BREAK).unwrap_or(t);
            if t.len() == s.len() {
                break;
            }
            s = t;
        }
        s.to_owned()
    }
}

/// A link around `inner`, keeping edge whitespace outside the brackets.
fn link_piece(inner: &str, url: &str) -> String {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return link(inner, url);
    }
    let lead = &inner[..inner.len() - inner.trim_start().len()];
    let trail = &inner[inner.trim_end().len()..];
    format!("{lead}{}{trail}", link(trimmed, url))
}

fn render_run(text: &str, style: &Style, prev: Option<char>) -> String {
    let mut md = if style.code {
        let lead = &text[..text.len() - text.trim_start().len()];
        let trail = &text[text.trim_end().len()..];
        let core = code_span(text.trim());
        format!("{lead}{core}{trail}")
    } else {
        escape_text(text)
    };
    if text.trim().is_empty() {
        return md;
    }
    if style.strike {
        md = strikethrough(&md);
    }
    if style.italic {
        // `**a***b*` is ambiguous; use `_` right after a `*`.
        let marker = if prev == Some('*') { "_" } else { "*" };
        md = wrap(&md, marker);
    }
    if style.bold {
        md = strong(&md);
    }
    md
}

// ---------------------------------------------------------------- blocks

/// Kind of list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// `- item`
    Bullet,
    /// `n. item`
    Ordered(u32),
}

/// Column alignment for [`MarkdownWriter::table_aligned`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Default,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    Nothing,
    Block,
    ListItem,
}

/// Columns wider than this are not padded (keeps big sheets small).
const MAX_PAD_WIDTH: usize = 40;

/// Builds a Markdown document block by block. See the module docs.
#[derive(Debug, Clone)]
pub struct MarkdownWriter {
    out: String,
    last: Last,
    /// Content column of the last list item at each depth.
    list_indents: Vec<usize>,
    footnotes: Vec<String>,
}

impl Default for MarkdownWriter {
    fn default() -> Self {
        MarkdownWriter::new()
    }
}

impl MarkdownWriter {
    pub fn new() -> MarkdownWriter {
        MarkdownWriter {
            out: String::new(),
            last: Last::Nothing,
            list_indents: Vec::new(),
            footnotes: Vec::new(),
        }
    }

    /// Nothing written yet.
    pub fn is_empty(&self) -> bool {
        self.out.trim().is_empty() && self.footnotes.is_empty()
    }

    fn start_block(&mut self) {
        if self.last != Last::Nothing {
            self.out.push_str("\n\n");
        }
        self.last = Last::Block;
        self.list_indents.clear();
    }

    /// `# Heading` (level clamped to 1–6). Line breaks become spaces; a
    /// trailing `#` stays visible. Empty text writes nothing.
    pub fn heading(&mut self, level: u8, md: &str) {
        let text = md.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return;
        }
        let mut text = protect_line_start(&text);
        // `Title #` would lose its `#` (closing sequence).
        let hashes = text.len() - text.trim_end_matches('#').len();
        if hashes > 0 {
            let at = text.len() - hashes;
            let before = text[..at].chars().last();
            if before.is_none_or(|c| c == ' ') {
                text.insert(at, '\\');
            }
        }
        self.start_block();
        let level = level.clamp(1, 6) as usize;
        let _ = write!(self.out, "{} {text}", "#".repeat(level));
    }

    /// A paragraph. Lines (`\n`) are kept as soft line breaks; blank lines are
    /// dropped (they would split the paragraph).
    pub fn paragraph(&mut self, md: &str) {
        let lines = block_lines(md);
        if lines.is_empty() {
            return;
        }
        self.start_block();
        self.out.push_str(&lines.join("\n"));
    }

    /// A list item at `depth` (0 = top level). Depth can't skip levels (a jump
    /// from 0 to 2 is written as 1). Consecutive items make one tight list.
    /// The first item of a nested ordered list is written as `1.` (CommonMark
    /// only lets a list starting at 1 interrupt the parent's text).
    pub fn list_item(&mut self, depth: usize, kind: ListKind, md: &str) {
        let lines = block_lines(md);
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        if self.last == Last::ListItem {
            self.out.push('\n');
        } else {
            if self.last != Last::Nothing {
                self.out.push_str("\n\n");
            }
            self.list_indents.clear();
        }
        self.last = Last::ListItem;
        let depth = depth.min(self.list_indents.len());
        let indent = match depth {
            0 => 0,
            d => self.list_indents[d - 1],
        };
        // A nested ordered list must start at 1, or CommonMark reads it as
        // text of the parent item ("can't interrupt a paragraph").
        let opens_level = depth > 0 && depth == self.list_indents.len();
        let marker = match kind {
            ListKind::Bullet => "-".to_owned(),
            ListKind::Ordered(_) if opens_level => "1.".to_owned(),
            ListKind::Ordered(n) => format!("{n}."),
        };
        let content_col = indent + marker.chars().count() + 1;
        self.list_indents.truncate(depth);
        self.list_indents.push(content_col);
        let pad = " ".repeat(indent);
        let cont = " ".repeat(content_col);
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                let _ = write!(self.out, "{pad}{marker} {line}");
            } else {
                let _ = write!(self.out, "\n{cont}{line}");
            }
        }
        // `- ` with nothing after it must not leave trailing spaces.
        let trimmed = self.out.trim_end_matches(' ').len();
        self.out.truncate(trimmed);
    }

    /// End the current list (the next item starts a new list after a blank line).
    pub fn end_list(&mut self) {
        if self.last == Last::ListItem {
            self.last = Last::Block;
            self.list_indents.clear();
        }
    }

    /// A GFM pipe table; the first row is the header. Cells are inline
    /// Markdown: `|` is escaped and line breaks become `<br>` here. Short rows
    /// are padded; an empty table writes nothing.
    pub fn table(&mut self, rows: &[Vec<String>]) {
        self.table_aligned(rows, &[]);
    }

    /// [`MarkdownWriter::table`] with column alignments (missing = default).
    pub fn table_aligned(&mut self, rows: &[Vec<String>], aligns: &[Align]) {
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                (0..cols)
                    .map(|c| row.get(c).map(|s| table_cell(s)).unwrap_or_default())
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = (0..cols)
            .map(|c| {
                let w = cells
                    .iter()
                    .map(|r| r[c].chars().count())
                    .max()
                    .unwrap_or(0);
                if w > MAX_PAD_WIDTH { 3 } else { w.max(3) }
            })
            .collect();
        self.start_block();
        let mut lines: Vec<String> = Vec::with_capacity(cells.len() + 1);
        for (i, row) in cells.iter().enumerate() {
            lines.push(table_row(row, &widths));
            if i == 0 {
                let delim: Vec<String> = (0..cols)
                    .map(|c| {
                        let w = widths[c];
                        match aligns.get(c).copied().unwrap_or_default() {
                            Align::Default => "-".repeat(w),
                            Align::Left => format!(":{}", "-".repeat(w - 1)),
                            Align::Right => format!("{}:", "-".repeat(w - 1)),
                            Align::Center => format!(":{}:", "-".repeat(w - 2)),
                        }
                    })
                    .collect();
                lines.push(format!("| {} |", delim.join(" | ")));
            }
        }
        self.out.push_str(&lines.join("\n"));
    }

    /// A fenced code block (fence longer than any backtick run inside).
    /// `lang` keeps only letters, digits, `+`, `-`, `_`, `#`, `.`.
    pub fn code_block(&mut self, lang: &str, code: &str) {
        let lang: String = lang
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '#' | '.'))
            .collect();
        let fence = "`".repeat((longest_run(code, '`') + 1).max(3));
        let code = code.replace("\r\n", "\n");
        let code = code.trim_end_matches('\n');
        self.start_block();
        let _ = write!(self.out, "{fence}{lang}\n{code}\n{fence}");
    }

    /// A block quote (`> `), inline Markdown, lines kept.
    pub fn quote(&mut self, md: &str) {
        let lines = block_lines(md);
        if lines.is_empty() {
            return;
        }
        self.start_block();
        let quoted: Vec<String> = lines.iter().map(|l| format!("> {l}")).collect();
        self.out.push_str(&quoted.join("\n"));
    }

    /// A thematic break (`---`).
    pub fn rule(&mut self) {
        self.start_block();
        self.out.push_str("---");
    }

    /// Ready-made Markdown block(s), written as they are (trimmed).
    pub fn raw_block(&mut self, md: &str) {
        let md = md.trim_matches('\n').trim_end();
        if md.trim().is_empty() {
            return;
        }
        self.start_block();
        self.out.push_str(md);
    }

    /// A footnote definition (`[^label]: text`), written at the end of the
    /// document in the order added. Use [`footnote_ref`] in the text.
    pub fn footnote(&mut self, label: &str, md: &str) {
        let lines = block_lines(md);
        let mut def = format!("[^{}]:", footnote_label(label));
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                let _ = write!(def, " {line}");
            } else {
                let _ = write!(def, "\n    {line}");
            }
        }
        self.footnotes.push(def);
    }

    /// The document: blocks, then footnotes; ends with exactly one `\n`
    /// (empty string for an empty document).
    pub fn finish(mut self) -> String {
        let footnotes = std::mem::take(&mut self.footnotes);
        if !footnotes.is_empty() {
            self.start_block();
            self.out.push_str(&footnotes.join("\n"));
        }
        let mut out = self.out;
        out.truncate(out.trim_end().len());
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

/// Lines of a block: trimmed, blank ones dropped, line starts protected.
fn block_lines(md: &str) -> Vec<String> {
    md.replace("\r\n", "\n")
        .split(['\n', '\r'])
        .map(|l| l.trim_matches(|c: char| c == ' ' || c == '\t'))
        .filter(|l| !l.is_empty())
        .map(protect_line_start)
        .collect()
}

/// Escape what would make a line a heading, list item, quote, thematic break,
/// setext underline or code block. Leading whitespace is dropped.
pub fn protect_line_start(line: &str) -> String {
    let line = line.trim_start_matches([' ', '\t']);
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let second = chars.next();
    let space_or_end = |c: Option<char>| c.is_none_or(|c| c == ' ' || c == '\t');
    let escape_first = match first {
        '#' => {
            let hashes = line.chars().take_while(|&c| c == '#').count();
            hashes <= 6 && space_or_end(line.chars().nth(hashes))
        }
        '>' => true,
        '+' => space_or_end(second),
        '-' | '*' | '_' | '=' => space_or_end(second) || is_rule_or_underline(line),
        _ => false,
    };
    if escape_first {
        return format!("\\{line}");
    }
    // `1. ` / `1) ` ordered list markers.
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let after: Vec<char> = line.chars().skip(digits).take(2).collect();
        if matches!(after.first(), Some('.' | ')')) && space_or_end(after.get(1).copied()) {
            let mut s = line.to_owned();
            s.insert(digits, '\\');
            return s;
        }
    }
    line.to_owned()
}

/// `---`, `* * *`, `___` (thematic breaks) or `===` / `--` (setext underlines).
fn is_rule_or_underline(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(first) = compact.chars().next() else {
        return false;
    };
    if !compact.chars().all(|c| c == first) {
        return false;
    }
    match first {
        '=' => true,
        '-' => true,
        '*' | '_' => compact.len() >= 3,
        _ => false,
    }
}

/// A cell's inline Markdown made table-safe: unescaped `|` → `\|`, line
/// breaks → `<br>`, trimmed.
pub fn table_cell(md: &str) -> String {
    let md = md.trim();
    let mut out = String::with_capacity(md.len());
    let mut backslashes = 0usize;
    let mut chars = md.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '|' if backslashes.is_multiple_of(2) => out.push_str("\\|"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                trim_trailing_break(&mut out);
                out.push_str("<br>");
            }
            '\n' => {
                trim_trailing_break(&mut out);
                out.push_str("<br>");
            }
            c => out.push(c),
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
    }
    out
}

/// Before a `<br>`: drop spaces and a hard-break backslash.
fn trim_trailing_break(out: &mut String) {
    let len = out.trim_end_matches(' ').len();
    out.truncate(len);
    let backslashes = out.len() - out.trim_end_matches('\\').len();
    if backslashes % 2 == 1 {
        out.pop();
    }
}

fn table_row(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .zip(widths)
        .map(|(cell, &w)| {
            let n = cell.chars().count();
            if n < w {
                format!("{cell}{}", " ".repeat(w - n))
            } else {
                cell.clone()
            }
        })
        .collect();
    format!("| {} |", padded.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render with pulldown-cmark (GFM tables, strikethrough, footnotes) to check
    /// what a viewer would show.
    fn html(md: &str) -> String {
        use pulldown_cmark::{Options, Parser};
        let mut out = String::new();
        let opts =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES;
        pulldown_cmark::html::push_html(&mut out, Parser::new_ext(md, opts));
        out
    }

    #[test]
    fn escaped_text_renders_literally() {
        let raw = r"a*b* _x_ snake_case [link](u) `code` \ ~~s~~ <b>tag</b> &amp; & a<b 5 < 6";
        let md = escape_text(raw);
        assert!(md.contains("snake_case"), "intraword _ stays: {md}");
        assert!(md.contains("& a<b") || md.contains("&amp;"), "{md}");
        let out = html(&md);
        assert!(!out.contains("<em>") && !out.contains("<strong>"), "{out}");
        assert!(!out.contains("<a ") && !out.contains("<code>"), "{out}");
        assert!(!out.contains("<del>") && !out.contains("<b>"), "{out}");
        assert!(out.contains("&amp;amp;"), "entity shown literally: {out}");
        assert!(out.contains("5 &lt; 6"), "{out}");
    }

    #[test]
    fn line_starts_are_protected() {
        for (raw, bad) in [
            ("# not a heading", "<h1>"),
            ("- not a list", "<ul>"),
            ("+ not a list", "<ul>"),
            ("1. not a list", "<ol>"),
            ("2) not a list", "<ol>"),
            ("> not a quote", "<blockquote>"),
            ("---", "<hr"),
            ("***", "<hr"),
            ("    indented", "<pre>"),
        ] {
            let mut w = MarkdownWriter::new();
            w.paragraph(&escape_text(raw));
            let out = html(&w.finish());
            assert!(!out.contains(bad), "{raw:?} → {out}");
        }
        // Setext underline on the second line.
        let mut w = MarkdownWriter::new();
        w.paragraph("Title\n===\nmore\n--");
        let out = html(&w.finish());
        assert!(!out.contains("<h1>") && !out.contains("<h2>"), "{out}");
        assert_eq!(protect_line_start("#hashtag"), "#hashtag");
        assert_eq!(protect_line_start("2026. Year"), "2026\\. Year");
        assert_eq!(protect_line_start("3.5 m"), "3.5 m");
        assert_eq!(protect_line_start("-5 °C"), "-5 °C");
    }

    #[test]
    fn headings() {
        let mut w = MarkdownWriter::new();
        w.heading(1, "Title");
        w.heading(9, "Deep\nheading");
        w.heading(2, "C#");
        w.heading(2, "Issue #");
        w.heading(3, "   ");
        let md = w.finish();
        assert_eq!(
            md,
            "# Title\n\n###### Deep heading\n\n## C#\n\n## Issue \\#\n"
        );
        let out = html(&md);
        assert!(
            out.contains("<h2>C#</h2>") && out.contains("<h2>Issue #</h2>"),
            "{out}"
        );
    }

    #[test]
    fn nested_lists_render_nested() {
        let mut w = MarkdownWriter::new();
        w.paragraph("Intro");
        w.list_item(0, ListKind::Bullet, "one");
        w.list_item(1, ListKind::Bullet, "one.a");
        w.list_item(2, ListKind::Ordered(1), "deep");
        w.list_item(0, ListKind::Bullet, "two");
        w.list_item(3, ListKind::Bullet, "jump is clamped");
        w.end_list();
        w.list_item(0, ListKind::Ordered(9), "nine");
        w.list_item(1, ListKind::Ordered(10), "ten\nsecond line");
        w.list_item(0, ListKind::Ordered(11), "- dash text");
        let md = w.finish();
        assert_eq!(
            md,
            "Intro\n\n- one\n  - one.a\n    1. deep\n- two\n  - jump is clamped\n\n\
             9. nine\n   1. ten\n      second line\n11. \\- dash text\n"
        );
        let out = html(&md);
        assert_eq!(out.matches("<ul>").count(), 3, "{out}");
        assert_eq!(out.matches("<ol").count(), 3, "{out}");
        assert!(!out.contains("<p>one"), "tight list: {out}");
    }

    #[test]
    fn tables_escape_pipes_and_newlines() {
        let mut w = MarkdownWriter::new();
        w.table(&[
            vec!["Name".into(), "Value".into()],
            vec![escape_text("a|b"), "line 1\nline 2".into()],
            vec![r"already \| escaped".into()],
            vec![r"backslash \\| pipe".into(), "x".into()],
        ]);
        let md = w.finish();
        assert_eq!(
            md,
            "| Name                | Value            |\n\
             | ------------------- | ---------------- |\n\
             | a\\|b                | line 1<br>line 2 |\n\
             | already \\| escaped  |                  |\n\
             | backslash \\\\\\| pipe | x                |\n"
        );
        let out = html(&md);
        assert_eq!(out.matches("<tr>").count(), 4, "{out}");
        assert_eq!(out.matches("<td>").count(), 6, "{out}");
        assert!(out.contains("a|b"), "{out}");
    }

    #[test]
    fn table_alignment_and_wide_columns() {
        let mut w = MarkdownWriter::new();
        let long = "x".repeat(50);
        w.table_aligned(
            &[
                vec!["A".into(), "B".into(), "C".into()],
                vec!["1".into(), long.clone(), "c".into()],
            ],
            &[Align::Right, Align::Default, Align::Center],
        );
        w.table(&[]);
        w.table(&[vec![], vec![]]);
        let md = w.finish();
        assert_eq!(
            md,
            format!("| A   | B   | C   |\n| --: | --- | :-: |\n| 1   | {long} | c   |\n")
        );
    }

    #[test]
    fn inline_builder_merges_and_nests() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let italic = Style {
            italic: true,
            ..Style::default()
        };
        let linked = Style {
            link: Some("https://example.com/a b".into()),
            ..Style::default()
        };
        let linked_bold = Style {
            bold: true,
            ..linked.clone()
        };
        let mut l = Inline::new();
        l.push_text("Hello ", &Style::plain());
        l.push_text("bold ", &bold);
        l.push_text("world", &bold);
        l.push_text("it", &italic);
        l.push_text(" see ", &Style::plain());
        l.push_text("the ", &linked);
        l.push_text("site", &linked_bold);
        l.push_text(" ", &linked);
        l.push_md(&footnote_ref("1"));
        l.push_break();
        let md = l.finish();
        assert_eq!(
            md,
            "Hello **bold world**_it_ see [the **site**](<https://example.com/a b>) [^1]"
        );
        let out = html(&format!("{md}\n\n[^1]: note"));
        assert!(
            out.contains("<strong>bold world</strong><em>it</em>"),
            "{out}"
        );
        assert!(out.contains("href=\"https://example.com/a%20b\""), "{out}");
    }

    #[test]
    fn inline_code_and_whitespace_edges() {
        let code = Style {
            code: true,
            ..Style::default()
        };
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let mut l = Inline::new();
        l.push_text("run ", &Style::plain());
        l.push_text(" a`b ", &code);
        l.push_text("  ", &bold);
        l.push_text("*x*", &Style::plain());
        assert_eq!(l.finish(), "run  ``a`b``   \\*x\\*");
        assert_eq!(code_span("`x"), "`` `x ``");
        assert_eq!(code_span(""), "");
        assert!(Inline::new().is_blank());
    }

    #[test]
    fn links_and_images() {
        assert_eq!(link("t", "https://e.com/x"), "[t](https://e.com/x)");
        assert_eq!(
            link("", "https://e.com/x_y"),
            "[https://e.com/x_y](https://e.com/x_y)"
        );
        assert_eq!(
            link("", "https://e.com/*"),
            "[https://e.com/\\*](https://e.com/*)"
        );
        assert_eq!(link("t", " "), "t");
        assert_eq!(link("t", "a(b)"), "[t](<a(b)>)");
        // Unsafe targets keep only the text.
        for bad in [
            "javascript:alert(1)",
            " JavaScript\t:x",
            "vbscript:x",
            "file:///C:/secret.txt",
            "data:text/html,x",
            "C:\\x.txt",
            "\\\\host\\share",
            "//host/x",
        ] {
            assert_eq!(link("t", bad), "t", "{bad}");
        }
        assert_eq!(link("", "javascript:x"), "javascript:x");
        for good in [
            "mailto:a@b.c",
            "ftp://h/x",
            "HTTPS://e.com",
            "a/b.md",
            "#top",
            "/x?y=z:1",
        ] {
            assert!(is_safe_link(good), "{good}");
        }
        assert_eq!(link_destination("a<b>"), "<a%3Cb%3E>");
        assert_eq!(
            image("A [pic]\nhere", "x%20(images)/image-1.png"),
            "![A \\[pic\\] here](<x%20(images)/image-1.png>)"
        );
        let out = html(&image("alt", "a b/c.png"));
        assert!(out.contains("src=\"a%20b/c.png\""), "{out}");
    }

    #[test]
    fn code_quote_rule_raw_and_footnotes() {
        let mut w = MarkdownWriter::new();
        w.code_block("rust; rm", "let a = \"```\";\n");
        w.quote("# quoted\nline");
        w.rule();
        w.raw_block("\n\n| raw |\n");
        w.footnote("1", "First note.");
        w.paragraph(&format!("Text{}", footnote_ref("1")));
        w.footnote("a b", "Two\nlines");
        let md = w.finish();
        assert_eq!(
            md,
            "````rustrm\nlet a = \"```\";\n````\n\n> \\# quoted\n> line\n\n---\n\n| raw |\n\n\
             Text[^1]\n\n[^1]: First note.\n[^a-b]: Two\n    lines\n"
        );
        let out = html(&md);
        assert!(
            out.contains("<pre><code class=\"language-rustrm\">"),
            "{out}"
        );
        assert!(out.contains("footnote"), "{out}");
    }

    #[test]
    fn empty_document_is_empty() {
        let mut w = MarkdownWriter::new();
        assert!(w.is_empty());
        w.paragraph(" \n ");
        w.heading(1, "");
        assert_eq!(w.finish(), "");
    }

    #[test]
    fn cells_with_hard_breaks() {
        assert_eq!(table_cell("a\\\nb"), "a<br>b");
        assert_eq!(table_cell("a  \r\nb"), "a<br>b");
        assert_eq!(table_cell(" x|y "), "x\\|y");
    }
}
