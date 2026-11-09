//! Test helpers: a FILE record builder and an in-memory NTFS-like volume image.

use std::io;
use std::sync::Arc;

use super::fixup::protect;
use super::io::ReadAt;
use super::record::{ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_FILE_NAME, FileRef, RECORD_IN_USE, RECORD_IS_DIRECTORY};
use super::runs::{Run, encode_runs};

fn align8(n: usize) -> usize {
    n.div_ceil(8) * 8
}

/// Builds a FILE record with real update-sequence protection, so tests exercise the fixup code.
#[derive(Clone)]
pub struct RecordBuilder {
    size: usize,
    number: u32,
    seq: u16,
    flags: u16,
    base: FileRef,
    attrs: Vec<Vec<u8>>,
}

impl RecordBuilder {
    pub fn new(size: usize, number: u32, seq: u16) -> Self {
        Self { size, number, seq, flags: RECORD_IN_USE, base: FileRef(0), attrs: Vec::new() }
    }

    pub fn record_number(&self) -> u32 {
        self.number
    }

    pub fn flags(&mut self, flags: u16) -> &mut Self {
        self.flags = flags;
        self
    }

    pub fn base(&mut self, base: FileRef) -> &mut Self {
        self.base = base;
        self
    }

    pub fn file_name(
        &mut self,
        parent: u64,
        parent_seq: u16,
        namespace: u8,
        name: &str,
        file_attributes: u32,
        reparse_tag: u32,
    ) -> &mut Self {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut v = vec![0u8; 0x42 + 2 * units.len()];
        v[0..8].copy_from_slice(&FileRef::new(parent, parent_seq).0.to_le_bytes());
        v[0x38..0x3C].copy_from_slice(&file_attributes.to_le_bytes());
        v[0x3C..0x40].copy_from_slice(&reparse_tag.to_le_bytes());
        v[0x40] = units.len() as u8;
        v[0x41] = namespace;
        for (i, u) in units.iter().enumerate() {
            v[0x42 + 2 * i..0x44 + 2 * i].copy_from_slice(&u.to_le_bytes());
        }
        self.resident(ATTR_FILE_NAME, "", &v)
    }

    pub fn resident_data(&mut self, name: &str, value: &[u8]) -> &mut Self {
        self.resident(ATTR_DATA, name, value)
    }

    pub fn resident(&mut self, type_code: u32, name: &str, value: &[u8]) -> &mut Self {
        let name_units: Vec<u16> = name.encode_utf16().collect();
        let name_off = 0x18;
        let value_off = align8(name_off + 2 * name_units.len());
        let len = align8(value_off + value.len());
        let mut a = vec![0u8; len];
        a[0..4].copy_from_slice(&type_code.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[9] = name_units.len() as u8;
        a[0x0A..0x0C].copy_from_slice(&(name_off as u16).to_le_bytes());
        a[0x0E..0x10].copy_from_slice(&(self.attrs.len() as u16).to_le_bytes());
        a[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[0x14..0x16].copy_from_slice(&(value_off as u16).to_le_bytes());
        for (i, u) in name_units.iter().enumerate() {
            a[name_off + 2 * i..name_off + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        a[value_off..value_off + value.len()].copy_from_slice(value);
        self.attrs.push(a);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn non_resident_data(
        &mut self,
        name: &str,
        lowest_vcn: u64,
        highest_vcn: u64,
        allocated: u64,
        size: u64,
        initialized: u64,
        total_allocated: Option<u64>,
        runs: &[u8],
    ) -> &mut Self {
        let name_units: Vec<u16> = name.encode_utf16().collect();
        let header = if total_allocated.is_some() { 0x48 } else { 0x40 };
        let runs_off = align8(header + 2 * name_units.len());
        let len = align8(runs_off + runs.len());
        let mut a = vec![0u8; len];
        a[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[8] = 1;
        a[9] = name_units.len() as u8;
        a[0x0A..0x0C].copy_from_slice(&(header as u16).to_le_bytes());
        let flags: u16 = if total_allocated.is_some() { 0x8000 } else { 0 };
        a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
        a[0x0E..0x10].copy_from_slice(&(self.attrs.len() as u16).to_le_bytes());
        a[0x10..0x18].copy_from_slice(&lowest_vcn.to_le_bytes());
        a[0x18..0x20].copy_from_slice(&highest_vcn.to_le_bytes());
        a[0x20..0x22].copy_from_slice(&(runs_off as u16).to_le_bytes());
        a[0x28..0x30].copy_from_slice(&allocated.to_le_bytes());
        a[0x30..0x38].copy_from_slice(&size.to_le_bytes());
        a[0x38..0x40].copy_from_slice(&initialized.to_le_bytes());
        if let Some(t) = total_allocated {
            a[0x40..0x48].copy_from_slice(&t.to_le_bytes());
        }
        for (i, u) in name_units.iter().enumerate() {
            a[header + 2 * i..header + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        a[runs_off..runs_off + runs.len()].copy_from_slice(runs);
        self.attrs.push(a);
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut rec = vec![0u8; self.size];
        let usa_ofs = 0x30;
        let usa_cnt = self.size / 512 + 1;
        let first_attr = align8(usa_ofs + 2 * usa_cnt);
        rec[0..4].copy_from_slice(b"FILE");
        rec[0x10..0x12].copy_from_slice(&self.seq.to_le_bytes());
        rec[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
        rec[0x14..0x16].copy_from_slice(&(first_attr as u16).to_le_bytes());
        rec[0x16..0x18].copy_from_slice(&self.flags.to_le_bytes());
        rec[0x1C..0x20].copy_from_slice(&(self.size as u32).to_le_bytes());
        rec[0x20..0x28].copy_from_slice(&self.base.0.to_le_bytes());
        rec[0x28..0x2A].copy_from_slice(&(self.attrs.len() as u16).to_le_bytes());
        rec[0x2C..0x30].copy_from_slice(&self.number.to_le_bytes());
        let mut pos = first_attr;
        for a in &self.attrs {
            rec[pos..pos + a.len()].copy_from_slice(a);
            pos += a.len();
        }
        rec[pos..pos + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        pos += 8;
        rec[0x18..0x1C].copy_from_slice(&(pos as u32).to_le_bytes());
        protect(&mut rec, usa_ofs, self.seq.wrapping_mul(31).wrapping_add(7));
        rec
    }
}

/// An in-memory volume that enforces the alignment rules of real raw-volume reads.
#[derive(Clone)]
pub struct AlignedMemVolume {
    data: Arc<Vec<u8>>,
    sector: usize,
}

impl AlignedMemVolume {
    pub fn new(data: Vec<u8>, sector: usize) -> Self {
        Self { data: Arc::new(data), sector }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

impl ReadAt for AlignedMemVolume {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        assert_eq!(offset as usize % self.sector, 0, "unaligned read offset {offset}");
        assert_eq!(buf.len() % self.sector, 0, "unaligned read length {}", buf.len());
        assert_eq!(buf.as_ptr() as usize % 512, 0, "unaligned read buffer");
        let start = offset as usize;
        let src = self.data.get(start..start + buf.len()).ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

const SECTOR: usize = 512;
const CLUSTER: usize = 4096;
const RS: usize = 1024;
const TOTAL_CLUSTERS: usize = 256;

/// A 1 MiB volume image with a fragmented MFT (three extents, one of them listed only in an
/// extension record through `$ATTRIBUTE_LIST`) and a small directory tree:
///
/// ```text
/// $MFT (32 KiB; initialized 28 records)   Docs/ (20)  a.txt 100 B (21)
/// big.iso 1,000,000 B (22)                Docs/Sub/ (23)  deep.dat 5000 B (24)
/// Junction (25, name surrogate)           deleted.txt (26, not in use)
/// ghost.txt (30, past the initialized size: must be ignored)
/// ```
pub fn sample_volume() -> AlignedMemVolume {
    let mut img = vec![0u8; TOTAL_CLUSTERS * CLUSTER];

    // Boot sector.
    img[3..11].copy_from_slice(b"NTFS    ");
    img[0x0B..0x0D].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    img[0x0D] = (CLUSTER / SECTOR) as u8;
    img[0x28..0x30].copy_from_slice(&((TOTAL_CLUSTERS * CLUSTER / SECTOR) as u64).to_le_bytes());
    img[0x30..0x38].copy_from_slice(&100u64.to_le_bytes());
    img[0x40] = 0xF6;
    img[0x48..0x50].copy_from_slice(&0xABCD_1234u64.to_le_bytes());
    img[0x1FE] = 0x55;
    img[0x1FF] = 0xAA;

    // MFT extents: VCN 0-3 -> LCN 100, VCN 4-5 -> LCN 40 (negative delta), VCN 6-7 -> LCN 60.
    let map = |vcn: usize| -> usize {
        match vcn {
            0..=3 => 100 + vcn,
            4..=5 => 40 + vcn - 4,
            6..=7 => 60 + vcn - 6,
            _ => panic!("vcn {vcn} outside the MFT"),
        }
    };
    let mut put = |rb: &RecordBuilder| {
        let off = rb.record_number() as usize * RS;
        let disk = map(off / CLUSTER) * CLUSTER + off % CLUSTER;
        img[disk..disk + RS].copy_from_slice(&rb.build());
    };

    let first_runs = encode_runs(&[Run { len: 4, lcn: Some(100) }, Run { len: 2, lcn: Some(40) }]);
    let mut attr_list = Vec::new();
    for (type_code, vcn, rec) in [(0x30u32, 0u64, 0u64), (0x80, 0, 0), (0x80, 6, 16)] {
        let mut e = vec![0u8; 0x20];
        e[0..4].copy_from_slice(&type_code.to_le_bytes());
        e[4..6].copy_from_slice(&0x20u16.to_le_bytes());
        e[7] = 0x1A;
        e[8..16].copy_from_slice(&vcn.to_le_bytes());
        e[16..24].copy_from_slice(&FileRef::new(rec, 1).0.to_le_bytes());
        attr_list.extend(e);
    }
    let mut mft = RecordBuilder::new(RS, 0, 1);
    mft.flags(RECORD_IN_USE)
        .resident(ATTR_ATTRIBUTE_LIST, "", &attr_list)
        .file_name(5, 5, 3, "$MFT", 0, 0)
        .non_resident_data("", 0, 5, 32768, 32768, 28 * 1024, None, &first_runs);
    put(&mft);

    let mut mft_ext = RecordBuilder::new(RS, 16, 1);
    mft_ext.flags(RECORD_IN_USE).base(FileRef::new(0, 1)).non_resident_data(
        "",
        6,
        7,
        0,
        0,
        0,
        None,
        &encode_runs(&[Run { len: 2, lcn: Some(60) }]),
    );
    put(&mft_ext);

    let dir_flags = RECORD_IN_USE | RECORD_IS_DIRECTORY;
    let mut root = RecordBuilder::new(RS, 5, 5);
    root.flags(dir_flags).file_name(5, 5, 3, ".", 0x16, 0);
    put(&root);

    let mut docs = RecordBuilder::new(RS, 20, 1);
    docs.flags(dir_flags).file_name(5, 5, 1, "Docs", 0x10, 0).file_name(5, 5, 2, "DOCS~1", 0x10, 0);
    put(&docs);

    let mut a = RecordBuilder::new(RS, 21, 1);
    a.flags(RECORD_IN_USE).file_name(20, 1, 3, "a.txt", 0x20, 0).resident_data("", &[b'a'; 100]);
    put(&a);

    let small_run = encode_runs(&[Run { len: 1, lcn: Some(200) }]);
    let mut big = RecordBuilder::new(RS, 22, 1);
    big.flags(RECORD_IN_USE).file_name(5, 5, 3, "big.iso", 0x20, 0).non_resident_data(
        "",
        0,
        244,
        1_003_520,
        1_000_000,
        1_000_000,
        None,
        &small_run,
    );
    put(&big);

    let mut sub = RecordBuilder::new(RS, 23, 1);
    sub.flags(dir_flags).file_name(20, 1, 3, "Sub", 0x10, 0);
    put(&sub);

    let mut deep = RecordBuilder::new(RS, 24, 1);
    deep.flags(RECORD_IN_USE).file_name(23, 1, 3, "deep.dat", 0x20, 0).non_resident_data(
        "",
        0,
        1,
        8192,
        5000,
        5000,
        None,
        &small_run,
    );
    put(&deep);

    let mut junction = RecordBuilder::new(RS, 25, 1);
    junction.flags(dir_flags).file_name(5, 5, 3, "Junction", 0x410, 0xA000_0003);
    put(&junction);

    let mut deleted = RecordBuilder::new(RS, 26, 3);
    deleted.flags(0).file_name(5, 5, 3, "deleted.txt", 0x20, 0).resident_data("", &[0; 50]);
    put(&deleted);

    let mut ghost = RecordBuilder::new(RS, 30, 1);
    ghost.flags(RECORD_IN_USE).file_name(5, 5, 3, "ghost.txt", 0x20, 0).resident_data("", &[0; 77]);
    put(&ghost);

    AlignedMemVolume::new(img, SECTOR)
}
