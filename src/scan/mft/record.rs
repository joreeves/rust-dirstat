//! FILE record header and attribute parsing (zero-copy views into a fixed-up record).

use super::error::{OrCorrupt, ParseError};
use super::le::{slice_at, u8_at, u16_at, u32_at, u64_at};

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
pub const ATTR_END: u32 = 0xFFFF_FFFF;

pub const RECORD_IN_USE: u16 = 0x0001;
pub const RECORD_IS_DIRECTORY: u16 = 0x0002;

/// `$FILE_NAME` namespace for DOS 8.3 short names (skipped: they duplicate the long name).
pub const NAMESPACE_DOS: u8 = 2;
/// File attribute bit that says the reparse tag field of `$FILE_NAME` is valid.
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// Reparse tags with this bit are name surrogates (symlinks, junctions, mount points).
pub const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// The root directory is always MFT record 5.
pub const ROOT_RECORD: u64 = 5;

/// A file reference: 48-bit record number plus 16-bit sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FileRef(pub u64);

impl FileRef {
    #[inline]
    pub fn record(self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_FFFF
    }
    #[inline]
    pub fn seq(self) -> u16 {
        (self.0 >> 48) as u16
    }
    pub fn new(record: u64, seq: u16) -> Self {
        FileRef((u64::from(seq) << 48) | (record & 0x0000_FFFF_FFFF_FFFF))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordHeader {
    pub seq: u16,
    pub flags: u16,
    pub first_attr: usize,
    pub bytes_in_use: usize,
    /// Zero for base records; otherwise the base record this extension record belongs to.
    pub base: FileRef,
    /// The record's own number (NTFS 3.1+ only).
    pub record_number: Option<u32>,
}

impl RecordHeader {
    pub fn in_use(&self) -> bool {
        self.flags & RECORD_IN_USE != 0
    }
    pub fn is_dir(&self) -> bool {
        self.flags & RECORD_IS_DIRECTORY != 0
    }
    pub fn is_base(&self) -> bool {
        self.base.0 == 0
    }
}

/// Parses the header of a fixed-up FILE record.
pub fn parse_header(rec: &[u8]) -> Result<RecordHeader, ParseError> {
    if slice_at(rec, 0, 4) != Some(b"FILE") {
        return Err(ParseError("missing FILE signature"));
    }
    let usa_ofs = usize::from(u16_at(rec, 4).or_corrupt("record header")?);
    let usa_cnt = usize::from(u16_at(rec, 6).or_corrupt("record header")?);
    let seq = u16_at(rec, 0x10).or_corrupt("record header")?;
    let first_attr = usize::from(u16_at(rec, 0x14).or_corrupt("record header")?);
    let flags = u16_at(rec, 0x16).or_corrupt("record header")?;
    let bytes_in_use = u32_at(rec, 0x18).or_corrupt("record header")? as usize;
    let bytes_alloc = u32_at(rec, 0x1C).or_corrupt("record header")? as usize;
    let base = FileRef(u64_at(rec, 0x20).or_corrupt("record header")?);
    if bytes_alloc != rec.len() || bytes_in_use > rec.len() {
        return Err(ParseError("record size mismatch"));
    }
    if first_attr % 8 != 0 || first_attr < usa_ofs + 2 * usa_cnt || first_attr >= bytes_in_use {
        return Err(ParseError("invalid first attribute offset"));
    }
    let record_number = if usa_ofs >= 0x30 { u32_at(rec, 0x2C) } else { None };
    Ok(RecordHeader { seq, flags, first_attr, bytes_in_use, base, record_number })
}

/// Fields of a non-resident attribute header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonResident<'a> {
    pub lowest_vcn: u64,
    pub highest_vcn: u64,
    pub allocated: u64,
    pub size: u64,
    pub initialized: u64,
    /// Actual allocated clusters in bytes, present for compressed/sparse attributes.
    pub total_allocated: Option<u64>,
    pub runs: &'a [u8],
}

impl NonResident<'_> {
    /// Bytes actually occupied on disk (only meaningful on the piece with lowest VCN 0).
    pub fn on_disk(&self) -> u64 {
        self.total_allocated.unwrap_or(self.allocated)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrBody<'a> {
    Resident { value: &'a [u8] },
    NonResident(NonResident<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr<'a> {
    pub type_code: u32,
    /// Attribute name as raw UTF-16LE bytes (empty for unnamed attributes).
    pub name: &'a [u8],
    pub flags: u16,
    pub body: AttrBody<'a>,
}

/// Iterates over the attributes of a fixed-up record, stopping at the end marker.
/// Yields at most one error, after which iteration stops.
pub struct AttrIter<'a> {
    rec: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> AttrIter<'a> {
    pub fn new(rec: &'a [u8], header: &RecordHeader) -> Self {
        let rec = rec.get(..header.bytes_in_use).unwrap_or(rec);
        Self { rec, pos: header.first_attr, done: false }
    }

    fn parse_next(&mut self) -> Result<Option<Attr<'a>>, ParseError> {
        let type_code = u32_at(self.rec, self.pos).or_corrupt("attribute past end of record")?;
        if type_code == ATTR_END {
            return Ok(None);
        }
        let len = u32_at(self.rec, self.pos + 4).or_corrupt("attribute header")? as usize;
        if len < 0x18 || !len.is_multiple_of(8) {
            return Err(ParseError("invalid attribute length"));
        }
        let a = slice_at(self.rec, self.pos, len).or_corrupt("attribute past end of record")?;
        self.pos += len;

        let non_resident = u8_at(a, 8).or_corrupt("attribute header")? != 0;
        let name_len = usize::from(u8_at(a, 9).or_corrupt("attribute header")?);
        let name_off = usize::from(u16_at(a, 0x0A).or_corrupt("attribute header")?);
        let flags = u16_at(a, 0x0C).or_corrupt("attribute header")?;
        let name = if name_len == 0 { &[][..] } else { slice_at(a, name_off, name_len * 2).or_corrupt("attribute name")? };

        let body = if !non_resident {
            let value_len = u32_at(a, 0x10).or_corrupt("attribute header")? as usize;
            let value_off = usize::from(u16_at(a, 0x14).or_corrupt("attribute header")?);
            AttrBody::Resident { value: slice_at(a, value_off, value_len).or_corrupt("resident value")? }
        } else {
            if len < 0x40 {
                return Err(ParseError("non-resident attribute too short"));
            }
            let runs_off = usize::from(u16_at(a, 0x20).or_corrupt("attribute header")?);
            if runs_off < 0x40 || runs_off >= len {
                return Err(ParseError("invalid run list offset"));
            }
            let total_allocated = if flags & 0x80FF != 0 && runs_off >= 0x48 { u64_at(a, 0x40) } else { None };
            AttrBody::NonResident(NonResident {
                lowest_vcn: u64_at(a, 0x10).or_corrupt("attribute header")?,
                highest_vcn: u64_at(a, 0x18).or_corrupt("attribute header")?,
                allocated: u64_at(a, 0x28).or_corrupt("attribute header")?,
                size: u64_at(a, 0x30).or_corrupt("attribute header")?,
                initialized: u64_at(a, 0x38).or_corrupt("attribute header")?,
                total_allocated,
                runs: a.get(runs_off..).or_corrupt("run list")?,
            })
        };
        Ok(Some(Attr { type_code, name, flags, body }))
    }
}

impl<'a> Iterator for AttrIter<'a> {
    type Item = Result<Attr<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.parse_next() {
            Ok(Some(attr)) => Some(Ok(attr)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// A parsed `$FILE_NAME` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileName<'a> {
    pub parent: FileRef,
    pub file_attributes: u32,
    pub reparse_tag: u32,
    pub namespace: u8,
    /// UTF-16LE bytes.
    pub name: &'a [u8],
}

impl FileName<'_> {
    /// True for symlinks, junctions and mount points (never traversed).
    pub fn is_name_surrogate(&self) -> bool {
        self.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            && self.reparse_tag & REPARSE_TAG_NAME_SURROGATE != 0
    }
}

pub fn parse_file_name(v: &[u8]) -> Result<FileName<'_>, ParseError> {
    let name_len = usize::from(u8_at(v, 0x40).or_corrupt("$FILE_NAME")?);
    Ok(FileName {
        parent: FileRef(u64_at(v, 0).or_corrupt("$FILE_NAME")?),
        file_attributes: u32_at(v, 0x38).or_corrupt("$FILE_NAME")?,
        reparse_tag: u32_at(v, 0x3C).or_corrupt("$FILE_NAME")?,
        namespace: u8_at(v, 0x41).or_corrupt("$FILE_NAME")?,
        name: slice_at(v, 0x42, name_len * 2).or_corrupt("$FILE_NAME name")?,
    })
}

/// One entry of an `$ATTRIBUTE_LIST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttrListEntry<'a> {
    pub type_code: u32,
    pub lowest_vcn: u64,
    /// The record holding this attribute piece.
    pub frn: FileRef,
    pub name: &'a [u8],
}

pub fn parse_attribute_list(v: &[u8]) -> Result<Vec<AttrListEntry<'_>>, ParseError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 0x1A <= v.len() {
        let type_code = u32_at(v, pos).or_corrupt("attribute list")?;
        if type_code == 0 || type_code == ATTR_END {
            break;
        }
        let len = usize::from(u16_at(v, pos + 4).or_corrupt("attribute list")?);
        if len < 0x1A {
            return Err(ParseError("invalid attribute list entry length"));
        }
        let e = slice_at(v, pos, len).or_corrupt("attribute list entry")?;
        let name_len = usize::from(u8_at(e, 6).or_corrupt("attribute list")?);
        let name_off = usize::from(u8_at(e, 7).or_corrupt("attribute list")?);
        out.push(AttrListEntry {
            type_code,
            lowest_vcn: u64_at(e, 8).or_corrupt("attribute list")?,
            frn: FileRef(u64_at(e, 0x10).or_corrupt("attribute list")?),
            name: if name_len == 0 { &[][..] } else { slice_at(e, name_off, name_len * 2).or_corrupt("attribute list name")? },
        });
        pos += len;
    }
    Ok(out)
}

/// Decodes a UTF-16LE byte slice (lossy for unpaired surrogates).
pub fn utf16_bytes_to_string(bytes: &[u8]) -> String {
    let units = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
    char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

#[cfg(test)]
mod tests {
    use super::super::testutil::RecordBuilder;
    use super::*;

    #[test]
    fn header_and_attributes() {
        let mut rb = RecordBuilder::new(1024, 42, 3);
        rb.flags(RECORD_IN_USE).file_name(5, 5, 1, "hello.txt", 0, 0).resident_data("", b"abc");
        let mut rec = rb.build();
        super::super::fixup::apply_fixups(&mut rec).unwrap();
        let h = parse_header(&rec).unwrap();
        assert_eq!((h.seq, h.record_number, h.is_base(), h.in_use(), h.is_dir()), (3, Some(42), true, true, false));
        let attrs: Vec<_> = AttrIter::new(&rec, &h).collect::<Result<_, _>>().unwrap();
        assert_eq!(attrs.len(), 2);
        let AttrBody::Resident { value } = attrs[0].body else { panic!() };
        let fname = parse_file_name(value).unwrap();
        assert_eq!(fname.parent, FileRef::new(5, 5));
        assert_eq!(utf16_bytes_to_string(fname.name), "hello.txt");
        assert_eq!(attrs[1].body, AttrBody::Resident { value: b"abc" });
    }

    #[test]
    fn non_resident_sizes() {
        let mut rb = RecordBuilder::new(1024, 50, 1);
        rb.flags(RECORD_IN_USE).non_resident_data("", 0, 9, 40960, 40000, 40000, Some(8192), &[0x11, 10, 0x20, 0]);
        let mut rec = rb.build();
        super::super::fixup::apply_fixups(&mut rec).unwrap();
        let h = parse_header(&rec).unwrap();
        let a = AttrIter::new(&rec, &h).next().unwrap().unwrap();
        let AttrBody::NonResident(nr) = a.body else { panic!() };
        assert_eq!((nr.lowest_vcn, nr.highest_vcn, nr.size, nr.on_disk()), (0, 9, 40000, 8192));
        assert_eq!(&nr.runs[..4], &[0x11, 10, 0x20, 0]);
    }

    #[test]
    fn attribute_iterator_errors_do_not_panic() {
        let mut rb = RecordBuilder::new(1024, 7, 1);
        rb.flags(RECORD_IN_USE).resident_data("", b"x");
        let mut rec = rb.build();
        super::super::fixup::apply_fixups(&mut rec).unwrap();
        let h = parse_header(&rec).unwrap();
        // Zero attribute length.
        let mut bad = rec.clone();
        bad[h.first_attr + 4..h.first_attr + 8].copy_from_slice(&0u32.to_le_bytes());
        let items: Vec<_> = AttrIter::new(&bad, &h).collect();
        assert_eq!(items.len(), 1);
        assert!(items[0].is_err());
        // Length running past bytes-in-use.
        let mut bad = rec.clone();
        bad[h.first_attr + 4..h.first_attr + 8].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(AttrIter::new(&bad, &h).next().unwrap().is_err());
        // Missing end marker: bytes-in-use cut right after the first attribute.
        let mut h2 = h;
        h2.bytes_in_use = h.first_attr + 0x20;
        assert!(AttrIter::new(&rec, &h2).any(|r| r.is_err()));
    }

    #[test]
    fn attribute_list_entries() {
        let mut v = Vec::new();
        for (t, vcn, rec) in [(0x10u32, 0u64, 9u64), (0x80, 100, 77)] {
            let mut e = vec![0u8; 0x20];
            e[0..4].copy_from_slice(&t.to_le_bytes());
            e[4..6].copy_from_slice(&0x20u16.to_le_bytes());
            e[7] = 0x1A;
            e[8..16].copy_from_slice(&vcn.to_le_bytes());
            e[16..24].copy_from_slice(&FileRef::new(rec, 2).0.to_le_bytes());
            v.extend(e);
        }
        let entries = parse_attribute_list(&v).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[1].type_code, entries[1].lowest_vcn, entries[1].frn.record()), (0x80, 100, 77));
        v[0x24..0x26].copy_from_slice(&4u16.to_le_bytes());
        assert!(parse_attribute_list(&v).is_err());
    }
}
