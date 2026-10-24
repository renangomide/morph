// SPDX-License-Identifier: GPL-3.0-or-later
//! DOCX → Markdown, native OOXML reader (SPEC §4.6).
//! Reached through `office::plan` → `Plan::DocxToMarkdown` (CPU queue, no engine).
//! doc/odt/rtf keep going through the engine ([`super::to_markdown`]).
//!
//! What it reads (zip + `roxmltree`, elements matched by local name so Strict
//! OOXML works too):
//! - Headings: paragraph `w:outlineLvl`, else the style chain (`w:basedOn`):
//!   `w:outlineLvl`, or a style named `heading N`. Level = outline level + 1
//!   (max 6). The built-in **Title** style is `#`, like Heading 1.
//! - Lists: `numPr` (paragraph or style) → `numbering.xml`: `numFmt` `bullet`
//!   → `-`, any other format → ordered (numbered from `w:start`, overrides
//!   honoured), `none` → plain paragraph; nesting from `ilvl`.
//! - Runs: bold, italic, strike, monospace fonts → code (paragraph style →
//!   character style → direct formatting); hidden text dropped. A paragraph
//!   that is all monospace joins a fenced code block with its neighbours.
//! - Links: `w:hyperlink` with an external `r:id`, and `HYPERLINK` fields;
//!   internal anchors keep just the text. Fields show their result.
//! - Tables → pipe tables (first row = header); merged cells: the spanned or
//!   continued cells are left empty; nested tables are flattened to text.
//! - Footnotes and endnotes → `[^n]` (one numbering, in reference order).
//! - Pictures (`a:blip`, VML `v:imagedata`) → `name (images)/` with the
//!   `wp:docPr/@descr` alt text; pictures off → the alt text only. External
//!   pictures are never fetched. Text boxes → their text.
//! - Tracked changes: deletions dropped, insertions kept. Headers, footers and
//!   comments are skipped.

use super::ooxml::{Package, Rel, attr, check_depth, child, parse};
use super::options::OfficeConvertOpts;
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::markdown::{
    self, ImageFolder, Inline, ListKind, MarkdownWriter, Style, footnote_ref, one_line,
};
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::path::Path;

/// Style `basedOn` chains longer than this are cut (cycles).
const MAX_CHAIN: usize = 32;

pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let mut pkg = Package::open(input)?;

    // Main document part and its relationships (a broken package `.rels`
    // falls back to the usual name).
    let main = pkg
        .rels("")
        .ok()
        .and_then(|rels| {
            rels.into_values()
                .find(|r| r.kind.ends_with("/officeDocument") && !r.external)
                .map(|r| r.target)
        })
        .unwrap_or_else(|| "word/document.xml".to_owned());
    let dir = match main.rsplit_once('/') {
        Some((d, _)) => format!("{d}/"),
        None => String::new(),
    };
    let document_xml = pkg
        .xml(&main)?
        .ok_or_else(|| super::damaged_document(format!("{main} is missing")))?;
    let rels = pkg.rels(&main)?;
    let part = |suffix: &str, default: &str| {
        rels.values()
            .find(|r| r.kind.ends_with(suffix) && !r.external)
            .map(|r| r.target.clone())
            .unwrap_or_else(|| format!("{dir}{default}"))
    };
    let styles_xml = pkg.xml(&part("/styles", "styles.xml"))?;
    let numbering_xml = pkg.xml(&part("/numbering", "numbering.xml"))?;
    let footnotes_xml = pkg.xml(&part("/footnotes", "footnotes.xml"))?;
    let endnotes_xml = pkg.xml(&part("/endnotes", "endnotes.xml"))?;

    let document = parse(&document_xml)?;
    let styles = match &styles_xml {
        Some(xml) => Styles::read(&parse(xml)?),
        None => Styles::default(),
    };
    let numbering = match &numbering_xml {
        Some(xml) => Numbering::read(&parse(xml)?),
        None => Numbering::default(),
    };
    let footnotes = footnotes_xml.as_deref().map(parse).transpose()?;
    let endnotes = endnotes_xml.as_deref().map(parse).transpose()?;
    let mut notes = HashMap::new();
    for (endnote, doc) in [(false, &footnotes), (true, &endnotes)] {
        if let Some(doc) = doc {
            for n in doc.root_element().children().filter(|n| n.is_element()) {
                if let Some(id) = attr(n, "id") {
                    notes.insert((endnote, id.to_owned()), n);
                }
            }
        }
    }

    let folder = task.output.folder_for(input);
    let mut reader = Reader {
        pkg,
        rels,
        styles,
        numbering,
        notes,
        note_labels: HashMap::new(),
        note_queue: Vec::new(),
        images: ImageFolder::new(&folder, input, opts.images),
        counters: HashMap::new(),
        fields: Vec::new(),
        writer: MarkdownWriter::new(),
        code: Vec::new(),
    };

    let body = document
        .root_element()
        .children()
        .find(|n| n.has_tag_name_local("body"))
        .ok_or_else(|| super::damaged_document(format!("{main}: no document body")))?;
    let blocks: Vec<Node> = body.children().filter(|n| n.is_element()).collect();
    for (i, block) in blocks.iter().enumerate() {
        ctx.check_cancel()?;
        reader.block(*block)?;
        ctx.progress(0.9 * (i + 1) as f32 / blocks.len() as f32, None);
    }
    reader.flush_code();
    reader.write_notes()?;
    ctx.check_cancel()?;

    let Reader { writer, images, .. } = reader;
    let out = markdown::write_markdown(&folder, input, &writer.finish(), images)?;
    ctx.progress(1.0, None);
    Ok(out)
}

// ---------------------------------------------------------------- XML helpers

trait LocalName {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl LocalName for Node<'_, '_> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}

fn val<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|n| attr(n, "val"))
}

/// A toggle property (`<w:b/>`, `<w:b w:val="0"/>`).
fn toggle(rpr: Node<'_, '_>, name: &str) -> Option<bool> {
    child(rpr, name).map(|n| !matches!(attr(n, "val"), Some("0" | "false" | "off" | "none")))
}

// ---------------------------------------------------------------- styles

#[derive(Debug, Clone, Copy, Default)]
struct RunProps {
    bold: Option<bool>,
    italic: Option<bool>,
    strike: Option<bool>,
    code: Option<bool>,
    hidden: Option<bool>,
}

impl RunProps {
    fn read(rpr: Option<Node<'_, '_>>) -> RunProps {
        let Some(rpr) = rpr else {
            return RunProps::default();
        };
        let code = child(rpr, "rFonts").and_then(|f| {
            attr(f, "ascii")
                .or_else(|| attr(f, "hAnsi"))
                .map(is_monospace)
        });
        RunProps {
            bold: toggle(rpr, "b"),
            italic: toggle(rpr, "i"),
            strike: toggle(rpr, "strike").or_else(|| toggle(rpr, "dstrike")),
            code,
            hidden: toggle(rpr, "vanish"),
        }
    }

    /// `self` wins where set.
    fn over(self, base: RunProps) -> RunProps {
        RunProps {
            bold: self.bold.or(base.bold),
            italic: self.italic.or(base.italic),
            strike: self.strike.or(base.strike),
            code: self.code.or(base.code),
            hidden: self.hidden.or(base.hidden),
        }
    }
}

fn is_monospace(font: &str) -> bool {
    let f = font.to_ascii_lowercase();
    [
        "mono",
        "courier",
        "consolas",
        "lucida console",
        "menlo",
        "monaco",
        "inconsolata",
    ]
    .iter()
    .any(|m| f.contains(m))
        || f.ends_with(" code")
}

#[derive(Debug, Clone, Default)]
struct StyleDef {
    based_on: Option<String>,
    name: String,
    outline: Option<u8>,
    num: Option<(String, Option<u8>)>,
    run: RunProps,
}

#[derive(Debug, Default)]
struct Styles {
    by_id: HashMap<String, StyleDef>,
    default_para: Option<String>,
}

/// Heading level from a style or paragraph: 1–6, or `None` for body text.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Outline {
    Level(u8),
    Body,
}

impl Styles {
    fn read(doc: &Document<'_>) -> Styles {
        let mut out = Styles::default();
        for s in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name_local("style"))
        {
            let Some(id) = attr(s, "styleId") else {
                continue;
            };
            let ppr = child(s, "pPr");
            if attr(s, "type") == Some("paragraph") && attr(s, "default") == Some("1") {
                out.default_para = Some(id.to_owned());
            }
            out.by_id.insert(
                id.to_owned(),
                StyleDef {
                    based_on: val(s, "basedOn").map(str::to_owned),
                    name: val(s, "name").unwrap_or_default().to_ascii_lowercase(),
                    outline: ppr.and_then(|p| val(p, "outlineLvl")?.parse().ok()),
                    num: ppr.and_then(|p| num_pr(child(p, "numPr")?)),
                    run: RunProps::read(child(s, "rPr")),
                },
            );
        }
        out
    }

    /// The style and its `basedOn` ancestors, nearest first.
    fn chain(&self, id: Option<&str>) -> Vec<&StyleDef> {
        let mut out = Vec::new();
        let mut next = id;
        while let Some(id) = next {
            let Some(s) = self.by_id.get(id) else { break };
            if out.len() >= MAX_CHAIN {
                break;
            }
            out.push(s);
            next = s.based_on.as_deref();
        }
        out
    }

    fn outline(&self, id: Option<&str>) -> Option<Outline> {
        for s in self.chain(id) {
            if let Some(lvl) = s.outline {
                return Some(outline_level(lvl));
            }
            if let Some(n) = s.name.strip_prefix("heading ")
                && let Ok(n) = n.trim().parse::<u8>()
                && (1..=9).contains(&n)
            {
                return Some(outline_level(n - 1));
            }
            if s.name == "title" {
                return Some(Outline::Level(1));
            }
        }
        None
    }

    fn num(&self, id: Option<&str>) -> Option<(String, Option<u8>)> {
        self.chain(id).into_iter().find_map(|s| s.num.clone())
    }

    fn run(&self, id: Option<&str>) -> RunProps {
        self.chain(id)
            .into_iter()
            .fold(RunProps::default(), |acc, s| acc.over(s.run))
    }
}

fn outline_level(lvl: u8) -> Outline {
    if lvl >= 9 {
        Outline::Body
    } else {
        Outline::Level((lvl + 1).min(6))
    }
}

fn num_pr(n: Node<'_, '_>) -> Option<(String, Option<u8>)> {
    let id = val(n, "numId")?.to_owned();
    Some((id, val(n, "ilvl").and_then(|v| v.parse().ok())))
}

// ---------------------------------------------------------------- numbering

#[derive(Debug, Clone, Copy)]
struct Level {
    /// `None`: no number shown (`numFmt none`).
    ordered: Option<bool>,
    start: u32,
}

#[derive(Debug, Default)]
struct Numbering {
    /// numId → (abstractNumId, per-level overrides).
    nums: HashMap<String, (String, HashMap<u8, Level>)>,
    abstracts: HashMap<String, HashMap<u8, Level>>,
}

fn read_level(lvl: Node<'_, '_>, base: Option<Level>) -> Level {
    let ordered = match val(lvl, "numFmt") {
        Some("bullet") => Some(false),
        Some("none") => None,
        Some(_) => Some(true),
        None => base.map_or(Some(false), |b| b.ordered),
    };
    let start = val(lvl, "start")
        .and_then(|v| v.parse().ok())
        .or(base.map(|b| b.start))
        .unwrap_or(1);
    Level { ordered, start }
}

impl Numbering {
    fn read(doc: &Document<'_>) -> Numbering {
        let mut out = Numbering::default();
        let root = doc.root_element();
        for a in root
            .children()
            .filter(|n| n.has_tag_name_local("abstractNum"))
        {
            let Some(id) = attr(a, "abstractNumId") else {
                continue;
            };
            let levels = a
                .children()
                .filter(|n| n.has_tag_name_local("lvl"))
                .filter_map(|l| Some((attr(l, "ilvl")?.parse().ok()?, read_level(l, None))))
                .collect();
            out.abstracts.insert(id.to_owned(), levels);
        }
        for n in root.children().filter(|n| n.has_tag_name_local("num")) {
            let (Some(id), Some(abs)) = (attr(n, "numId"), val(n, "abstractNumId")) else {
                continue;
            };
            let mut overrides = HashMap::new();
            for o in n.children().filter(|c| c.has_tag_name_local("lvlOverride")) {
                let Some(ilvl) = attr(o, "ilvl").and_then(|v| v.parse::<u8>().ok()) else {
                    continue;
                };
                let base = out.abstracts.get(abs).and_then(|l| l.get(&ilvl)).copied();
                let mut level = match child(o, "lvl") {
                    Some(l) => read_level(l, base),
                    None => base.unwrap_or(Level {
                        ordered: Some(false),
                        start: 1,
                    }),
                };
                if let Some(s) = val(o, "startOverride").and_then(|v| v.parse().ok()) {
                    level.start = s;
                }
                overrides.insert(ilvl, level);
            }
            out.nums.insert(id.to_owned(), (abs.to_owned(), overrides));
        }
        out
    }

    fn level(&self, num_id: &str, ilvl: u8) -> Option<Level> {
        let (abs, overrides) = self.nums.get(num_id)?;
        overrides
            .get(&ilvl)
            .or_else(|| self.abstracts.get(abs)?.get(&ilvl))
            .copied()
    }
}

// ---------------------------------------------------------------- reader

/// One paragraph's inline content.
#[derive(Default)]
struct Line {
    inline: Inline,
    /// Plain text (for code blocks).
    raw: String,
    /// Every visible run so far is monospace, and nothing else is there.
    code: bool,
}

impl Line {
    fn new() -> Line {
        Line {
            code: true,
            ..Line::default()
        }
    }

    fn text(&mut self, text: &str, style: &Style) {
        if !text.trim().is_empty() && (!style.code || style.link.is_some()) {
            self.code = false;
        }
        self.raw.push_str(text);
        self.inline.push_text(text, style);
    }

    fn md(&mut self, md: &str) {
        if !md.is_empty() {
            self.code = false;
            self.inline.push_md(md);
        }
    }

    fn brk(&mut self) {
        self.raw.push('\n');
        self.inline.push_break();
    }

    fn is_code(&self) -> bool {
        self.code && !self.raw.trim().is_empty()
    }
}

/// An open complex field (`fldChar begin … separate … end`).
struct Field {
    instr: String,
    in_result: bool,
    link: Option<String>,
}

struct Reader<'a, 'i> {
    pkg: Package,
    rels: HashMap<String, Rel>,
    styles: Styles,
    numbering: Numbering,
    /// (is endnote, id) → note element.
    notes: HashMap<(bool, String), Node<'a, 'i>>,
    note_labels: HashMap<(bool, String), String>,
    note_queue: Vec<(String, Node<'a, 'i>)>,
    images: ImageFolder,
    /// numId → counter per level.
    counters: HashMap<String, Vec<u32>>,
    fields: Vec<Field>,
    writer: MarkdownWriter,
    /// Pending code block lines.
    code: Vec<String>,
}

/// Run-level context passed down the inline walk.
#[derive(Clone, Copy)]
struct RunCtx<'s> {
    base: RunProps,
    link: Option<&'s str>,
    /// Inside a heading: no bold/italic markers.
    heading: bool,
}

impl<'a, 'i> Reader<'a, 'i> {
    fn flush_code(&mut self) {
        if !self.code.is_empty() {
            let code = std::mem::take(&mut self.code).join("\n");
            self.writer.code_block("", &code);
        }
    }

    // ------------------------------------------------------------ blocks

    fn block(&mut self, node: Node<'a, 'i>) -> Result<(), ConvertError> {
        check_depth(node)?;
        match node.tag_name().name() {
            "p" => self.paragraph(node)?,
            "tbl" => {
                self.flush_code();
                let rows = self.table(node)?;
                if rows.iter().flatten().any(|c| !c.trim().is_empty()) {
                    self.writer.table(&rows);
                }
            }
            "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" | "smartTag" => {
                for c in node.children().filter(|n| n.is_element()) {
                    self.block(c)?;
                }
            }
            "AlternateContent" => {
                if let Some(choice) = child(node, "Choice") {
                    for c in choice.children().filter(|n| n.is_element()) {
                        self.block(c)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn paragraph(&mut self, p: Node<'a, 'i>) -> Result<(), ConvertError> {
        let ppr = child(p, "pPr");
        let style_id = ppr
            .and_then(|n| val(n, "pStyle"))
            .or(self.styles.default_para.as_deref())
            .map(str::to_owned);
        let style = style_id.as_deref();
        let outline = ppr
            .and_then(|n| val(n, "outlineLvl")?.parse::<u8>().ok())
            .map(outline_level)
            .or_else(|| self.styles.outline(style));
        let heading = match outline {
            Some(Outline::Level(l)) => Some(l),
            _ => None,
        };
        let list = if heading.is_some() {
            None
        } else {
            self.list_of(ppr, style)
        };
        let base = self.styles.run(style);
        let line = self.line(
            p,
            RunCtx {
                base,
                link: None,
                heading: heading.is_some(),
            },
        )?;

        if heading.is_none() && list.is_none() {
            if line.is_code() {
                let raw = line.raw.trim_end_matches(['\n', ' ', '\t']);
                self.code.push(raw.to_owned());
                return Ok(());
            }
            if line.inline.is_blank() {
                // Blank lines inside a run of code paragraphs stay in the block.
                if !self.code.is_empty() {
                    self.code.push(String::new());
                }
                return Ok(());
            }
        }
        self.flush_code();
        let md = line.inline.finish();
        if let Some(level) = heading {
            self.writer.heading(level, &md);
        } else if let Some((depth, kind)) = list {
            if !md.trim().is_empty() {
                self.writer.list_item(depth, kind, &md);
            }
        } else {
            self.writer.paragraph(&md);
        }
        Ok(())
    }

    /// List depth and marker for a paragraph, counting ordered items.
    fn list_of(
        &mut self,
        ppr: Option<Node<'_, '_>>,
        style: Option<&str>,
    ) -> Option<(usize, ListKind)> {
        let direct = ppr.and_then(|p| child(p, "numPr")).and_then(num_pr);
        let from_style = self.styles.num(style);
        let (num_id, ilvl) = match (direct, &from_style) {
            (Some((id, l)), s) => (id, l.or_else(|| s.as_ref().and_then(|s| s.1))),
            (None, Some((id, l))) => (id.clone(), *l),
            (None, None) => return None,
        };
        let ilvl = ilvl.unwrap_or(0).min(8);
        let level = self.numbering.level(&num_id, ilvl)?;
        let ordered = level.ordered?;
        let counters = self.counters.entry(num_id).or_default();
        let depth = usize::from(ilvl);
        let n = match counters.get(depth) {
            Some(&n) => n.saturating_add(1),
            None => level.start,
        };
        // Deeper levels restart; skipped levels count from 0.
        counters.resize(depth, 0);
        counters.push(n);
        Some((
            depth,
            if ordered {
                ListKind::Ordered(n)
            } else {
                ListKind::Bullet
            },
        ))
    }

    fn line(&mut self, p: Node<'a, 'i>, rc: RunCtx<'_>) -> Result<Line, ConvertError> {
        let mut line = Line::new();
        self.inline(p, &mut line, rc)?;
        Ok(line)
    }

    // ------------------------------------------------------------ inline

    fn inline(
        &mut self,
        node: Node<'a, 'i>,
        line: &mut Line,
        rc: RunCtx<'_>,
    ) -> Result<(), ConvertError> {
        check_depth(node)?;
        for c in node.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "pPr" | "rPr" | "del" | "moveFrom" | "delText" | "instrText" | "Fallback" => {}
                "r" => self.run(c, line, rc)?,
                "hyperlink" => {
                    let target = attr(c, "id")
                        .and_then(|id| self.rels.get(id))
                        .filter(|r| r.external)
                        .map(|r| r.target.clone());
                    let link = target.as_deref().or(rc.link);
                    self.inline(c, line, RunCtx { link, ..rc })?;
                }
                "fldSimple" => {
                    let target = attr(c, "instr").and_then(hyperlink_instr);
                    let link = target.as_deref().or(rc.link);
                    self.inline(c, line, RunCtx { link, ..rc })?;
                }
                "AlternateContent" => {
                    if let Some(choice) = child(c, "Choice") {
                        self.inline(choice, line, rc)?;
                    }
                }
                "drawing" | "pict" | "object" => {
                    let md = self.picture(c)?;
                    line.md(&md);
                }
                _ => self.inline(c, line, rc)?,
            }
        }
        Ok(())
    }

    fn run(
        &mut self,
        r: Node<'a, 'i>,
        line: &mut Line,
        rc: RunCtx<'_>,
    ) -> Result<(), ConvertError> {
        let rpr = child(r, "rPr");
        let char_style = rpr.and_then(|n| val(n, "rStyle"));
        let props = RunProps::read(rpr)
            .over(self.styles.run(char_style))
            .over(rc.base);
        let hidden = props.hidden == Some(true);
        let field_link = self
            .fields
            .iter()
            .rev()
            .find(|f| f.in_result && f.link.is_some())
            .and_then(|f| f.link.clone());
        let style = Style {
            bold: props.bold == Some(true) && !rc.heading,
            italic: props.italic == Some(true) && !rc.heading,
            strike: props.strike == Some(true),
            code: props.code == Some(true) && !rc.heading,
            link: rc.link.map(str::to_owned).or(field_link),
        };
        self.run_content(r, line, &style, hidden)
    }

    fn run_content(
        &mut self,
        r: Node<'a, 'i>,
        line: &mut Line,
        style: &Style,
        hidden: bool,
    ) -> Result<(), ConvertError> {
        check_depth(r)?;
        for c in r.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "fldChar" => match attr(c, "fldCharType") {
                    Some("begin") => self.fields.push(Field {
                        instr: String::new(),
                        in_result: false,
                        link: None,
                    }),
                    Some("separate") => {
                        if let Some(f) = self.fields.last_mut() {
                            f.in_result = true;
                            f.link = hyperlink_instr(&f.instr);
                        }
                    }
                    Some("end") => {
                        self.fields.pop();
                    }
                    _ => {}
                },
                "instrText" => {
                    if let Some(f) = self.fields.last_mut()
                        && !f.in_result
                        && f.instr.len() < 4096
                    {
                        f.instr.push_str(c.text().unwrap_or_default());
                    }
                }
                _ if hidden => {}
                // Field instructions (between begin and separate) aren't shown.
                _ if self.fields.last().is_some_and(|f| !f.in_result) => {}
                "t" => line.text(c.text().unwrap_or_default(), style),
                "tab" | "ptab" => line.text("\t", style),
                "noBreakHyphen" => line.text("-", style),
                "br" => {
                    if !matches!(attr(c, "type"), Some("page" | "column")) {
                        line.brk();
                    }
                }
                "cr" => line.brk(),
                "drawing" | "pict" | "object" => {
                    let md = self.picture(c)?;
                    line.md(&md);
                }
                "footnoteReference" | "endnoteReference" => {
                    let endnote = c.tag_name().name() == "endnoteReference";
                    if let Some(id) = attr(c, "id") {
                        let md = self.note_ref(endnote, id);
                        line.md(&md);
                    }
                }
                "AlternateContent" => {
                    if let Some(choice) = child(c, "Choice") {
                        self.run_content(choice, line, style, hidden)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ pictures

    fn picture(&mut self, node: Node<'a, 'i>) -> Result<String, ConvertError> {
        let alt = node
            .descendants()
            .find(|n| n.has_tag_name_local("docPr"))
            .and_then(|d| {
                attr(d, "descr")
                    .filter(|s| !s.trim().is_empty())
                    .or(attr(d, "title"))
            })
            .or_else(|| {
                node.descendants()
                    .find(|n| n.has_tag_name_local("shape"))
                    .and_then(|s| attr(s, "alt"))
            })
            .map(one_line)
            .unwrap_or_default();
        let ids: Vec<&str> = node
            .descendants()
            .filter_map(|n| match n.tag_name().name() {
                "blip" if n.is_element() => attr(n, "embed").or_else(|| attr(n, "link")),
                "imagedata" if n.is_element() => attr(n, "id"),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            // A text box: its text, inline. Only this drawing's own paragraphs:
            // text boxes nested in them are read through those paragraphs
            // (reading them here too would grow exponentially with nesting).
            let mut parts = Vec::new();
            let own = |p: &Node<'_, '_>| {
                p.has_tag_name_local("p")
                    && p.ancestors().any(|a| a.has_tag_name_local("txbxContent"))
                    && p.ancestors()
                        .find(|a| matches!(a.tag_name().name(), "drawing" | "pict" | "object"))
                        == Some(node)
            };
            for p in node.descendants().filter(own) {
                let line = self.line(p, plain_ctx())?;
                if !line.inline.is_blank() {
                    parts.push(line.inline.finish());
                }
            }
            if parts.is_empty() {
                return Ok(markdown::escape_text(&alt));
            }
            return Ok(parts.join(" "));
        }
        let mut out = Vec::new();
        for id in ids {
            // A missing or oversized picture keeps its alt text.
            let target = self
                .rels
                .get(id)
                .filter(|r| !r.external)
                .map(|r| r.target.clone());
            let md = self
                .pkg
                .picture_md(&mut self.images, target.as_deref(), &alt)?;
            if !md.is_empty() && !out.contains(&md) {
                out.push(md);
            }
        }
        Ok(out.join(" "))
    }

    // ------------------------------------------------------------ notes

    fn note_ref(&mut self, endnote: bool, id: &str) -> String {
        let key = (endnote, id.to_owned());
        let Some(&note) = self.notes.get(&key) else {
            return String::new();
        };
        if let Some(label) = self.note_labels.get(&key) {
            return footnote_ref(label);
        }
        let label = (self.note_labels.len() + 1).to_string();
        self.note_labels.insert(key, label.clone());
        self.note_queue.push((label.clone(), note));
        footnote_ref(&label)
    }

    fn write_notes(&mut self) -> Result<(), ConvertError> {
        let mut i = 0;
        // Notes may reference notes; each is queued once.
        while i < self.note_queue.len() {
            let (label, note) = self.note_queue[i].clone();
            let text = self.flat_text(note)?;
            self.writer.footnote(&label, &text);
            i += 1;
        }
        Ok(())
    }

    // ------------------------------------------------------------ tables

    fn table(&mut self, tbl: Node<'a, 'i>) -> Result<Vec<Vec<String>>, ConvertError> {
        let mut rows = Vec::new();
        for tr in elements_through_wrappers(tbl, "tr")? {
            let trpr = child(tr, "trPr");
            if trpr.is_some_and(|p| child(p, "del").is_some()) {
                continue;
            }
            let mut row = Vec::new();
            let before = trpr
                .and_then(|p| val(p, "gridBefore")?.parse::<usize>().ok())
                .unwrap_or(0)
                .min(64);
            row.resize(before, String::new());
            for tc in elements_through_wrappers(tr, "tc")? {
                let tcpr = child(tc, "tcPr");
                let span = tcpr
                    .and_then(|p| val(p, "gridSpan")?.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 64);
                let continued = tcpr
                    .and_then(|p| child(p, "vMerge"))
                    .is_some_and(|m| attr(m, "val") != Some("restart"));
                let text = if continued {
                    String::new()
                } else {
                    self.cell_text(tc)?
                };
                row.push(text);
                row.extend(std::iter::repeat_n(String::new(), span - 1));
            }
            rows.push(row);
        }
        Ok(rows)
    }

    /// A cell's blocks as inline Markdown, one line per paragraph.
    fn cell_text(&mut self, tc: Node<'a, 'i>) -> Result<String, ConvertError> {
        self.flat_text(tc)
    }

    /// Paragraphs (lists marked, nested tables flattened) as inline Markdown
    /// lines: for table cells and notes.
    fn flat_text(&mut self, node: Node<'a, 'i>) -> Result<String, ConvertError> {
        let mut lines = Vec::new();
        self.flat_into(node, &mut lines)?;
        Ok(lines.join("\n"))
    }

    fn flat_into(
        &mut self,
        node: Node<'a, 'i>,
        lines: &mut Vec<String>,
    ) -> Result<(), ConvertError> {
        check_depth(node)?;
        for c in node.children().filter(|n| n.is_element()) {
            match c.tag_name().name() {
                "p" => {
                    let ppr = child(c, "pPr");
                    let style = ppr.and_then(|n| val(n, "pStyle")).map(str::to_owned);
                    let list = self.list_of(ppr, style.as_deref());
                    let base = self.styles.run(style.as_deref());
                    let line = self.line(
                        c,
                        RunCtx {
                            base,
                            ..plain_ctx()
                        },
                    )?;
                    if line.inline.is_blank() {
                        continue;
                    }
                    let md = line.inline.finish();
                    lines.push(match list {
                        Some((_, ListKind::Bullet)) => format!("• {md}"),
                        Some((_, ListKind::Ordered(n))) => format!("{n}. {md}"),
                        None => md,
                    });
                }
                "tbl" => {
                    for row in self.table(c)? {
                        let cells: Vec<String> = row
                            .into_iter()
                            .map(|s| s.replace('\n', " "))
                            .filter(|s| !s.trim().is_empty())
                            .collect();
                        if !cells.is_empty() {
                            lines.push(cells.join("; "));
                        }
                    }
                }
                "del" | "moveFrom" | "tcPr" | "pPr" => {}
                _ => self.flat_into(c, lines)?,
            }
        }
        Ok(())
    }
}

fn plain_ctx() -> RunCtx<'static> {
    RunCtx {
        base: RunProps::default(),
        link: None,
        heading: false,
    }
}

/// Child elements named `name`, looking through `sdt`/`customXml` wrappers.
fn elements_through_wrappers<'a, 'i>(
    node: Node<'a, 'i>,
    name: &str,
) -> Result<Vec<Node<'a, 'i>>, ConvertError> {
    check_depth(node)?;
    let mut out = Vec::new();
    for c in node.children().filter(|n| n.is_element()) {
        match c.tag_name().name() {
            n if n == name => out.push(c),
            "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" => {
                out.extend(elements_through_wrappers(c, name)?);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The URL of a `HYPERLINK "url"` field (not `\l` anchors).
fn hyperlink_instr(instr: &str) -> Option<String> {
    let rest = instr.trim().strip_prefix("HYPERLINK")?;
    if rest.contains("\\l") && !rest.trim_start().starts_with('"') {
        return None;
    }
    let rest = rest.trim_start();
    let url = match rest.strip_prefix('"') {
        Some(q) => q.split('"').next()?,
        None => rest.split_whitespace().next()?,
    };
    (!url.is_empty()).then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperlink_fields() {
        assert_eq!(
            hyperlink_instr(r#" HYPERLINK "https://a.b/c" \o "tip" "#).as_deref(),
            Some("https://a.b/c")
        );
        assert_eq!(hyperlink_instr(r#"HYPERLINK \l "_Toc1""#), None);
        assert_eq!(hyperlink_instr("PAGE"), None);
    }

    #[test]
    fn monospace_fonts() {
        assert!(is_monospace("Courier New"));
        assert!(is_monospace("Consolas"));
        assert!(is_monospace("Source Code Pro") || is_monospace("Fira Code"));
        assert!(!is_monospace("Calibri"));
    }
}
