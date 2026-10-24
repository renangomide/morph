// SPDX-License-Identifier: GPL-3.0-or-later
//! Where images are drawn: walks every page's content (and the form XObjects it
//! draws) tracking `q`/`Q`/`cm`, and records the largest size each image XObject
//! is shown at, in points. Images also reachable through patterns, Type 3 fonts
//! or annotation appearances get an unknown size (never downscaled); images only
//! reachable some other way are never recorded, so they stay untouched.
//!
//! Hostile files: each form is decoded once (cached), decoded bytes and content
//! operators have a total budget, and every walk is cycle- and depth-guarded.
//! Running out of budget marks the result incomplete (nothing is downscaled).

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// `[a b c d e f]`, as in the PDF `cm` operator.
type Matrix = [f32; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Nested form XObjects deeper than this are not followed.
const MAX_FORM_DEPTH: usize = 12;

/// Content operators walked in total before giving up (pathological files).
const MAX_OPERATIONS: usize = 20_000_000;

/// Decoded content streams larger than this are not walked.
const MAX_CONTENT_BYTES: usize = 32 * 1024 * 1024;

/// Decoded content bytes (pages + forms) in total before giving up.
const MAX_TOTAL_CONTENT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct Usage {
    /// Largest (width, height) in points each image XObject is drawn at.
    pub placed: HashMap<ObjectId, (f32, f32)>,
    /// Images whose size on the page is unknown (content unreadable, or drawn
    /// from a pattern, glyph or annotation): they must not be downscaled.
    pub unknown: HashSet<ObjectId>,
    /// The walk stopped early; placements may be incomplete.
    pub incomplete: bool,
}

/// `m × n` (apply `m`, then `n`).
fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn number(o: &Object) -> Option<f32> {
    o.as_float().ok().filter(|v| v.is_finite())
}

fn matrix(operands: &[Object]) -> Option<Matrix> {
    if operands.len() != 6 {
        return None;
    }
    let mut m = IDENTITY;
    for (slot, o) in m.iter_mut().zip(operands) {
        *slot = number(o)?;
    }
    Some(m)
}

/// Follow references.
fn resolve<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    resolve(doc, o).and_then(|o| o.as_dict().ok())
}

/// The resources that apply to a page: its own, or the nearest ancestor's.
fn page_resources(doc: &Document, page: ObjectId) -> Option<&Dictionary> {
    let mut node = doc.get_dictionary(page).ok()?;
    for _ in 0..64 {
        if let Ok(r) = node.get(b"Resources") {
            return dict_of(doc, r);
        }
        node = dict_of(doc, node.get(b"Parent").ok()?)?;
    }
    None
}

/// Plain content of a stream, bounded.
pub(super) fn plain(stream: &Stream, limit: usize) -> Option<Vec<u8>> {
    stream.get_plain_content_with_limit(limit).ok()
}

struct Walker<'a> {
    doc: &'a Document,
    usage: Usage,
    ops_left: usize,
    bytes_left: usize,
    /// Form XObjects being walked (cycle guard).
    forms: Vec<ObjectId>,
    /// Parsed content of each form seen (`None` = unreadable).
    form_cache: HashMap<ObjectId, Option<Rc<Content>>>,
    /// Objects already handled by [`Walker::mark_object`].
    marked: HashSet<ObjectId>,
    /// Resource dictionaries already checked for patterns and Type 3 fonts.
    indirect_done: HashSet<*const Dictionary>,
}

impl<'a> Walker<'a> {
    fn new(doc: &'a Document) -> Self {
        Walker {
            doc,
            usage: Usage::default(),
            ops_left: MAX_OPERATIONS,
            bytes_left: MAX_TOTAL_CONTENT_BYTES,
            forms: Vec::new(),
            form_cache: HashMap::new(),
            marked: HashSet::new(),
            indirect_done: HashSet::new(),
        }
    }

    /// Size limit for the next decode: per stream, and what's left in total.
    fn limit(&self) -> usize {
        MAX_CONTENT_BYTES.min(self.bytes_left)
    }

    /// Count decoded bytes against the budget, then parse them.
    fn parse(&mut self, bytes: Option<Vec<u8>>) -> Option<Rc<Content>> {
        let Some(bytes) = bytes else {
            if self.bytes_left < MAX_CONTENT_BYTES {
                // Probably refused for the total budget, not this stream.
                self.usage.incomplete = true;
            }
            return None;
        };
        self.bytes_left = self.bytes_left.saturating_sub(bytes.len());
        Content::decode(&bytes).ok().map(Rc::new)
    }

    fn xobject(&self, resources: Option<&'a Dictionary>, name: &[u8]) -> Option<ObjectId> {
        let xobjects = dict_of(self.doc, resources?.get(b"XObject").ok()?)?;
        xobjects.get(name).ok()?.as_reference().ok()
    }

    fn walk(&mut self, content: &Content, resources: Option<&'a Dictionary>, ctm: Matrix) {
        self.mark_indirect(resources);
        let mut ctm = ctm;
        let mut stack: Vec<Matrix> = Vec::new();
        for op in &content.operations {
            if self.ops_left == 0 {
                self.usage.incomplete = true;
                return;
            }
            self.ops_left -= 1;
            match op.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => ctm = stack.pop().unwrap_or(ctm),
                "cm" => {
                    if let Some(m) = matrix(&op.operands) {
                        ctm = mul(&m, &ctm);
                    }
                }
                "Do" => {
                    if let Some(name) = op.operands.first().and_then(|o| o.as_name().ok())
                        && let Some(id) = self.xobject(resources, name)
                    {
                        self.draw(id, resources, ctm);
                    }
                }
                _ => {}
            }
            if self.usage.incomplete {
                return;
            }
        }
    }

    fn draw(&mut self, id: ObjectId, resources: Option<&'a Dictionary>, ctm: Matrix) {
        let Ok(stream) = self.doc.get_object(id).and_then(Object::as_stream) else {
            return;
        };
        match stream.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => {
                // The image fills the unit square: its sides are the CTM's two axes.
                let w = ctm[0].hypot(ctm[1]);
                let h = ctm[2].hypot(ctm[3]);
                let e = self.usage.placed.entry(id).or_insert((0.0, 0.0));
                e.0 = e.0.max(w);
                e.1 = e.1.max(h);
            }
            Ok(b"Form") => {
                if self.forms.contains(&id) || self.forms.len() >= MAX_FORM_DEPTH {
                    return;
                }
                let m = stream
                    .dict
                    .get(b"Matrix")
                    .ok()
                    .and_then(|o| resolve(self.doc, o))
                    .and_then(|o| o.as_array().ok())
                    .and_then(|a| matrix(a))
                    .unwrap_or(IDENTITY);
                let form_resources = match stream.dict.get(b"Resources") {
                    Ok(r) => dict_of(self.doc, r),
                    Err(_) => resources,
                };
                let content = match self.form_cache.get(&id) {
                    Some(c) => c.clone(),
                    None => {
                        let bytes = plain(stream, self.limit());
                        let c = self.parse(bytes);
                        self.form_cache.insert(id, c.clone());
                        c
                    }
                };
                let Some(content) = content else {
                    self.mark_unknown(form_resources, 0);
                    return;
                };
                self.forms.push(id);
                self.walk(&content, form_resources, mul(&m, &ctm));
                self.forms.pop();
            }
            _ => {}
        }
    }

    /// Every image reachable from `resources` (through forms, patterns and
    /// Type 3 fonts too) gets an unknown size.
    fn mark_unknown(&mut self, resources: Option<&'a Dictionary>, depth: usize) {
        let Some(resources) = resources else { return };
        for key in [b"XObject".as_slice(), b"Pattern", b"Font"] {
            let Some(entries) = resources.get(key).ok().and_then(|o| dict_of(self.doc, o)) else {
                continue;
            };
            for (_, o) in entries.iter() {
                self.mark_object(o, depth);
            }
        }
    }

    /// `o` is an image (→ unknown size) or something with resources (a form,
    /// tiling pattern, Type 3 font): mark everything it can draw.
    fn mark_object(&mut self, o: &'a Object, depth: usize) {
        if depth > MAX_FORM_DEPTH {
            return;
        }
        let id = o.as_reference().ok();
        if let Some(id) = id
            && !self.marked.insert(id)
        {
            return;
        }
        let dict = match resolve(self.doc, o) {
            Some(Object::Stream(s)) => &s.dict,
            Some(Object::Dictionary(d)) => d,
            _ => return,
        };
        if dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image") {
            if let Some(id) = id {
                self.usage.unknown.insert(id);
            }
            return;
        }
        let inner = dict
            .get(b"Resources")
            .ok()
            .and_then(|r| dict_of(self.doc, r));
        self.mark_unknown(inner, depth + 1);
    }

    /// Images drawn by patterns or Type 3 glyphs in `resources`: their size
    /// isn't tracked, so it is unknown. Each resource dictionary once.
    fn mark_indirect(&mut self, resources: Option<&'a Dictionary>) {
        let Some(resources) = resources else { return };
        if !self.indirect_done.insert(std::ptr::from_ref(resources)) {
            return;
        }
        for key in [b"Pattern".as_slice(), b"Font"] {
            let Some(entries) = resources.get(key).ok().and_then(|o| dict_of(self.doc, o)) else {
                continue;
            };
            for (_, o) in entries.iter() {
                self.mark_object(o, 0);
            }
        }
    }

    /// Images in annotation appearances (`/AP` `/N`, `/R`, `/D`): unknown size.
    fn mark_annotations(&mut self, page: ObjectId) {
        let doc = self.doc;
        let Ok(annots) = doc.get_page_annotations(page) else {
            return;
        };
        for annot in annots {
            let Some(ap) = annot.get(b"AP").ok().and_then(|o| dict_of(doc, o)) else {
                continue;
            };
            for (_, appearance) in ap.iter() {
                match resolve(doc, appearance) {
                    // One appearance stream.
                    Some(Object::Stream(_)) => self.mark_object(appearance, 0),
                    // A state → stream dictionary (check boxes and the like).
                    Some(Object::Dictionary(states)) => {
                        for (_, s) in states.iter() {
                            self.mark_object(s, 0);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Walk every page. `cancel` is polled between pages.
pub(super) fn collect(
    doc: &Document,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Usage, crate::error::ConvertError> {
    let mut walker = Walker::new(doc);
    for page in doc.page_iter() {
        if cancelled() {
            return Err(crate::error::ConvertError::cancelled());
        }
        let resources = page_resources(doc, page);
        let unit = doc
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"UserUnit").ok())
            .and_then(number)
            .filter(|u| *u > 0.0)
            .unwrap_or(1.0);
        let ctm = [unit, 0.0, 0.0, unit, 0.0, 0.0];
        let bytes = doc.get_page_content_with_limit(page, walker.limit()).ok();
        match walker.parse(bytes) {
            Some(content) => walker.walk(&content, resources, ctm),
            None => walker.mark_unknown(resources, 0),
        }
        walker.mark_annotations(page);
        if walker.usage.incomplete {
            break;
        }
    }
    Ok(walker.usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn matrix_product_scales_and_translates() {
        let scale = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let shift = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        // Scale first, then move.
        assert_eq!(mul(&scale, &shift), [2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
        // Move first, then scale: the move is scaled too.
        assert_eq!(mul(&shift, &scale), [2.0, 0.0, 0.0, 3.0, 20.0, 60.0]);
    }

    fn image(doc: &mut Document) -> ObjectId {
        doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 10, "Height" => 10 },
            vec![0; 300],
        ))
    }

    /// One page with `content`, `resources` on the page tree and `page_extra`
    /// entries on the page.
    fn one_page(doc: &mut Document, content: &[u8], resources: Dictionary, page_extra: Dictionary) {
        let pages_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let mut page =
            dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => content };
        for (k, v) in page_extra.iter() {
            page.set(k.clone(), v.clone());
        }
        let page = doc.add_object(page);
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
                "Resources" => resources,
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
    }

    /// Image drawn once directly and once through a form scaled ×2.
    #[test]
    fn largest_placement_through_forms() {
        let mut doc = Document::with_version("1.5");
        let img = image(&mut doc);
        let form = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "Matrix" => vec![2.into(), 0.into(), 0.into(), 2.into(), 0.into(), 0.into()],
                "Resources" => dictionary! { "XObject" => dictionary! { "I" => img } },
            },
            b"q 50 0 0 40 0 0 cm /I Do Q".to_vec(),
        ));
        one_page(
            &mut doc,
            b"q 100 0 0 30 0 0 cm /Im Do Q q 1.5 0 0 1.5 0 0 cm /F Do Q /F Do",
            dictionary! { "XObject" => dictionary! { "Im" => img, "F" => form } },
            dictionary! {},
        );

        let mut walker = Walker::new(&doc);
        let page = doc.page_iter().next().unwrap();
        let bytes = Some(doc.get_page_content(page));
        let content = walker.parse(bytes).unwrap();
        walker.walk(&content, page_resources(&doc, page), IDENTITY);
        // The form is drawn twice but decoded once.
        assert_eq!(walker.form_cache.len(), 1);

        let usage = collect(&doc, || false).unwrap();
        let (w, h) = usage.placed[&img];
        // Direct: 100×30. Through the form: 50×40 × 2 × 1.5 = 150×120.
        assert!(
            (w - 150.0).abs() < 1e-3 && (h - 120.0).abs() < 1e-3,
            "{w}×{h}"
        );
        assert!(usage.unknown.is_empty() && !usage.incomplete);
    }

    /// Drawn directly, but also from a tiling pattern / an annotation
    /// appearance: the size is unknown.
    #[test]
    fn patterns_and_annotations_make_sizes_unknown() {
        for via in ["pattern", "annotation"] {
            let mut doc = Document::with_version("1.5");
            let img = image(&mut doc);
            let inner = dictionary! { "XObject" => dictionary! { "I" => img } };
            let drawer = doc.add_object(Stream::new(
                dictionary! { "Type" => "Pattern", "PatternType" => 1, "Resources" => inner },
                b"/I Do".to_vec(),
            ));
            let mut resources = dictionary! { "XObject" => dictionary! { "Im" => img } };
            let mut extra = dictionary! {};
            if via == "pattern" {
                resources.set("Pattern", dictionary! { "P0" => drawer });
            } else {
                extra.set(
                    "Annots",
                    vec![Object::Dictionary(dictionary! {
                        "Type" => "Annot", "Subtype" => "Stamp",
                        "AP" => dictionary! { "N" => drawer },
                    })],
                );
            }
            one_page(&mut doc, b"q 100 0 0 30 0 0 cm /Im Do Q", resources, extra);
            let usage = collect(&doc, || false).unwrap();
            assert!(usage.placed.contains_key(&img), "{via}");
            assert!(usage.unknown.contains(&img), "{via}");
        }
    }

    /// Unreadable page content with a form that lists itself: marking stops.
    #[test]
    fn mark_unknown_survives_cycles() {
        let mut doc = Document::with_version("1.5");
        let img = image(&mut doc);
        let form_id = doc.new_object_id();
        let form = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "Resources" => dictionary! { "XObject" => dictionary! { "F" => form_id, "I" => img } },
            },
            b"/F Do /I Do".to_vec(),
        );
        doc.objects.insert(form_id, Object::Stream(form));
        let resources = dictionary! { "XObject" => dictionary! { "F" => form_id } };
        let mut walker = Walker::new(&doc);
        walker.mark_unknown(Some(&resources), 0);
        assert!(walker.usage.unknown.contains(&img));
        drop(walker);
        // The self-referencing form also walks without looping.
        one_page(&mut doc, b"/F Do", resources, dictionary! {});
        let usage = collect(&doc, || false).unwrap();
        assert!(usage.placed.contains_key(&img));
    }
}
