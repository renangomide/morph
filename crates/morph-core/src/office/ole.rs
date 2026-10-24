// SPDX-License-Identifier: GPL-3.0-or-later
//! Password check for old binary Office files (`.doc`, `.xls`, `.ppt`): a tiny,
//! read-only OLE/CFB reader ([MS-CFB]) that looks at the first bytes of one
//! stream. No dependency; every malformed file just reads as "not encrypted"
//! (the engine then reports it).
//!
//! - Word: `WordDocument` stream, FIB `wIdent = 0xA5EC`, `fEncrypted` (bit 8 of
//!   the flags at offset 0x0A) [MS-DOC 2.5.2].
//! - PowerPoint: `Current User` stream, `CurrentUserAtom.headerToken =
//!   0xF3D1C4DF` (encrypted) [MS-PPT 2.3.2].
//! - Excel: a `FILEPASS` record (0x002F) in the workbook globals [MS-XLS 2.4.117].
//!   Excel also uses it for files only "protected" with the built-in
//!   `VelvetSweatshop` password, which open without asking, so this one is only
//!   [`Encryption::Maybe`]: used to explain an engine failure, never to refuse
//!   a file up front.

use std::path::Path;

pub(crate) const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Files bigger than this aren't scanned (the check is a shortcut, not a gate).
const SCAN_LIMIT: u64 = 256 * 1024 * 1024;
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const MAX_SECT: u32 = 0xFFFF_FFFA;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encryption {
    /// Certainly needs a password (Word, PowerPoint).
    Yes,
    /// Encrypted, but possibly with Excel's built-in default password.
    Maybe,
}

/// Is this old binary Office file encrypted? `None` = no, not OLE, or unreadable.
pub(crate) fn legacy_encryption(path: &Path) -> Option<Encryption> {
    if std::fs::metadata(path).ok()?.len() > SCAN_LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    legacy_encryption_of(&bytes)
}

pub(crate) fn legacy_encryption_of(bytes: &[u8]) -> Option<Encryption> {
    let cfb = Cfb::parse(bytes)?;
    if let Some(fib) = cfb.stream_prefix("WordDocument", 12) {
        let encrypted =
            u16_at(&fib, 0)? == 0xA5EC && u16_at(&fib, 10).is_some_and(|f| f & 0x0100 != 0);
        return encrypted.then_some(Encryption::Yes);
    }
    if let Some(atom) = cfb.stream_prefix("Current User", 16) {
        return (u32_at(&atom, 12)? == 0xF3D1_C4DF).then_some(Encryption::Yes);
    }
    for name in ["Workbook", "Book"] {
        if let Some(wb) = cfb.stream_prefix(name, 64 * 1024) {
            return has_filepass(&wb).then_some(Encryption::Maybe);
        }
    }
    None
}

/// BIFF records of the globals substream, up to its EOF: is FILEPASS there?
fn has_filepass(stream: &[u8]) -> bool {
    let mut at = 0usize;
    for _ in 0..1000 {
        let Some(head) = stream.get(at..at + 4) else {
            return false;
        };
        let kind = u16::from_le_bytes([head[0], head[1]]);
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        match kind {
            0x002F => return true,
            0x000A => return false, // EOF of the globals
            _ => at += 4 + len,
        }
    }
    false
}

struct Cfb<'a> {
    bytes: &'a [u8],
    sector: usize,
    mini_sector: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    first_minifat: u32,
    dir: Vec<u8>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

impl<'a> Cfb<'a> {
    fn parse(bytes: &'a [u8]) -> Option<Cfb<'a>> {
        if !bytes.starts_with(&OLE_MAGIC) {
            return None;
        }
        let shift = u16_at(bytes, 0x1E)?;
        let mini_shift = u16_at(bytes, 0x20)?;
        if !(shift == 9 || shift == 12) || mini_shift != 6 {
            return None;
        }
        let sector = 1usize << shift;
        let mut cfb = Cfb {
            bytes,
            sector,
            mini_sector: 1usize << mini_shift,
            mini_cutoff: u64::from(u32_at(bytes, 0x38)?),
            fat: Vec::new(),
            first_minifat: u32_at(bytes, 0x3C)?,
            dir: Vec::new(),
        };
        // FAT sectors: 109 in the header, then the DIFAT chain.
        let fat_count = u32_at(bytes, 0x2C)? as usize;
        let mut fat_sectors: Vec<u32> = (0..109)
            .filter_map(|i| u32_at(bytes, 0x4C + i * 4))
            .filter(|&s| s <= MAX_SECT)
            .collect();
        let mut difat = u32_at(bytes, 0x44)?;
        let per = sector / 4 - 1;
        let mut guard = 0;
        while difat <= MAX_SECT && fat_sectors.len() < fat_count && guard < 100_000 {
            let s = cfb.sector_bytes(difat)?;
            fat_sectors.extend(
                (0..per)
                    .filter_map(|i| u32_at(s, i * 4))
                    .filter(|&x| x <= MAX_SECT),
            );
            difat = u32_at(s, per * 4)?;
            guard += 1;
        }
        fat_sectors.truncate(fat_count);
        let mut fat = Vec::with_capacity(fat_sectors.len() * sector / 4);
        for s in fat_sectors {
            let data = cfb.sector_bytes(s)?;
            fat.extend((0..sector / 4).filter_map(|i| u32_at(data, i * 4)));
        }
        cfb.fat = fat;
        cfb.dir = cfb.read_chain(u32_at(bytes, 0x30)?, usize::MAX)?;
        Some(cfb)
    }

    fn sector_bytes(&self, n: u32) -> Option<&'a [u8]> {
        let start = (n as usize).checked_add(1)?.checked_mul(self.sector)?;
        self.bytes.get(start..start.checked_add(self.sector)?)
    }

    /// A regular sector chain, at most `limit` bytes.
    fn read_chain(&self, mut s: u32, limit: usize) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut steps = 0usize;
        while s != END_OF_CHAIN && out.len() < limit {
            if s > MAX_SECT || steps > self.fat.len() {
                return None;
            }
            out.extend_from_slice(self.sector_bytes(s)?);
            s = *self.fat.get(s as usize)?;
            steps += 1;
        }
        Some(out)
    }

    /// Up to `len` first bytes of the stream `name` (searched in every storage:
    /// these names are unique enough for the checks above).
    fn stream_prefix(&self, name: &str, len: usize) -> Option<Vec<u8>> {
        let want: Vec<u16> = name.encode_utf16().collect();
        let root = self.dir.get(..128)?;
        for e in self.dir.as_chunks::<128>().0 {
            let name_len = usize::from(u16_at(e, 64)?);
            if e[66] != 2 || !(2..=64).contains(&name_len) {
                continue; // not a stream
            }
            let chars: Vec<u16> = (0..name_len / 2 - 1)
                .filter_map(|i| u16_at(e, i * 2))
                .collect();
            if chars != want {
                continue;
            }
            let start = u32_at(e, 116)?;
            let size = u64::from(u32_at(e, 120)?);
            let take = len.min(usize::try_from(size).ok()?);
            let data = if size < self.mini_cutoff {
                self.read_mini(root, start, take)?
            } else {
                self.read_chain(start, take)?
            };
            return Some(data.get(..take)?.to_vec());
        }
        None
    }

    fn read_mini(&self, root: &[u8], mut s: u32, limit: usize) -> Option<Vec<u8>> {
        let mini_stream = self.read_chain(u32_at(root, 116)?, usize::MAX)?;
        let minifat_bytes = self.read_chain(self.first_minifat, usize::MAX)?;
        let minifat: Vec<u32> = (0..minifat_bytes.len() / 4)
            .filter_map(|i| u32_at(&minifat_bytes, i * 4))
            .collect();
        let mut out = Vec::new();
        let mut steps = 0usize;
        while s != END_OF_CHAIN && out.len() < limit {
            if s > MAX_SECT || steps > minifat.len() {
                return None;
            }
            let start = (s as usize).checked_mul(self.mini_sector)?;
            out.extend_from_slice(mini_stream.get(start..start + self.mini_sector)?);
            s = *minifat.get(s as usize)?;
            steps += 1;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn legacy_passwords_are_spotted() {
        assert_eq!(
            legacy_encryption(&fixture("password.doc")),
            Some(Encryption::Yes)
        );
        assert_eq!(
            legacy_encryption(&fixture("password.xls")),
            Some(Encryption::Maybe)
        );
        // PowerPoint: LibreOffice can't write an encrypted .ppt, so build the
        // stream by hand: CurrentUserAtom header + size + headerToken.
        let atom = |token: u32| {
            let mut s = vec![0, 0, 0xF6, 0x0F, 0x14, 0, 0, 0, 0x14, 0, 0, 0];
            s.extend(token.to_le_bytes());
            s.resize(4096, 0);
            one_stream_cfb("Current User", &s)
        };
        assert_eq!(
            legacy_encryption_of(&atom(0xF3D1_C4DF)),
            Some(Encryption::Yes)
        );
        assert_eq!(legacy_encryption_of(&atom(0xE391_C05F)), None);
    }

    /// A v3 OLE file: sector 0 = FAT, 1 = directory, 2.. = one regular stream
    /// (at least 4096 bytes, so not in the mini stream).
    fn one_stream_cfb(name: &str, data: &[u8]) -> Vec<u8> {
        let sectors = data.len().div_ceil(512) as u32;
        let mut b = OLE_MAGIC.to_vec();
        b.resize(512, 0);
        b[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
        b[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
        b[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
        b[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
        b[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
        b[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes()); // FAT sectors
        b[0x30..0x34].copy_from_slice(&1u32.to_le_bytes()); // directory
        b[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        b[0x44..0x48].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        for i in 0..109 {
            let v = if i == 0 { 0 } else { 0xFFFF_FFFF };
            b[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&u32::to_le_bytes(v));
        }
        let mut fat = vec![0xFFFF_FFFDu32, END_OF_CHAIN];
        fat.extend((0..sectors).map(|i| {
            if i + 1 == sectors {
                END_OF_CHAIN
            } else {
                i + 3
            }
        }));
        fat.resize(128, 0xFFFF_FFFF);
        b.extend(fat.iter().flat_map(|v| v.to_le_bytes()));
        let entry = |name: &str, kind: u8, start: u32, size: u32| {
            let mut e = vec![0u8; 128];
            let n: Vec<u8> = name
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect();
            e[..n.len()].copy_from_slice(&n);
            e[64..66].copy_from_slice(&(n.len() as u16).to_le_bytes());
            e[66] = kind;
            e[116..120].copy_from_slice(&start.to_le_bytes());
            e[120..124].copy_from_slice(&size.to_le_bytes());
            e
        };
        b.extend(entry("Root Entry", 5, END_OF_CHAIN, 0));
        b.extend(entry(name, 2, 2, data.len() as u32));
        b.resize(512 * 3, 0);
        b.extend_from_slice(data);
        b.resize(512 * (3 + sectors as usize), 0);
        b
    }

    #[test]
    fn ordinary_files_pass() {
        for name in [
            "sample.doc",
            "sample.xls",
            "sample.docx",
            "broken.docx",
            "sample.png",
        ] {
            assert_eq!(legacy_encryption(&fixture(name)), None, "{name}");
        }
        assert_eq!(legacy_encryption_of(&OLE_MAGIC), None, "truncated");
        let mut junk = OLE_MAGIC.to_vec();
        junk.resize(4096, 0xFF);
        assert_eq!(legacy_encryption_of(&junk), None, "garbage header");
    }
}
