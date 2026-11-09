//! Raw volume I/O: aligned positional reads, opening `\\.\X:`, and locating the `$MFT` data.

use std::io;

use super::boot::{BootSector, parse_boot_sector};
use super::error::MftError;
use super::fixup::apply_fixups;
use super::record::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, AttrBody, AttrIter, parse_attribute_list, parse_header,
};
use super::runs::{Extents, decode_runs};

/// Buffers handed to volume reads are aligned to this many bytes.
pub const IO_ALIGN: usize = 4096;

/// Positional reads from a volume. Offsets and lengths are always multiples of the sector size.
pub trait ReadAt: Send + Sync {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

#[cfg(windows)]
impl ReadAt for std::fs::File {
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// A heap buffer whose usable slice starts at an [`IO_ALIGN`]-aligned address.
pub struct AlignedBuf {
    raw: Vec<u8>,
    off: usize,
    len: usize,
}

impl AlignedBuf {
    pub fn new(len: usize) -> Self {
        let raw = vec![0u8; len + IO_ALIGN];
        let off = raw.as_ptr().align_offset(IO_ALIGN);
        assert!(off < IO_ALIGN, "cannot align buffer");
        Self { raw, off, len }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.raw[self.off..self.off + self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.raw[self.off..self.off + self.len]
    }
}

/// Opens the raw volume for drive `letter` read-only.
#[cfg(windows)]
pub fn open_volume(letter: char) -> Result<std::fs::File, MftError> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    let path = format!(r"\\.\{letter}:");
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .map_err(|e| match e.raw_os_error() {
            Some(5) => MftError::AccessDenied,
            Some(2 | 3) => MftError::NotLocalNtfs(format!("no volume {letter}:")),
            Some(21) => MftError::NotLocalNtfs(format!("{letter}: is not ready")),
            Some(1005) => MftError::NotLocalNtfs(format!("{letter}: has no recognized file system")),
            _ => MftError::Io(e),
        })
}

/// Where the MFT lives on disk and how many records to parse.
#[derive(Clone, Debug)]
pub struct MftLayout {
    pub boot: BootSector,
    pub extents: Extents,
    pub record_count: u64,
}

/// Reads the boot sector and `$MFT`'s own record to build the MFT extent map.
pub fn bootstrap<R: ReadAt + ?Sized>(r: &R) -> Result<MftLayout, MftError> {
    let mut sector = AlignedBuf::new(IO_ALIGN);
    r.read_exact_at(sector.as_mut_slice(), 0)?;
    let boot = parse_boot_sector(sector.as_slice())?;
    let cs = boot.cluster_size;
    let rs = u64::from(boot.record_size);

    let mut rec0 = read_bytes(r, boot.mft_lcn * cs, rs, cs)?;
    apply_fixups(&mut rec0)?;
    let header = parse_header(&rec0)?;
    if !header.in_use() || !header.is_base() {
        return Err(MftError::Corrupt("$MFT record is not an in-use base record"));
    }

    let mut extents = Extents::default();
    let mut sizes: Option<(u64, u64)> = None; // (data size, initialized size)
    let mut attr_list: Option<Vec<u8>> = None;
    for attr in AttrIter::new(&rec0, &header) {
        let attr = attr?;
        match (attr.type_code, attr.body) {
            (ATTR_DATA, AttrBody::NonResident(nr)) if attr.name.is_empty() => {
                let runs = decode_runs(nr.runs, boot.total_clusters)?;
                extents.insert_runs(nr.lowest_vcn, &runs);
                if nr.lowest_vcn == 0 {
                    sizes = Some((nr.size, nr.initialized));
                }
            }
            (ATTR_ATTRIBUTE_LIST, AttrBody::Resident { value }) => attr_list = Some(value.to_vec()),
            (ATTR_ATTRIBUTE_LIST, AttrBody::NonResident(nr)) => {
                let runs = decode_runs(nr.runs, boot.total_clusters)?;
                let mut list = Vec::new();
                for run in runs {
                    let lcn = run.lcn.ok_or(MftError::Corrupt("sparse attribute list"))?;
                    list.extend(read_bytes(r, lcn * cs, run.len * cs, cs)?);
                }
                list.truncate(nr.size as usize);
                attr_list = Some(list);
            }
            _ => {}
        }
    }
    let (data_size, initialized) = sizes.ok_or(MftError::Corrupt("$MFT has no $DATA attribute"))?;

    // A heavily fragmented $MFT keeps the rest of its run list in extension records, listed in
    // its $ATTRIBUTE_LIST. Those records live inside the MFT itself, so load them iteratively
    // as the known extents grow.
    if let Some(list) = attr_list {
        let mut pending: Vec<(u64, u64)> = parse_attribute_list(&list)?
            .into_iter()
            .filter(|e| e.type_code == ATTR_DATA && e.name.is_empty() && e.frn.record() != 0)
            .map(|e| (e.frn.record(), e.lowest_vcn))
            .collect();
        pending.sort_unstable();
        pending.dedup();
        while !pending.is_empty() {
            let before = pending.len();
            let mut still = Vec::new();
            for (recno, vcn) in pending {
                match read_record(r, &boot, &extents, recno)? {
                    Some(mut rec) => load_data_piece(&mut rec, &boot, vcn, &mut extents)?,
                    None => still.push((recno, vcn)),
                }
            }
            pending = still;
            if pending.len() == before {
                return Err(MftError::Unsupported("$MFT attribute list points outside the known MFT extents"));
            }
        }
    }

    // Never parse past the initialized size: the rest of the allocation holds stale data.
    let max_records = boot.volume_bytes() / rs;
    let record_count = (data_size.min(initialized) / rs).min(max_records).min(u64::from(u32::MAX));
    if extents.contiguous_clusters() < (record_count * rs).div_ceil(cs) {
        return Err(MftError::Unsupported("$MFT extent map has gaps"));
    }
    Ok(MftLayout { boot, extents, record_count })
}

fn load_data_piece(rec: &mut [u8], boot: &BootSector, vcn: u64, extents: &mut Extents) -> Result<(), MftError> {
    apply_fixups(rec)?;
    let header = parse_header(rec)?;
    if header.base.record() != 0 {
        return Err(MftError::Corrupt("$MFT extension record has the wrong base"));
    }
    for attr in AttrIter::new(rec, &header) {
        let attr = attr?;
        if let (ATTR_DATA, AttrBody::NonResident(nr)) = (attr.type_code, attr.body)
            && attr.name.is_empty()
            && nr.lowest_vcn == vcn
        {
            let runs = decode_runs(nr.runs, boot.total_clusters)?;
            extents.insert_runs(vcn, &runs);
            return Ok(());
        }
    }
    Err(MftError::Corrupt("$MFT extension record lacks the listed $DATA piece"))
}

/// Reads `len` bytes at `offset`, rounding the read up to whole clusters.
fn read_bytes<R: ReadAt + ?Sized>(r: &R, offset: u64, len: u64, cluster: u64) -> Result<Vec<u8>, MftError> {
    let padded = len.div_ceil(cluster) * cluster;
    let mut buf = AlignedBuf::new(padded as usize);
    r.read_exact_at(buf.as_mut_slice(), offset)?;
    Ok(buf.as_slice()[..len as usize].to_vec())
}

/// Reads MFT record `recno` through the extent map; `None` if it is not mapped yet.
fn read_record<R: ReadAt + ?Sized>(
    r: &R,
    boot: &BootSector,
    extents: &Extents,
    recno: u64,
) -> Result<Option<Vec<u8>>, MftError> {
    let (cs, rs) = (boot.cluster_size, u64::from(boot.record_size));
    let off = recno * rs;
    let (vcn, within) = (off / cs, off % cs);
    let clusters = (within + rs).div_ceil(cs);
    let Some((Some(lcn), left)) = extents.map(vcn) else { return Ok(None) };
    if left < clusters {
        return Err(MftError::Unsupported("MFT record spans two extents"));
    }
    let bytes = read_bytes(r, lcn * cs, clusters * cs, cs)?;
    Ok(Some(bytes[within as usize..(within + rs) as usize].to_vec()))
}

/// Fills `buf` with MFT data starting at record `first_record` (cluster-aligned), following
/// the extent map. `buf.len()` must be a multiple of the cluster size.
pub fn read_mft_range<R: ReadAt + ?Sized>(
    r: &R,
    layout: &MftLayout,
    first_record: u64,
    buf: &mut [u8],
) -> Result<(), MftError> {
    let cs = layout.boot.cluster_size;
    let mut off = first_record * u64::from(layout.boot.record_size);
    debug_assert_eq!(off % cs, 0);
    let mut pos = 0usize;
    while pos < buf.len() {
        let (lcn, left) = layout.extents.map(off / cs).ok_or(MftError::Corrupt("MFT read outside its extents"))?;
        let n = (left * cs).min((buf.len() - pos) as u64) as usize;
        let dst = &mut buf[pos..pos + n];
        match lcn {
            Some(l) => r.read_exact_at(dst, l * cs)?,
            None => dst.fill(0),
        }
        pos += n;
        off += n as u64;
    }
    Ok(())
}
