// SPDX-License-Identifier: GPL-3.0-or-later
//! Page ranges like `1-3,7,10-` (SPEC §4.2, §11). Pages are 1-based, as users count them.

use crate::error::ConvertError;

/// An inclusive run of pages, 1-based: `first..=last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub first: u32,
    pub last: u32,
}

impl PageRange {
    pub fn new(first: u32, last: u32) -> Self {
        PageRange { first, last }
    }

    pub fn len(&self) -> u32 {
        self.last - self.first + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// 0-based page indexes, for PDFium (`PdfPageIndex` is `i32`; cast at the call).
    pub fn indexes(&self) -> std::ops::Range<u32> {
        (self.first - 1)..self.last
    }

    /// `1-3`, or `7` for a single page (used in file names: `name-p1-3.pdf`).
    pub fn label(&self) -> String {
        if self.first == self.last {
            self.first.to_string()
        } else {
            format!("{}-{}", self.first, self.last)
        }
    }
}

fn bad_range(spec: &str, why: impl ToString) -> ConvertError {
    ConvertError::unsupported(
        format!("The page range \"{spec}\" isn't valid. Use something like 1-3,7,10-."),
        why,
    )
}

/// Parse `spec` for a document with `page_count` pages. Accepted parts, comma
/// separated (spaces ignored): `7`, `1-3`, `10-` (to the end), `-5` (from the
/// start). Empty or `all` = every page. Parts are kept in the order written
/// (so they can overlap or go backwards; [`unique_pages`] flattens them).
/// Ranges are clipped to the document; a part entirely past the end is an error.
pub fn parse_ranges(spec: &str, page_count: u32) -> Result<Vec<PageRange>, ConvertError> {
    if page_count == 0 {
        return Err(ConvertError::unsupported(
            "This PDF has no pages.",
            "page count 0",
        ));
    }
    let compact: String = spec.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() || compact.eq_ignore_ascii_case("all") {
        return Ok(vec![PageRange::new(1, page_count)]);
    }
    let number = |s: &str| -> Result<u32, ConvertError> {
        match s.parse::<u32>() {
            Ok(0) => Err(bad_range(spec, "pages start at 1")),
            Ok(n) => Ok(n),
            Err(e) => Err(bad_range(spec, format!("'{s}': {e}"))),
        }
    };
    let mut out = Vec::new();
    for part in compact.split(',') {
        if part.is_empty() {
            continue;
        }
        let (first, last) = match part.split_once('-') {
            None => {
                let n = number(part)?;
                (n, n)
            }
            Some(("", "")) => return Err(bad_range(spec, "'-' alone")),
            Some(("", b)) => (1, number(b)?),
            Some((a, "")) => (number(a)?, page_count),
            Some((a, b)) => (number(a)?, number(b)?),
        };
        // Past-the-end first: for 3 pages, `4-` means `4-3`, which is past
        // the end, not backwards.
        if first > page_count {
            return Err(ConvertError::unsupported(
                format!(
                    "This PDF has only {page_count} page{}, so \"{part}\" is past the end.",
                    if page_count == 1 { "" } else { "s" }
                ),
                format!("range {part} with {page_count} pages"),
            ));
        }
        if first > last {
            return Err(bad_range(spec, format!("'{part}' goes backwards")));
        }
        out.push(PageRange::new(first, last.min(page_count)));
    }
    if out.is_empty() {
        return Err(bad_range(spec, "no pages"));
    }
    Ok(out)
}

/// Every page in `ranges`, in order, each once (1-based).
pub fn unique_pages(ranges: &[PageRange]) -> Vec<u32> {
    let mut seen = std::collections::HashSet::new();
    ranges
        .iter()
        .flat_map(|r| r.first..=r.last)
        .filter(|p| seen.insert(*p))
        .collect()
}

/// Consecutive chunks of `n` pages: `1-n`, `n+1-2n`, …, last one shorter.
pub fn chunks(page_count: u32, n: u32) -> Vec<PageRange> {
    let n = n.max(1);
    (0..page_count.div_ceil(n))
        .map(|i| PageRange::new(i * n + 1, ((i + 1) * n).min(page_count)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(first: u32, last: u32) -> PageRange {
        PageRange::new(first, last)
    }

    #[test]
    fn spec_example() {
        assert_eq!(
            parse_ranges("1-3,7,10-", 12).unwrap(),
            [r(1, 3), r(7, 7), r(10, 12)]
        );
        assert_eq!(parse_ranges(" 1 - 3 , 7 ", 12).unwrap(), [r(1, 3), r(7, 7)]);
    }

    #[test]
    fn empty_and_all_mean_every_page() {
        assert_eq!(parse_ranges("", 5).unwrap(), [r(1, 5)]);
        assert_eq!(parse_ranges("All", 5).unwrap(), [r(1, 5)]);
        assert_eq!(parse_ranges("-2", 5).unwrap(), [r(1, 2)]);
    }

    #[test]
    fn clipped_and_errors() {
        assert_eq!(parse_ranges("4-99", 5).unwrap(), [r(4, 5)]);
        for bad in ["0", "3-1", "a", "1-2-3", "-", ",", "6", "6-"] {
            assert!(parse_ranges(bad, 5).is_err(), "{bad}");
        }
        assert!(parse_ranges("1", 0).is_err());
    }

    #[test]
    fn open_range_past_the_end_says_so() {
        for bad in ["4-", "4", "4-9"] {
            let msg = parse_ranges(bad, 3).unwrap_err().message;
            assert!(msg.contains("only 3 pages"), "{bad}: {msg}");
        }
        let msg = parse_ranges("3-1", 3).unwrap_err().message;
        assert!(msg.contains("isn't valid"), "{msg}");
    }

    #[test]
    fn unique_pages_and_chunks() {
        assert_eq!(unique_pages(&[r(3, 4), r(1, 3)]), [3, 4, 1, 2]);
        assert_eq!(chunks(7, 3), [r(1, 3), r(4, 6), r(7, 7)]);
        assert_eq!(chunks(2, 0), [r(1, 1), r(2, 2)]);
        assert_eq!(r(4, 4).label(), "4");
        assert_eq!(r(1, 3).label(), "1-3");
        assert_eq!(r(2, 3).indexes(), 1..3);
    }
}
