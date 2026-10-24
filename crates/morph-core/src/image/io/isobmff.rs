// SPDX-License-Identifier: GPL-3.0-or-later
//! Add an ICC profile (`colr` box of type `prof`) to an AVIF file written by
//! ravif / avif-serialize, which can't write one itself.
//!
//! The box is appended to `ipco` and associated with the primary item in `ipma`;
//! box sizes and `iloc` offsets are fixed up. Layouts we don't expect make the
//! function give up (`None`) rather than risk a broken file.

/// A box: start offset, header length, total length, type.
#[derive(Debug, Clone, Copy)]
struct Bx {
    start: usize,
    header: usize,
    len: usize,
    kind: [u8; 4],
}

impl Bx {
    fn body(&self) -> std::ops::Range<usize> {
        self.start + self.header..self.start + self.len
    }
    fn end(&self) -> usize {
        self.start + self.len
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// Child boxes in `range` (32-bit sizes only).
fn boxes(b: &[u8], range: std::ops::Range<usize>) -> Option<Vec<Bx>> {
    let mut out = Vec::new();
    let mut p = range.start;
    while p < range.end {
        let len = be32(b, p)? as usize;
        if len < 8 || p + len > range.end {
            return None;
        }
        let kind: [u8; 4] = b.get(p + 4..p + 8)?.try_into().ok()?;
        out.push(Bx {
            start: p,
            header: 8,
            len,
            kind,
        });
        p += len;
    }
    Some(out)
}

fn find(list: &[Bx], kind: &[u8; 4]) -> Option<Bx> {
    list.iter().find(|x| &x.kind == kind).copied()
}

/// Read a big-endian number of `size` bytes (0, 4 or 8).
fn read_n(b: &[u8], at: usize, size: usize) -> Option<u64> {
    match size {
        0 => Some(0),
        4 => be32(b, at).map(u64::from),
        8 => Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?)),
        _ => None,
    }
}

fn write_n(b: &mut [u8], at: usize, size: usize, v: u64) -> Option<()> {
    match size {
        4 => b
            .get_mut(at..at + 4)?
            .copy_from_slice(&u32::try_from(v).ok()?.to_be_bytes()),
        8 => b.get_mut(at..at + 8)?.copy_from_slice(&v.to_be_bytes()),
        _ => return None,
    }
    Some(())
}

/// Positions of the `iloc` offset fields that point into the file (to shift).
/// Returns (field position, field size).
fn iloc_offset_fields(b: &[u8], iloc: Bx, moved_from: u64) -> Option<Vec<(usize, usize)>> {
    let mut p = iloc.body().start;
    let version = *b.get(p)?;
    p += 4;
    let sizes = *b.get(p)?;
    let (offset_size, length_size) = (usize::from(sizes >> 4), usize::from(sizes & 15));
    let sizes2 = *b.get(p + 1)?;
    let base_size = usize::from(sizes2 >> 4);
    let index_size = if version >= 1 {
        usize::from(sizes2 & 15)
    } else {
        0
    };
    p += 2;
    let count = if version < 2 {
        p += 2;
        usize::from(be16(b, p - 2)?)
    } else {
        p += 4;
        be32(b, p - 4)? as usize
    };
    let mut fields = Vec::new();
    for _ in 0..count {
        p += if version < 2 { 2 } else { 4 }; // item id
        let method = if version >= 1 {
            p += 2;
            be16(b, p - 2)? & 15
        } else {
            0
        };
        let data_ref = be16(b, p)?;
        p += 2;
        let base = read_n(b, p, base_size)?;
        let base_pos = p;
        p += base_size;
        let extents = usize::from(be16(b, p)?);
        p += 2;
        let in_file = method == 0 && data_ref == 0;
        if in_file && base_size > 0 && base >= moved_from {
            fields.push((base_pos, base_size));
        }
        for _ in 0..extents {
            p += index_size;
            let off = read_n(b, p, offset_size)?;
            if in_file && base_size == 0 && off >= moved_from {
                fields.push((p, offset_size));
            } else if in_file && base_size > 0 && base < moved_from && base + off >= moved_from {
                return None; // mixed addressing: not worth handling
            }
            p += offset_size + length_size;
        }
    }
    (p <= iloc.end()).then_some(fields)
}

/// The AVIF with `icc` attached to its primary image, or `None` if the file
/// layout isn't one we can safely edit.
pub(crate) fn avif_with_icc(file: &[u8], icc: &[u8]) -> Option<Vec<u8>> {
    let top = boxes(file, 0..file.len())?;
    let meta = find(&top, b"meta")?;
    let meta_children = boxes(file, meta.body().start + 4..meta.end())?;
    let pitm = find(&meta_children, b"pitm")?;
    let primary = if *file.get(pitm.body().start)? == 0 {
        u32::from(be16(file, pitm.body().start + 4)?)
    } else {
        be32(file, pitm.body().start + 4)?
    };
    let iloc = find(&meta_children, b"iloc")?;
    let iprp = find(&meta_children, b"iprp")?;
    let iprp_children = boxes(file, iprp.body())?;
    let ipco = find(&iprp_children, b"ipco")?;
    let ipma = find(&iprp_children, b"ipma")?;
    // Exactly one ipma, after ipco (as avif-serialize writes it).
    if iprp_children.iter().filter(|x| &x.kind == b"ipma").count() != 1 || ipma.start < ipco.end() {
        return None;
    }
    let props = boxes(file, ipco.body())?;
    if props.iter().any(|p| {
        &p.kind == b"colr" && file.get(p.body().start..p.body().start + 4) == Some(b"prof")
    }) {
        return Some(file.to_vec()); // already has one
    }
    let prop_index = u8::try_from(props.len() + 1).ok().filter(|&i| i < 0x80)?;

    // Find the primary item's entry in ipma (1-byte associations only).
    let ipma_body = ipma.body().start;
    let version = *file.get(ipma_body)?;
    let flags = be32(file, ipma_body)? & 0x00FF_FFFF;
    if flags & 1 != 0 {
        return None;
    }
    let mut p = ipma_body + 4;
    let entries = be32(file, p)?;
    p += 4;
    let mut assoc_count_pos = None;
    let mut insert_at = 0;
    for _ in 0..entries {
        let id = if version < 1 {
            p += 2;
            u32::from(be16(file, p - 2)?)
        } else {
            p += 4;
            be32(file, p - 4)?
        };
        let n = *file.get(p)?;
        if id == primary {
            assoc_count_pos = Some(p);
            insert_at = p + 1 + usize::from(n);
            if n == 0xFF {
                return None;
            }
        }
        p += 1 + usize::from(n);
    }
    let assoc_count_pos = assoc_count_pos?;
    if p > ipma.end() {
        return None;
    }

    let mut colr = Vec::with_capacity(12 + icc.len());
    colr.extend_from_slice(&u32::try_from(12 + icc.len()).ok()?.to_be_bytes());
    colr.extend_from_slice(b"colr");
    colr.extend_from_slice(b"prof");
    colr.extend_from_slice(icc);
    let delta = colr.len() + 1;

    // Data after `meta` moves by `delta`: fix iloc first (positions unchanged so far).
    let mut out = file.to_vec();
    for (pos, size) in iloc_offset_fields(file, iloc, meta.end() as u64)? {
        let v = read_n(&out, pos, size)?;
        write_n(&mut out, pos, size, v + delta as u64)?;
    }
    // Sizes of the enclosing boxes (all start before both insertion points).
    let grow = |out: &mut Vec<u8>, at: usize, by: usize| -> Option<()> {
        let v = be32(out, at)? as usize + by;
        write_n(out, at, 4, v as u64)
    };
    grow(&mut out, meta.start, delta)?;
    grow(&mut out, iprp.start, delta)?;
    grow(&mut out, ipco.start, colr.len())?;
    grow(&mut out, ipma.start, 1)?;
    *out.get_mut(assoc_count_pos)? += 1;
    // Insert the later position first so the earlier one stays valid.
    out.insert(insert_at, prop_index);
    out.splice(ipco.end()..ipco.end(), colr);
    Some(out)
}
