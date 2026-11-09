//! Update sequence ("fixup") handling. NTFS replaces the last two bytes of every 512-byte
//! stride of a multi-sector record with an update sequence number (USN) and keeps the real
//! bytes in the update sequence array (USA). A mismatch means the record was torn mid-write.

use super::error::{OrCorrupt, ParseError};
use super::le::u16_at;

/// Validates and undoes the fixups of `rec` in place. `rec.len()` must be the record size.
pub fn apply_fixups(rec: &mut [u8]) -> Result<(), ParseError> {
    let usa_ofs = usize::from(u16_at(rec, 4).or_corrupt("record header")?);
    let usa_cnt = usize::from(u16_at(rec, 6).or_corrupt("record header")?);
    if usa_cnt < 2 {
        return Err(ParseError("invalid update sequence count"));
    }
    let strides = usa_cnt - 1;
    let stride = rec.len() / strides;
    if stride * strides != rec.len() || stride < 512 || !stride.is_power_of_two() {
        return Err(ParseError("update sequence count does not match record size"));
    }
    if usa_ofs % 2 != 0 || usa_ofs + 2 * usa_cnt > stride - 2 {
        return Err(ParseError("invalid update sequence offset"));
    }
    let usn = u16_at(rec, usa_ofs).or_corrupt("update sequence")?;
    for i in 1..usa_cnt {
        let end = i * stride - 2;
        if u16_at(rec, end) != Some(usn) {
            return Err(ParseError("torn record (update sequence mismatch)"));
        }
        let saved = u16_at(rec, usa_ofs + 2 * i).or_corrupt("update sequence")?;
        rec.get_mut(end..end + 2).or_corrupt("update sequence")?.copy_from_slice(&saved.to_le_bytes());
    }
    Ok(())
}

/// Test helper: protects a record by writing a USA (the inverse of [`apply_fixups`]).
#[cfg(test)]
pub fn protect(rec: &mut [u8], usa_ofs: usize, usn: u16) {
    let strides = rec.len() / 512;
    let usa_cnt = strides + 1;
    rec[4..6].copy_from_slice(&(usa_ofs as u16).to_le_bytes());
    rec[6..8].copy_from_slice(&(usa_cnt as u16).to_le_bytes());
    rec[usa_ofs..usa_ofs + 2].copy_from_slice(&usn.to_le_bytes());
    for i in 1..usa_cnt {
        let end = i * 512 - 2;
        let (saved0, saved1) = (rec[end], rec[end + 1]);
        rec[usa_ofs + 2 * i] = saved0;
        rec[usa_ofs + 2 * i + 1] = saved1;
        rec[end..end + 2].copy_from_slice(&usn.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(size: usize) -> Vec<u8> {
        let mut r: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
        r[0..4].copy_from_slice(b"FILE");
        r
    }

    #[test]
    fn roundtrip_1k_and_4k() {
        for size in [1024usize, 4096] {
            let original = record(size);
            let mut r = original.clone();
            protect(&mut r, 0x30, 0x0042);
            assert_ne!(r, original);
            apply_fixups(&mut r).unwrap();
            // Everything except the header fields we wrote must match the original.
            assert_eq!(r[0x30 + 2 * (size / 512 + 1)..], original[0x30 + 2 * (size / 512 + 1)..]);
            assert_eq!(u16_at(&r, 6), Some((size / 512 + 1) as u16));
        }
    }

    #[test]
    fn detects_torn_write() {
        let mut r = record(1024);
        protect(&mut r, 0x30, 7);
        r[1022] ^= 0xFF;
        assert!(apply_fixups(&mut r).is_err());
    }

    #[test]
    fn rejects_bad_counts() {
        let mut r = record(1024);
        protect(&mut r, 0x30, 7);
        r[6] = 5; // claims 4 strides of 256 bytes
        assert!(apply_fixups(&mut r).is_err());
        let mut r = record(1024);
        protect(&mut r, 0x30, 7);
        r[4] = 0x31; // odd offset
        assert!(apply_fixups(&mut r).is_err());
    }
}
