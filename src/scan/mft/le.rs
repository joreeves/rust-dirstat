//! Bounds-checked little-endian readers. Every on-disk read in the MFT parser goes through
//! these so malformed records produce errors instead of panics.

#[inline]
pub fn u8_at(b: &[u8], o: usize) -> Option<u8> {
    b.get(o).copied()
}

#[inline]
pub fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

#[inline]
pub fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

#[inline]
pub fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o.checked_add(8)?)?.try_into().ok()?))
}

#[inline]
pub fn i8_at(b: &[u8], o: usize) -> Option<i8> {
    b.get(o).map(|&v| v as i8)
}

/// Returns `b[o..o + len]` if it is in bounds.
#[inline]
pub fn slice_at(b: &[u8], o: usize, len: usize) -> Option<&[u8]> {
    b.get(o..o.checked_add(len)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_bounds() {
        let b = [1u8, 2, 3, 4, 5, 6, 7, 8, 0xFF];
        assert_eq!(u16_at(&b, 0), Some(0x0201));
        assert_eq!(u32_at(&b, 0), Some(0x0403_0201));
        assert_eq!(u64_at(&b, 0), Some(0x0807_0605_0403_0201));
        assert_eq!(i8_at(&b, 8), Some(-1));
        assert_eq!(u64_at(&b, 2), None);
        assert_eq!(u16_at(&b, usize::MAX), None);
        assert_eq!(slice_at(&b, 7, 2), Some(&[8u8, 0xFF][..]));
        assert_eq!(slice_at(&b, 8, 2), None);
    }
}
