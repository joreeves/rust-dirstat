//! Parses a contiguous run of MFT records (one read chunk) into compact per-record facts.
//! Pure and allocation-light: no per-record heap allocations, names go into one buffer.

use super::error::ParseError;
use super::fixup::apply_fixups;
use super::le::u32_at;
use super::record::{
    ATTR_DATA, ATTR_FILE_NAME, ATTR_REPARSE_POINT, AttrBody, AttrIter, FileRef, NAMESPACE_DOS,
    REPARSE_TAG_NAME_SURROGATE, RecordHeader, parse_file_name, parse_header,
};

/// Facts about an in-use base record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseRec {
    pub record: u32,
    pub seq: u16,
    pub is_dir: bool,
    /// Symlink / junction / mount point.
    pub surrogate: bool,
    /// Logical size of the unnamed `$DATA` stream, if its first piece lives in this record.
    pub data_size: Option<u64>,
    /// On-disk bytes of named `$DATA` streams found in this record (used for NTFS metafiles).
    pub stream_alloc: u64,
}

/// Facts from an extension record, to be merged into its base record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtPatch {
    pub base: FileRef,
    pub data_size: Option<u64>,
    pub stream_alloc: u64,
    pub surrogate: bool,
}

/// One hard link (`$FILE_NAME` that is not a DOS 8.3 alias): `child` is named `name` in `parent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawLink {
    pub child: u32,
    /// For links found in extension records: the base record's expected sequence number.
    pub child_seq: Option<u16>,
    pub parent: FileRef,
    pub name_start: u32,
    pub name_len: u32,
}

#[derive(Debug, Default)]
pub struct ChunkOut {
    pub bases: Vec<BaseRec>,
    pub patches: Vec<ExtPatch>,
    pub links: Vec<RawLink>,
    /// UTF-16 code units of all link names in this chunk.
    pub names: Vec<u16>,
    /// Records that were skipped as malformed or torn.
    pub bad: u64,
}

impl ChunkOut {
    pub fn name(&self, link: &RawLink) -> &[u16] {
        let start = link.name_start as usize;
        self.names.get(start..start + link.name_len as usize).unwrap_or(&[])
    }
}

/// Parses `buf`, which holds consecutive records starting at `first_record`. Fixups are applied
/// in place. A trailing partial record is ignored.
pub fn parse_chunk(buf: &mut [u8], first_record: u64, record_size: usize) -> ChunkOut {
    let mut out = ChunkOut::default();
    for (i, rec) in buf.chunks_exact_mut(record_size).enumerate() {
        let recno = first_record + i as u64;
        let (links_mark, names_mark) = (out.links.len(), out.names.len());
        if parse_record(rec, recno, &mut out).is_err() {
            out.links.truncate(links_mark);
            out.names.truncate(names_mark);
            out.bad += 1;
        }
    }
    out
}

#[derive(Default)]
struct Summary {
    data_size: Option<u64>,
    stream_alloc: u64,
    surrogate: bool,
}

fn parse_record(rec: &mut [u8], recno: u64, out: &mut ChunkOut) -> Result<(), ParseError> {
    match rec.get(0..4) {
        Some(b"FILE") => {}
        Some(b"BAAD") => return Err(ParseError("record marked BAAD")),
        // Never-used slots are zero-filled or hold stale garbage; they are not errors.
        _ => return Ok(()),
    }
    apply_fixups(rec)?;
    let rec: &[u8] = rec;
    let header = parse_header(rec)?;
    if !header.in_use() {
        return Ok(());
    }
    if header.record_number.is_some_and(|n| u64::from(n) != recno & 0xFFFF_FFFF) {
        return Err(ParseError("record number mismatch"));
    }
    let record = u32::try_from(recno).map_err(|_| ParseError("record number too large"))?;
    let (child, child_seq) = if header.is_base() {
        (record, None)
    } else {
        let base = u32::try_from(header.base.record()).map_err(|_| ParseError("base record too large"))?;
        (base, Some(header.base.seq()))
    };
    let s = scan_attributes(rec, &header, child, child_seq, out)?;
    if header.is_base() {
        out.bases.push(BaseRec {
            record,
            seq: header.seq,
            is_dir: header.is_dir(),
            surrogate: s.surrogate,
            data_size: s.data_size,
            stream_alloc: s.stream_alloc,
        });
    } else {
        out.patches.push(ExtPatch {
            base: header.base,
            data_size: s.data_size,
            stream_alloc: s.stream_alloc,
            surrogate: s.surrogate,
        });
    }
    Ok(())
}

fn scan_attributes(
    rec: &[u8],
    header: &RecordHeader,
    child: u32,
    child_seq: Option<u16>,
    out: &mut ChunkOut,
) -> Result<Summary, ParseError> {
    let mut s = Summary::default();
    let links_mark = out.links.len();
    for attr in AttrIter::new(rec, header) {
        let attr = attr?;
        match (attr.type_code, attr.body) {
            (ATTR_FILE_NAME, AttrBody::Resident { value }) => {
                let fname = parse_file_name(value)?;
                if fname.is_name_surrogate() {
                    s.surrogate = true;
                }
                if fname.namespace == NAMESPACE_DOS {
                    continue;
                }
                let units = fname.name.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c));
                let duplicate = out.links[links_mark..].iter().any(|l| {
                    l.parent == fname.parent
                        && l.name_len as usize == fname.name.len() / 2
                        && out.name(l).iter().copied().eq(units.clone())
                });
                if duplicate {
                    continue;
                }
                let start = out.names.len();
                out.names.extend(units);
                out.links.push(RawLink {
                    child,
                    child_seq,
                    parent: fname.parent,
                    name_start: u32::try_from(start).map_err(|_| ParseError("name buffer overflow"))?,
                    name_len: (out.names.len() - start) as u32,
                });
            }
            (ATTR_DATA, AttrBody::Resident { value }) if attr.name.is_empty() => {
                s.data_size = Some(value.len() as u64);
            }
            (ATTR_DATA, AttrBody::NonResident(nr)) if nr.lowest_vcn == 0 => {
                if attr.name.is_empty() {
                    s.data_size = Some(nr.size);
                } else {
                    s.stream_alloc += nr.on_disk();
                }
            }
            (ATTR_REPARSE_POINT, AttrBody::Resident { value })
                if u32_at(value, 0).is_some_and(|tag| tag & REPARSE_TAG_NAME_SURROGATE != 0) => {
                    s.surrogate = true;
                }
            _ => {}
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::super::record::{RECORD_IN_USE, RECORD_IS_DIRECTORY};
    use super::super::testutil::RecordBuilder;
    use super::*;

    fn names(out: &ChunkOut) -> Vec<String> {
        out.links.iter().map(|l| String::from_utf16_lossy(out.name(l))).collect()
    }

    #[test]
    fn base_record_with_win32_and_dos_names() {
        let mut rb = RecordBuilder::new(1024, 30, 4);
        rb.flags(RECORD_IN_USE)
            .file_name(5, 5, 1, "Long File Name.txt", 0, 0)
            .file_name(5, 5, 2, "LONGFI~1.TXT", 0, 0)
            .resident_data("", &[1; 300]);
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 30, 1024);
        assert_eq!(out.bad, 0);
        assert_eq!(out.bases, vec![BaseRec { record: 30, seq: 4, is_dir: false, surrogate: false, data_size: Some(300), stream_alloc: 0 }]);
        assert_eq!(names(&out), ["Long File Name.txt"]);
        assert_eq!(out.links[0].parent, FileRef::new(5, 5));
    }

    #[test]
    fn posix_only_and_win32_dos_combined() {
        let mut rb = RecordBuilder::new(1024, 31, 1);
        rb.flags(RECORD_IN_USE).file_name(5, 5, 0, "posix", 0, 0).file_name(40, 2, 3, "SHORT.TXT", 0, 0);
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 31, 1024);
        assert_eq!(names(&out), ["posix", "SHORT.TXT"]);
    }

    #[test]
    fn dos_only_name_is_skipped() {
        let mut rb = RecordBuilder::new(1024, 32, 1);
        rb.flags(RECORD_IN_USE).file_name(5, 5, 2, "A~1", 0, 0);
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 32, 1024);
        assert!(out.links.is_empty());
        assert_eq!(out.bases.len(), 1);
    }

    #[test]
    fn data_variants() {
        let runs = [0x11, 4, 0x10, 0];
        let mut rb = RecordBuilder::new(1024, 33, 1);
        rb.flags(RECORD_IN_USE)
            .file_name(5, 5, 1, "big.bin", 0, 0)
            .non_resident_data("", 0, 3, 16384, 15000, 15000, None, &runs)
            .non_resident_data("ads", 0, 3, 16384, 16000, 16000, Some(4096), &runs)
            .non_resident_data("", 4, 7, 0, 0, 0, None, &runs);
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 33, 1024);
        assert_eq!(out.bases[0].data_size, Some(15000));
        assert_eq!(out.bases[0].stream_alloc, 4096);
    }

    #[test]
    fn extension_record_yields_patch_and_links() {
        let mut rb = RecordBuilder::new(1024, 60, 9);
        rb.flags(RECORD_IN_USE).base(FileRef::new(33, 1)).file_name(70, 3, 1, "second link.bin", 0, 0).non_resident_data(
            "",
            0,
            1,
            8192,
            8000,
            8000,
            None,
            &[0x11, 2, 0x30, 0],
        );
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 60, 1024);
        assert!(out.bases.is_empty());
        assert_eq!(out.patches, vec![ExtPatch { base: FileRef::new(33, 1), data_size: Some(8000), stream_alloc: 0, surrogate: false }]);
        assert_eq!(out.links[0].child, 33);
        assert_eq!(out.links[0].child_seq, Some(1));
    }

    #[test]
    fn junction_is_flagged() {
        let mut rb = RecordBuilder::new(1024, 34, 1);
        rb.flags(RECORD_IN_USE | RECORD_IS_DIRECTORY).file_name(5, 5, 1, "junction", 0x400 | 0x10, 0xA000_0003);
        let mut buf = rb.build();
        let out = parse_chunk(&mut buf, 34, 1024);
        assert!(out.bases[0].surrogate && out.bases[0].is_dir);
    }

    #[test]
    fn unused_garbage_and_mismatched_records() {
        let mut buf = vec![0u8; 4096];
        // Record 0: never used (zeros). Record 1: not in use. Record 2: wrong number. Record 3: torn.
        let mut free = RecordBuilder::new(1024, 101, 1);
        free.flags(0).file_name(5, 5, 1, "deleted", 0, 0);
        buf[1024..2048].copy_from_slice(&free.build());
        let mut wrong = RecordBuilder::new(1024, 999, 1);
        wrong.flags(RECORD_IN_USE);
        buf[2048..3072].copy_from_slice(&wrong.build());
        let mut torn = RecordBuilder::new(1024, 103, 1);
        torn.flags(RECORD_IN_USE);
        let mut t = torn.build();
        t[1022] ^= 1;
        buf[3072..].copy_from_slice(&t);
        let out = parse_chunk(&mut buf, 100, 1024);
        assert!(out.bases.is_empty() && out.links.is_empty());
        assert_eq!(out.bad, 2);
    }

    #[test]
    fn fuzz_never_panics() {
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut rb = RecordBuilder::new(1024, 10, 1);
        rb.flags(RECORD_IN_USE)
            .file_name(5, 5, 1, "fuzz target.txt", 0, 0)
            .non_resident_data("", 0, 3, 16384, 15000, 15000, Some(8192), &[0x11, 4, 0x10, 0]);
        let valid = rb.build();
        for i in 0..100_000u32 {
            let mut rec = valid.clone();
            if i % 10 == 0 {
                for b in rec.iter_mut().skip(4) {
                    *b = rnd() as u8;
                }
            } else {
                for _ in 0..(rnd() % 4 + 1) {
                    let pos = (rnd() % 1024) as usize;
                    rec[pos] = rnd() as u8;
                }
            }
            let _ = parse_chunk(&mut rec, 10, 1024);
        }
    }
}
