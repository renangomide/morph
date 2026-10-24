// SPDX-License-Identifier: GPL-3.0-or-later
//! Presentation → Markdown, native OOXML reader (SPEC §4.6).
//! Reached through `office::plan` →
//! `Plan::SlidesToMarkdown { engine }`. PPT/ODP (`engine` is `Some`) are first
//! converted to PPTX in the task's temp folder by the engine.
//!
//! Per slide (in `presentation.xml` order): `## Slide N: Title` (title/ctrTitle
//! placeholder; `## Slide N` without one), then the shapes in reading order:
//! placeholders first, then by position (top, then left); groups are read as
//! one unit, their shapes in the same order. Text: paragraphs of body
//! placeholders are bullets nested by `a:pPr/@lvl`; other text boxes are
//! paragraphs unless they have `a:buChar` / `a:buAutoNum`; `a:buNone` is never
//! a bullet. Bold/italic/strike runs, external `a:hlinkClick` links, line
//! breaks. Tables (`a:tbl`) → pipe tables (first row = header). Pictures →
//! `name (images)/` + alt text (`p:cNvPr/@descr`). Charts → a table of the
//! cached values (categories × series). Speaker notes (`### Notes:`) when
//! `opts.slide_notes`.
//!
//! Hostile files: parts are size-capped, external relationships are only used
//! as link targets and never read.

use super::detect::EngineChoice;
use super::ooxml::{Package, Rel, check_depth, child};
use super::options::{DocFormat, Family, OfficeConvertOpts};
use crate::converter::{TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::markdown::{
    ImageFolder, Inline, ListKind, MarkdownWriter, Style, escape_text, one_line, write_markdown,
};
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::path::Path;

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MAX_SLIDES: usize = 10_000;
/// Chart tables: at most this many series and cells (the rest is cut, with a note).
const MAX_CHART_SERIES: usize = 256;
const MAX_CHART_CELLS: usize = 1_000_000;
/// Chart point indexes at or above this are ignored.
const MAX_CHART_POINTS: usize = 100_000;

pub fn convert(
    task: &Task,
    input: &Path,
    opts: &OfficeConvertOpts,
    engine: Option<&EngineChoice>,
    ctx: &TaskCtx<'_>,
) -> Result<TaskOutput, ConvertError> {
    let converted;
    let source = match engine {
        Some(engine) => {
            converted = super::engine_convert::export(
                input,
                Family::PowerPoint,
                DocFormat::Pptx,
                engine,
                ctx,
            )?;
            converted.as_path()
        }
        None => input,
    };
    let pkg = Package::open(source)?;
    let folder = task.output.folder_for(input);
    let mut images = ImageFolder::new(&folder, input, opts.images);
    let md = Reader { pkg }.markdown(opts.slide_notes, &mut images, ctx)?;
    ctx.check_cancel()?;
    let out = write_markdown(&folder, input, &md, images)?;
    ctx.progress(1.0, None);
    Ok(out)
}

// ---------------------------------------------------------------- reader

struct Reader {
    pkg: Package,
}

impl Reader {
    fn xml(&mut self, part: &str) -> Result<Option<String>, ConvertError> {
        self.pkg.xml(part)
    }

    fn rels(&mut self, part: &str) -> Result<HashMap<String, Rel>, ConvertError> {
        self.pkg.rels(part)
    }
    fn markdown(
        &mut self,
        notes: bool,
        images: &mut ImageFolder,
        ctx: &TaskCtx<'_>,
    ) -> Result<String, ConvertError> {
        let pres_part = "ppt/presentation.xml";
        let pres = self
            .xml(pres_part)?
            .ok_or_else(|| super::damaged_document("no ppt/presentation.xml"))?;
        let pres_rels = self.rels(pres_part)?;
        let slides: Vec<String> = {
            let doc = parse(&pres, pres_part)?;
            doc.descendants()
                .filter(|n| n.has_tag_name("sldId"))
                .filter_map(|n| r_id(n, "id"))
                .filter_map(|id| pres_rels.get(id))
                .filter(|r| !r.external)
                .map(|r| r.target.clone())
                .take(MAX_SLIDES)
                .collect()
        };
        let mut w = MarkdownWriter::new();
        for (i, part) in slides.iter().enumerate() {
            ctx.check_cancel()?;
            self.slide(&mut w, i + 1, part, notes, images)?;
            ctx.progress((i + 1) as f32 / slides.len() as f32 * 0.95, None);
        }
        Ok(w.finish())
    }

    fn slide(
        &mut self,
        w: &mut MarkdownWriter,
        number: usize,
        part: &str,
        notes: bool,
        images: &mut ImageFolder,
    ) -> Result<(), ConvertError> {
        let Some(text) = self.xml(part)? else {
            w.heading(2, &format!("Slide {number}"));
            return Ok(());
        };
        let rels = self.rels(part)?;
        let doc = parse(&text, part)?;
        let tree = doc.descendants().find(|n| n.has_tag_name("spTree"));
        let title = tree.and_then(|t| {
            t.descendants()
                .find(|n| n.has_tag_name("sp") && is_title(placeholder_type(*n)))
                .map(|sp| one_line(&plain_text(sp)))
                .filter(|t| !t.is_empty())
        });
        match &title {
            Some(t) => w.heading(2, &format!("Slide {number}: {}", escape_text(t))),
            None => w.heading(2, &format!("Slide {number}")),
        }
        if let Some(tree) = tree {
            self.shapes(w, tree, &rels, images)?;
        }
        if notes
            && let Some(rel) = rels
                .values()
                .find(|r| r.kind.ends_with("/notesSlide") && !r.external)
        {
            let target = rel.target.clone();
            self.notes(w, &target)?;
        }
        Ok(())
    }

    /// The shapes of an `spTree` / `grpSp`, in reading order.
    fn shapes(
        &mut self,
        w: &mut MarkdownWriter,
        tree: Node<'_, '_>,
        rels: &HashMap<String, Rel>,
        images: &mut ImageFolder,
    ) -> Result<(), ConvertError> {
        check_depth(tree)?;
        let mut shapes: Vec<Node<'_, '_>> = tree
            .children()
            .filter(|n| {
                matches!(
                    n.tag_name().name(),
                    "sp" | "pic" | "graphicFrame" | "grpSp" | "cxnSp"
                )
            })
            .collect();
        shapes.sort_by_key(|n| sort_key(*n));
        for shape in shapes {
            match shape.tag_name().name() {
                "sp" => {
                    let ph = placeholder_type(shape);
                    if is_title(ph) {
                        continue;
                    }
                    if let Some(body) = child(shape, "txBody") {
                        // Body/object placeholders (no type, `body`, `obj`) are bullet lists.
                        let bullets = matches!(ph, Some("" | "body" | "obj"));
                        text_body(w, body, rels, bullets);
                    }
                }
                "pic" => {
                    let alt = shape
                        .descendants()
                        .find(|n| n.has_tag_name("cNvPr"))
                        .and_then(|n| n.attribute("descr").or(n.attribute("title")))
                        .unwrap_or("");
                    let target = shape
                        .descendants()
                        .find(|n| n.has_tag_name("blip"))
                        .and_then(|b| r_id(b, "embed"))
                        .and_then(|id| rels.get(id))
                        .filter(|r| !r.external)
                        .map(|r| r.target.clone());
                    // A missing, oversized or over-budget picture keeps its alt text.
                    let md = self.pkg.picture_md(images, target.as_deref(), alt)?;
                    w.paragraph(&md);
                }
                "graphicFrame" => {
                    if let Some(tbl) = shape.descendants().find(|n| n.has_tag_name("tbl")) {
                        w.table(&table(tbl, rels));
                    } else if let Some(chart) = shape
                        .descendants()
                        .find(|n| n.has_tag_name("chart"))
                        .and_then(|c| r_id(c, "id"))
                        .and_then(|id| rels.get(id))
                        .filter(|r| !r.external)
                    {
                        let part = chart.target.clone();
                        self.chart(w, &part)?;
                    }
                }
                "grpSp" => self.shapes(w, shape, rels, images)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn chart(&mut self, w: &mut MarkdownWriter, part: &str) -> Result<(), ConvertError> {
        let Some(text) = self.xml(part)? else {
            return Ok(());
        };
        let doc = parse(&text, part)?;
        let title = doc
            .descendants()
            .find(|n| {
                n.has_tag_name("title") && n.parent().is_some_and(|p| p.has_tag_name("chart"))
            })
            .map(|t| one_line(&plain_text(t)))
            .filter(|t| !t.is_empty());
        let mut header = vec![String::new()];
        let mut categories: Vec<String> = Vec::new();
        let mut columns: Vec<Vec<String>> = Vec::new();
        // Hostile charts: series, points and cells are capped (cut: a note).
        let mut cells = 0usize;
        let mut cut = false;
        for ser in doc.descendants().filter(|n| n.has_tag_name("ser")) {
            if columns.len() >= MAX_CHART_SERIES {
                cut = true;
                break;
            }
            let name = child(ser, "tx")
                .and_then(|t| t.descendants().find(|n| n.has_tag_name("v")))
                .and_then(|v| v.text())
                .unwrap_or("");
            header.push(escape_text(name));
            if categories.is_empty()
                && let Some(cat) = child(ser, "cat").or(child(ser, "xVal"))
            {
                let (points, more) = cached_points(cat, MAX_CHART_CELLS - cells);
                cells += points.len();
                cut |= more;
                categories = points;
            }
            let (points, more) = match child(ser, "val").or(child(ser, "yVal")) {
                Some(val) => cached_points(val, MAX_CHART_CELLS - cells),
                None => (Vec::new(), false),
            };
            cells += points.len();
            cut |= more;
            columns.push(points);
        }
        if columns.is_empty() {
            return Ok(());
        }
        let mut len = columns
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(categories.len());
        let max_rows = MAX_CHART_CELLS / (columns.len() + 1);
        if len > max_rows {
            len = max_rows;
            cut = true;
        }
        if let Some(t) = title {
            w.paragraph(&format!("Chart: {}", escape_text(&t)));
        }
        let mut rows = vec![header];
        for i in 0..len {
            let mut row = vec![escape_text(categories.get(i).map_or("", String::as_str))];
            row.extend(
                columns
                    .iter()
                    .map(|c| escape_text(c.get(i).map_or("", String::as_str))),
            );
            rows.push(row);
        }
        w.table(&rows);
        if cut {
            w.paragraph("… more chart values not shown.");
        }
        Ok(())
    }

    fn notes(&mut self, w: &mut MarkdownWriter, part: &str) -> Result<(), ConvertError> {
        let Some(text) = self.xml(part)? else {
            return Ok(());
        };
        let rels = self.rels(part)?;
        let doc = parse(&text, part)?;
        let body = doc
            .descendants()
            .filter(|n| n.has_tag_name("sp"))
            .find(|sp| placeholder_type(*sp) == Some("body"))
            .and_then(|sp| child(sp, "txBody"));
        let Some(body) = body else {
            return Ok(());
        };
        if one_line(&plain_text(body)).is_empty() {
            return Ok(());
        }
        w.heading(3, "Notes:");
        text_body(w, body, &rels, false);
        Ok(())
    }
}

// ---------------------------------------------------------------- XML helpers

fn parse<'a>(text: &'a str, part: &str) -> Result<Document<'a>, ConvertError> {
    if super::ooxml::xml_depth(text) > super::ooxml::MAX_DEPTH {
        return Err(super::ooxml::too_deep());
    }
    Document::parse(text).map_err(|e| super::damaged_document(format!("{part}: {e}")))
}

/// An `r:`-namespaced attribute (`r:id`, `r:embed`).
fn r_id<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name && a.namespace() == Some(R_NS))
        .map(|a| a.value())
}

/// `Some(type)` for placeholders (`""` when untyped, i.e. body/object), else `None`.
fn placeholder_type<'a>(shape: Node<'a, '_>) -> Option<&'a str> {
    let nv = shape
        .children()
        .find(|n| n.tag_name().name().starts_with("nv"))?;
    let ph = child(child(nv, "nvPr")?, "ph")?;
    Some(ph.attribute("type").unwrap_or(""))
}

fn is_title(ph: Option<&str>) -> bool {
    matches!(ph, Some("title" | "ctrTitle"))
}

/// (placeholders first, top, left).
fn sort_key(shape: Node<'_, '_>) -> (u8, i64, i64) {
    let rank = match placeholder_type(shape) {
        Some(t) if is_title(Some(t)) => 0,
        Some(_) => 1,
        None => 2,
    };
    let off = shape
        .children()
        .find(|n| matches!(n.tag_name().name(), "spPr" | "xfrm" | "grpSpPr"))
        .and_then(|p| {
            if p.has_tag_name("xfrm") {
                Some(p)
            } else {
                child(p, "xfrm")
            }
        })
        .and_then(|x| child(x, "off"));
    let coord = |a: &str| {
        off.and_then(|o| o.attribute(a))
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    (rank, coord("y"), coord("x"))
}

/// All `a:t` text, paragraphs separated by newlines.
fn plain_text(node: Node<'_, '_>) -> String {
    let mut out = String::new();
    for n in node.descendants() {
        if n.has_tag_name("t") {
            out.push_str(n.text().unwrap_or(""));
        } else if n.has_tag_name("p") || n.has_tag_name("br") {
            out.push('\n');
        }
    }
    out
}

/// Cached point values (`c:strCache`/`c:numCache` `c:pt` by `@idx`), at
/// most `limit` (and [`MAX_CHART_POINTS`]) of them; `true` when some were cut.
fn cached_points(node: Node<'_, '_>, limit: usize) -> (Vec<String>, bool) {
    let limit = limit.min(MAX_CHART_POINTS);
    let mut cut = false;
    let mut points: Vec<(usize, String)> = Vec::new();
    for pt in node.descendants().filter(|n| n.has_tag_name("pt")) {
        let Some(idx) = pt.attribute("idx").and_then(|i| i.parse::<usize>().ok()) else {
            continue;
        };
        let Some(v) = child(pt, "v") else { continue };
        if idx < limit {
            points.push((idx, v.text().unwrap_or("").to_owned()));
        } else {
            cut = true;
        }
    }
    points.sort_by_key(|p| p.0);
    let len = points.last().map_or(0, |p| p.0 + 1);
    let mut out = vec![String::new(); len];
    for (i, v) in points {
        out[i] = v;
    }
    (out, cut)
}

// ---------------------------------------------------------------- text

/// One paragraph's inline Markdown.
fn paragraph_md(p: Node<'_, '_>, rels: &HashMap<String, Rel>) -> Inline {
    let mut line = Inline::new();
    for run in p.children() {
        match run.tag_name().name() {
            "r" | "fld" => {
                let text = child(run, "t").and_then(|t| t.text()).unwrap_or("");
                let rpr = child(run, "rPr");
                let flag = |a: &str| {
                    rpr.and_then(|r| r.attribute(a))
                        .is_some_and(|v| v == "1" || v == "true")
                };
                let link = rpr
                    .and_then(|r| child(r, "hlinkClick"))
                    .and_then(|h| r_id(h, "id"))
                    .and_then(|id| rels.get(id))
                    .filter(|r| r.external)
                    .map(|r| r.target.clone());
                let style = Style {
                    bold: flag("b"),
                    italic: flag("i"),
                    strike: rpr
                        .and_then(|r| r.attribute("strike"))
                        .is_some_and(|v| v != "noStrike"),
                    code: false,
                    link,
                };
                line.push_text(text, &style);
            }
            "br" => line.push_break(),
            _ => {}
        }
    }
    line
}

/// A `txBody`: bullets / numbered items / paragraphs.
fn text_body(
    w: &mut MarkdownWriter,
    body: Node<'_, '_>,
    rels: &HashMap<String, Rel>,
    bullets: bool,
) {
    // Numbering per level: (next number).
    let mut counters: Vec<u32> = Vec::new();
    for p in body.children().filter(|n| n.has_tag_name("p")) {
        let line = paragraph_md(p, rels);
        if line.is_blank() {
            continue;
        }
        let ppr = child(p, "pPr");
        let level = ppr
            .and_then(|x| x.attribute("lvl"))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(8);
        let has = |name: &str| ppr.and_then(|x| child(x, name));
        let kind = if has("buNone").is_some() {
            None
        } else if let Some(auto) = has("buAutoNum") {
            counters.resize(level + 1, 0);
            if counters[level] == 0 {
                counters[level] = auto
                    .attribute("startAt")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
            } else {
                counters[level] += 1;
            }
            Some(ListKind::Ordered(counters[level]))
        } else if bullets || has("buChar").is_some() || has("buBlip").is_some() {
            Some(ListKind::Bullet)
        } else {
            None
        };
        if !matches!(kind, Some(ListKind::Ordered(_))) {
            counters.truncate(level);
        } else {
            counters.truncate(level + 1);
        }
        // `|` escaped outside tables too: slide text like `A | B` never turns
        // into a table row when read next to other lines.
        let md = escape_pipes(&line.finish());
        match kind {
            Some(kind) => w.list_item(level, kind, &md),
            None => {
                counters.clear();
                w.paragraph(&md);
            }
        }
    }
    w.end_list();
}

/// Unescaped `|` → `\|`.
fn escape_pipes(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut backslashes = 0usize;
    for c in md.chars() {
        if c == '|' && backslashes.is_multiple_of(2) {
            out.push('\\');
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
        out.push(c);
    }
    out
}

/// Rows of an `a:tbl` (cells: paragraphs joined by line breaks).
fn table(tbl: Node<'_, '_>, rels: &HashMap<String, Rel>) -> Vec<Vec<String>> {
    tbl.children()
        .filter(|n| n.has_tag_name("tr"))
        .map(|tr| {
            tr.children()
                .filter(|n| n.has_tag_name("tc"))
                .map(|tc| {
                    tc.descendants()
                        .filter(|n| n.has_tag_name("p"))
                        .map(|p| paragraph_md(p, rels).finish())
                        .filter(|s| !s.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_and_bullets() {
        let xml = r#"<p:txBody xmlns:p="p" xmlns:a="a">
            <a:p><a:pPr><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:t>One</a:t></a:r></a:p>
            <a:p><a:pPr lvl="1"><a:buChar char="•"/></a:pPr><a:r><a:rPr b="1"/><a:t>Sub</a:t></a:r></a:p>
            <a:p><a:pPr><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:t>Two</a:t></a:r></a:p>
            <a:p><a:r><a:t>Plain</a:t></a:r><a:br/><a:r><a:t>next</a:t></a:r></a:p>
        </p:txBody>"#;
        let doc = Document::parse(xml).unwrap();
        let mut w = MarkdownWriter::new();
        text_body(&mut w, doc.root_element(), &HashMap::new(), false);
        assert_eq!(
            w.finish(),
            "1. One\n   - **Sub**\n2. Two\n\nPlain\\\nnext\n"
        );
    }
}
