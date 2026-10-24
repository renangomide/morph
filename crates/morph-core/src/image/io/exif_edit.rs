// SPDX-License-Identifier: GPL-3.0-or-later
//! Small in-place edits of raw EXIF (a TIFF structure, no "Exif\0\0" prefix).
//!
//! Edits never move data, so every other offset in the blob (maker notes,
//! thumbnails, sub-IFDs) stays valid.

const TAG_ORIENTATION: u16 = 0x0112;
const TAG_GPS_IFD: u16 = 0x8825;

/// Byte order of a TIFF structure.
#[derive(Clone, Copy)]
struct Tiff {
    little: bool,
}

impl Tiff {
    fn parse(data: &[u8]) -> Option<(Tiff, usize)> {
        let little = match data.get(0..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        let t = Tiff { little };
        let ifd0 = t.u32(data, 4)? as usize;
        Some((t, ifd0))
    }

    fn u16(self, d: &[u8], at: usize) -> Option<u16> {
        let b: [u8; 2] = d.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(self, d: &[u8], at: usize) -> Option<u32> {
        let b: [u8; 4] = d.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn put_u16(self, d: &mut [u8], at: usize, v: u16) -> Option<()> {
        let b = if self.little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        d.get_mut(at..at + 2)?.copy_from_slice(&b);
        Some(())
    }

    fn put_u32(self, d: &mut [u8], at: usize, v: u32) -> Option<()> {
        let b = if self.little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        d.get_mut(at..at + 4)?.copy_from_slice(&b);
        Some(())
    }
}

/// One 12-byte IFD entry.
struct Entry {
    /// Position of the entry in the blob.
    pos: usize,
    tag: u16,
    typ: u16,
    count: u32,
}

impl Entry {
    /// Size of the value in bytes (`None` for unknown types).
    fn value_len(&self) -> Option<usize> {
        let unit = match self.typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        (self.count as usize).checked_mul(unit)
    }
}

/// Entries of the IFD at `offset`, or `None` if it doesn't fit in `data`.
fn read_ifd(t: Tiff, data: &[u8], offset: usize) -> Option<Vec<Entry>> {
    let n = t.u16(data, offset)? as usize;
    let end = offset.checked_add(2 + n * 12 + 4)?;
    if end > data.len() {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let pos = offset + 2 + i * 12;
        out.push(Entry {
            pos,
            tag: t.u16(data, pos)?,
            typ: t.u16(data, pos + 2)?,
            count: t.u32(data, pos + 4)?,
        });
    }
    Some(out)
}

/// The ICC profile (tag 34675) of the first IFD of a TIFF file, if any.
/// (The `image` TIFF decoder doesn't return it.)
pub(crate) fn tiff_icc(tiff: &[u8]) -> Option<Vec<u8>> {
    const TAG_ICC: u16 = 34675;
    let (t, ifd0) = Tiff::parse(tiff)?;
    let e = read_ifd(t, tiff, ifd0)?
        .into_iter()
        .find(|e| e.tag == TAG_ICC && matches!(e.typ, 1 | 7))?;
    let len = e.value_len()?;
    let start = if len <= 4 {
        e.pos + 8
    } else {
        t.u32(tiff, e.pos + 8)? as usize
    };
    tiff.get(start..start.checked_add(len)?)
        .map(<[u8]>::to_vec)
        .filter(|v| !v.is_empty())
}

/// Strip an "Exif\0\0" prefix if present.
pub(crate) fn without_prefix(exif: &[u8]) -> &[u8] {
    exif.strip_prefix(b"Exif\0\0").unwrap_or(exif)
}

/// Is this a parseable TIFF/EXIF structure (header + first IFD)?
pub(crate) fn is_valid(exif: &[u8]) -> bool {
    Tiff::parse(exif).is_some_and(|(t, ifd0)| read_ifd(t, exif, ifd0).is_some())
}

/// Orientation tag value of IFD0 (1–8), if present.
pub(crate) fn orientation(exif: &[u8]) -> Option<u8> {
    let (t, ifd0) = Tiff::parse(exif)?;
    let e = read_ifd(t, exif, ifd0)?
        .into_iter()
        .find(|e| e.tag == TAG_ORIENTATION)?;
    let v = match e.typ {
        3 => u32::from(t.u16(exif, e.pos + 8)?),
        4 => t.u32(exif, e.pos + 8)?,
        _ => return None,
    };
    u8::try_from(v).ok().filter(|v| (1..=8).contains(v))
}

/// Set the IFD0 orientation tag to 1 (if present). Returns the edited copy.
/// Unparseable EXIF is returned unchanged.
pub(crate) fn with_orientation_reset(exif: &[u8]) -> Vec<u8> {
    let mut out = exif.to_vec();
    let Some((t, ifd0)) = Tiff::parse(&out) else {
        return out;
    };
    let Some(entries) = read_ifd(t, &out, ifd0) else {
        return out;
    };
    for e in entries.iter().filter(|e| e.tag == TAG_ORIENTATION) {
        // Value fits in the entry's 4-byte value field.
        let _ = match e.typ {
            3 => t
                .put_u16(&mut out, e.pos + 8, 1)
                .and_then(|_| t.put_u16(&mut out, e.pos + 10, 0)),
            4 => t.put_u32(&mut out, e.pos + 8, 1),
            _ => None,
        };
    }
    out
}

/// EXIF without the GPS IFD: the GPS data is zeroed in place and the pointer
/// to it removed from IFD0. `None` if the EXIF can't be parsed.
pub(crate) fn strip_gps(exif: &[u8]) -> Option<Vec<u8>> {
    let exif = without_prefix(exif);
    let (t, ifd0) = Tiff::parse(exif)?;
    let entries = read_ifd(t, exif, ifd0)?;
    let mut out = exif.to_vec();
    let Some(idx) = entries.iter().position(|e| e.tag == TAG_GPS_IFD) else {
        return Some(out);
    };

    // Zero the GPS IFD and the out-of-line values it points to.
    if let Some(gps_off) = t.u32(exif, entries[idx].pos + 8).map(|v| v as usize)
        && let Some(gps) = read_ifd(t, exif, gps_off)
    {
        for e in &gps {
            if let Some(len) = e.value_len()
                && len > 4
                && let Some(off) = t.u32(exif, e.pos + 8).map(|v| v as usize)
                && let Some(end) = off.checked_add(len)
                && end <= out.len()
            {
                out[off..end].fill(0);
            }
        }
        let end = gps_off + 2 + gps.len() * 12 + 4;
        out[gps_off..end].fill(0);
    }

    // Remove the pointer entry: shift later entries and the next-IFD offset up.
    let n = entries.len();
    let entry_pos = entries[idx].pos;
    let tail_end = ifd0 + 2 + n * 12 + 4;
    out.copy_within(entry_pos + 12..tail_end, entry_pos);
    out[tail_end - 12..tail_end].fill(0);
    t.put_u16(&mut out, ifd0, u16::try_from(n - 1).ok()?)?;
    Some(out)
}

#[cfg(test)]
pub(crate) mod test_data {
    /// Build a little-endian EXIF blob: IFD0 with Make, Orientation and a GPS
    /// pointer; the GPS IFD holds GPSLatitudeRef and GPSLatitude (3 rationals).
    pub(crate) fn sample_exif(orientation: u16) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"II*\0");
        d.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: 3 entries -> 2 + 36 + 4 = 42 bytes, ends at 50.
        let make_off = 50u32; // "Morph\0" (6 bytes) at 50..56
        let gps_off = 56u32;
        d.extend_from_slice(&3u16.to_le_bytes());
        // Make (ASCII, 6)
        d.extend_from_slice(&0x010Fu16.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&6u32.to_le_bytes());
        d.extend_from_slice(&make_off.to_le_bytes());
        // Orientation (SHORT, 1)
        d.extend_from_slice(&0x0112u16.to_le_bytes());
        d.extend_from_slice(&3u16.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&orientation.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        // GPS IFD pointer (LONG, 1)
        d.extend_from_slice(&0x8825u16.to_le_bytes());
        d.extend_from_slice(&4u16.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&gps_off.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        d.extend_from_slice(b"Morph\0");
        // GPS IFD at 56: 2 entries -> 30 bytes, ends at 86; latitude at 86..110.
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes()); // GPSLatitudeRef
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(b"N\0\0\0");
        d.extend_from_slice(&2u16.to_le_bytes()); // GPSLatitude
        d.extend_from_slice(&5u16.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(&86u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        for (num, den) in [(48u32, 1u32), (51, 1), (2405, 100)] {
            d.extend_from_slice(&num.to_le_bytes());
            d.extend_from_slice(&den.to_le_bytes());
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::test_data::sample_exif;
    use super::*;

    fn parse(d: &[u8]) -> ::exif::Exif {
        match ::exif::Reader::new().read_raw(d.to_vec()) {
            Ok(e) => e,
            Err(e) => panic!("kamadak-exif can't parse: {e}"),
        }
    }

    #[test]
    fn sample_is_valid_exif() {
        let d = sample_exif(6);
        let e = parse(&d);
        assert!(
            e.get_field(::exif::Tag::GPSLatitude, ::exif::In::PRIMARY)
                .is_some()
        );
        assert_eq!(orientation(&d), Some(6));
    }

    #[test]
    fn orientation_reset_to_1() {
        let d = with_orientation_reset(&sample_exif(8));
        assert_eq!(orientation(&d), Some(1));
        let e = parse(&d);
        let f = e.get_field(::exif::Tag::Orientation, ::exif::In::PRIMARY);
        assert_eq!(f.and_then(|f| f.value.get_uint(0)), Some(1));
    }

    #[test]
    fn strip_gps_keeps_other_tags() {
        let d = sample_exif(3);
        let Some(s) = strip_gps(&d) else {
            panic!("strip_gps failed")
        };
        let e = parse(&s);
        assert!(
            e.get_field(::exif::Tag::GPSLatitude, ::exif::In::PRIMARY)
                .is_none()
        );
        assert!(
            e.get_field(::exif::Tag::GPSInfoIFDPointer, ::exif::In::PRIMARY)
                .is_none()
        );
        let make = e.get_field(::exif::Tag::Make, ::exif::In::PRIMARY);
        assert_eq!(
            make.map(|f| f.display_value().to_string()),
            Some("\"Morph\"".to_string())
        );
        assert_eq!(orientation(&s), Some(3));
        // The coordinates are really gone from the bytes.
        assert!(!s.windows(4).any(|w| w == 2405u32.to_le_bytes()));
        // With an Exif prefix too.
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend_from_slice(&d);
        assert_eq!(strip_gps(&prefixed), Some(s));
    }

    #[test]
    fn strip_gps_rejects_garbage() {
        assert_eq!(strip_gps(b"not exif at all"), None);
        assert_eq!(strip_gps(b"II*\0\xff\xff\xff\x00"), None);
    }
}
