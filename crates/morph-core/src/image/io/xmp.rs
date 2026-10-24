// SPDX-License-Identifier: GPL-3.0-or-later
//! Small, byte-preserving XMP edits: remove GPS properties, reset the orientation.
//!
//! The packet is parsed with roxmltree only to find byte ranges; edits are applied
//! to the original text, so everything else stays exactly as it was.

use std::ops::Range;

/// EXIF namespace in XMP (holds `exif:GPSLatitude` etc.).
const NS_EXIF: &str = "http://ns.adobe.com/exif/1.0/";
/// CIPA "EXIF 2.3+ for XMP" namespace, which may hold GPS properties too.
const NS_EXIF_EX: &str = "http://cipa.jp/exif/1.0/";
const NS_TIFF: &str = "http://ns.adobe.com/tiff/1.0/";

const NS_PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
const NS_IPTC_CORE: &str = "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/";
const NS_IPTC_EXT: &str = "http://iptc.org/std/Iptc4xmpExt/2008-02-29/";
/// Google motion photos: point at a video appended after the JPEG (which has
/// its own GPS and is cut off when location is removed).
const NS_GCAMERA: &str = "http://ns.google.com/photos/1.0/camera/";
const NS_GCONTAINER: &str = "http://ns.google.com/photos/1.0/container/";

/// Properties that reveal where a photo was taken: any `GPS*`/`Gps*` name in
/// any namespace (EXIF, CIPA, DJI drones, …), IPTC/Photoshop place names, and
/// motion-photo pointers to the (removed) appended video.
fn is_gps(ns: Option<&str>, name: &str) -> bool {
    if name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("gps")) {
        return true;
    }
    match ns {
        Some(NS_EXIF | NS_EXIF_EX) => false,
        Some(NS_PHOTOSHOP) => matches!(name, "City" | "State" | "Country"),
        Some(NS_IPTC_CORE) => matches!(name, "Location" | "CountryCode"),
        Some(NS_IPTC_EXT) => matches!(name, "LocationCreated" | "LocationShown"),
        Some(NS_GCAMERA) => name.starts_with("MicroVideo") || name.starts_with("MotionPhoto"),
        Some(NS_GCONTAINER) => name == "Directory",
        _ => false,
    }
}

/// The XML text of a packet: UTF-8, without a BOM or trailing NUL padding.
/// Returns (start offset, text).
fn text(xmp: &[u8]) -> Option<(usize, &str)> {
    let start = if xmp.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    let end = xmp.iter().rposition(|&b| b != 0).map_or(start, |p| p + 1);
    let s = std::str::from_utf8(xmp.get(start..end)?).ok()?;
    Some((start, s))
}

fn parse(s: &str) -> Option<roxmltree::Document<'_>> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(s, opts).ok()
}

/// Apply non-overlapping `(range, replacement)` edits (ranges relative to `base`).
fn apply(xmp: &[u8], base: usize, mut edits: Vec<(Range<usize>, &[u8])>) -> Vec<u8> {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = Vec::with_capacity(xmp.len());
    let mut pos = 0;
    for (r, rep) in edits {
        let (s, e) = (base + r.start, base + r.end);
        if s < pos {
            continue; // overlaps an earlier edit (already removed)
        }
        out.extend_from_slice(&xmp[pos..s]);
        out.extend_from_slice(rep);
        pos = e;
    }
    out.extend_from_slice(&xmp[pos..]);
    out
}

/// XMP without GPS properties (see [`super::strip_xmp_gps`]).
pub(crate) fn strip_gps(xmp: &[u8]) -> Option<Vec<u8>> {
    let (base, s) = text(xmp)?;
    let doc = parse(s)?;
    let mut edits: Vec<(Range<usize>, &[u8])> = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(node) = stack.pop() {
        if node.is_element() {
            let tag = node.tag_name();
            if is_gps(tag.namespace(), tag.name()) {
                edits.push((node.range(), b""));
                continue; // the whole element goes, children included
            }
            for a in node.attributes() {
                if is_gps(a.namespace(), a.name()) {
                    edits.push((a.range(), b""));
                }
            }
        }
        stack.extend(node.children());
    }
    if edits.is_empty() {
        return Some(xmp.to_vec());
    }
    let out = apply(xmp, base, edits);
    // The result must still be valid XML; otherwise nothing is kept.
    let (_, s) = text(&out)?;
    parse(s)?;
    Some(out)
}

/// XMP with `tiff:Orientation` set to 1. Unparseable XMP is returned unchanged.
pub(crate) fn with_orientation_reset(xmp: &[u8]) -> Vec<u8> {
    let Some((base, s)) = text(xmp) else {
        return xmp.to_vec();
    };
    let Some(doc) = parse(s) else {
        return xmp.to_vec();
    };
    let is_orientation =
        |ns: Option<&str>, name: &str| ns == Some(NS_TIFF) && name == "Orientation";
    let mut edits: Vec<(Range<usize>, &[u8])> = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        for a in node.attributes() {
            if is_orientation(a.namespace(), a.name()) && a.value().trim() != "1" {
                edits.push((a.range_value(), b"1"));
            }
        }
        let tag = node.tag_name();
        if is_orientation(tag.namespace(), tag.name()) {
            let texts: Vec<_> = node.children().filter(|c| c.is_text()).collect();
            if let [t] = texts.as_slice()
                && t.text().is_some_and(|v| v.trim() != "1")
            {
                edits.push((t.range(), b"1"));
            }
        }
    }
    if edits.is_empty() {
        return xmp.to_vec();
    }
    apply(xmp, base, edits)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const XMP_WITH_GPS: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:e="http://ns.adobe.com/exif/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    e:GPSLatitude="48,51.5N" e:GPSLongitude="2,17.4E" e:ExposureTime="1/100"
    tiff:Orientation="6">
   <e:GPSAltitude>35/1</e:GPSAltitude>
   <exif:GPSTimeStamp xmlns:exif="http://ns.adobe.com/exif/1.0/">2020-01-01T10:00:00Z</exif:GPSTimeStamp>
   <e:UserComment><rdf:Alt><rdf:li xml:lang="x-default">hello</rdf:li></rdf:Alt></e:UserComment>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn removes_drone_gps_and_iptc_places() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:Iptc4xmpCore="http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"
    xmlns:Iptc4xmpExt="http://iptc.org/std/Iptc4xmpExt/2008-02-29/"
    xmlns:GCamera="http://ns.google.com/photos/1.0/camera/"
    drone-dji:GpsLatitude="48.85" drone-dji:GpsLongitude="2.29"
    drone-dji:FlightYawDegree="12" photoshop:City="Paris" photoshop:Country="France"
    photoshop:Credit="Me" Iptc4xmpCore:Location="Tour Eiffel"
    GCamera:MicroVideo="1" GCamera:MicroVideoOffset="12345">
   <Iptc4xmpExt:LocationCreated><rdf:Bag><rdf:li Iptc4xmpExt:Sublocation="Champ de Mars"/></rdf:Bag></Iptc4xmpExt:LocationCreated>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let Some(out) = strip_gps(xmp.as_bytes()) else {
            panic!("strip failed")
        };
        let s = String::from_utf8(out).unwrap_or_default();
        for gone in [
            "48.85",
            "Paris",
            "France",
            "Tour Eiffel",
            "Champ de Mars",
            "12345",
        ] {
            assert!(!s.contains(gone), "kept {gone}: {s}");
        }
        assert!(
            s.contains("FlightYawDegree") && s.contains("photoshop:Credit"),
            "{s}"
        );
    }

    #[test]
    fn removes_gps_attributes_and_elements_with_any_prefix() {
        let Some(out) = strip_gps(XMP_WITH_GPS.as_bytes()) else {
            panic!("strip failed")
        };
        let s = String::from_utf8(out).unwrap_or_default();
        assert!(!s.contains("GPS"), "{s}");
        assert!(!s.contains("48,51.5N") && !s.contains("35/1"), "{s}");
        for kept in [
            "e:ExposureTime=\"1/100\"",
            "hello",
            "tiff:Orientation",
            "xpacket end",
        ] {
            assert!(s.contains(kept), "lost {kept}: {s}");
        }
    }

    #[test]
    fn broken_xmp_is_dropped() {
        assert_eq!(strip_gps(b"<x:xmpmeta><unclosed"), None);
        assert_eq!(strip_gps(&[0xFF, 0xFE, 0x00]), None);
        // Without GPS: unchanged; trailing NUL padding is tolerated.
        let plain = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>\0\0";
        assert_eq!(strip_gps(plain).as_deref(), Some(&plain[..]));
    }

    #[test]
    fn orientation_reset_in_both_forms() {
        let out = with_orientation_reset(XMP_WITH_GPS.as_bytes());
        let s = String::from_utf8(out).unwrap_or_default();
        assert!(s.contains("tiff:Orientation=\"1\""), "{s}");
        let elem =
            br#"<r xmlns:t="http://ns.adobe.com/tiff/1.0/"><t:Orientation> 8 </t:Orientation></r>"#;
        let out = with_orientation_reset(elem);
        assert_eq!(
            out,
            br#"<r xmlns:t="http://ns.adobe.com/tiff/1.0/"><t:Orientation>1</t:Orientation></r>"#
        );
        assert_eq!(with_orientation_reset(b"junk"), b"junk");
    }
}
