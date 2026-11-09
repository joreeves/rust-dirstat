//! Non-resident attribute run lists ("mapping pairs") and the VCN -> LCN extent map.

use super::error::ParseError;

/// One run of a decoded run list: `len` clusters at `lcn`, or a sparse run when `lcn` is None.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: u64,
    pub lcn: Option<u64>,
}

/// Decodes a run list. Each entry is a header byte `0xOL` followed by an L-byte unsigned
/// length and an O-byte signed LCN delta (relative to the previous run's LCN; O = 0 means a
/// sparse run that does not move the LCN). A zero header terminates the list.
pub fn decode_runs(p: &[u8], total_clusters: u64) -> Result<Vec<Run>, ParseError> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    loop {
        let header = *p.get(pos).ok_or(ParseError("run list not terminated"))?;
        if header == 0 {
            return Ok(runs);
        }
        pos += 1;
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return Err(ParseError("invalid run header"));
        }
        let len = read_le(p, pos, len_size)?;
        pos += len_size;
        if len == 0 || len > i64::MAX as u64 {
            return Err(ParseError("invalid run length"));
        }
        if off_size == 0 {
            runs.push(Run { len, lcn: None });
            continue;
        }
        let raw = read_le(p, pos, off_size)?;
        pos += off_size;
        let delta = sign_extend(raw, off_size);
        lcn = lcn.checked_add(delta).ok_or(ParseError("run LCN overflow"))?;
        if lcn < 0 {
            return Err(ParseError("negative run LCN"));
        }
        let start = lcn as u64;
        if start.checked_add(len).is_none_or(|end| end > total_clusters) {
            return Err(ParseError("run outside the volume"));
        }
        runs.push(Run { len, lcn: Some(start) });
    }
}

fn read_le(p: &[u8], pos: usize, n: usize) -> Result<u64, ParseError> {
    let bytes = p.get(pos..pos + n).ok_or(ParseError("run list truncated"))?;
    Ok(bytes.iter().rev().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

fn sign_extend(v: u64, n: usize) -> i64 {
    let shift = 64 - 8 * n as u32;
    ((v << shift) as i64) >> shift
}

/// A contiguous piece of an attribute's data: `len` clusters starting at virtual cluster `vcn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub vcn: u64,
    pub lcn: Option<u64>,
    pub len: u64,
}

/// VCN -> LCN map assembled from one or more run lists, kept sorted by VCN.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extents(pub Vec<Extent>);

impl Extents {
    /// Adds the runs of an attribute piece whose first VCN is `start_vcn`.
    pub fn insert_runs(&mut self, start_vcn: u64, runs: &[Run]) {
        let mut vcn = start_vcn;
        for r in runs {
            if !self.0.iter().any(|e| e.vcn == vcn) {
                self.0.push(Extent { vcn, lcn: r.lcn, len: r.len });
            }
            vcn += r.len;
        }
        self.0.sort_by_key(|e| e.vcn);
    }

    /// Number of clusters covered contiguously from VCN 0 by non-sparse extents.
    pub fn contiguous_clusters(&self) -> u64 {
        let mut next = 0u64;
        for e in &self.0 {
            if e.vcn != next || e.lcn.is_none() {
                break;
            }
            next += e.len;
        }
        next
    }

    /// Maps `vcn` to `(lcn, clusters remaining in that extent from vcn)`.
    pub fn map(&self, vcn: u64) -> Option<(Option<u64>, u64)> {
        let idx = self.0.partition_point(|e| e.vcn <= vcn).checked_sub(1)?;
        let e = self.0.get(idx)?;
        let within = vcn - e.vcn;
        if within >= e.len {
            return None;
        }
        Some((e.lcn.map(|l| l + within), e.len - within))
    }
}

/// Test helper: encodes runs in the on-disk format (used for round-trip tests and test images).
#[cfg(test)]
pub fn encode_runs(runs: &[Run]) -> Vec<u8> {
    fn min_unsigned(v: u64) -> usize {
        ((64 - v.leading_zeros() as usize).div_ceil(8)).max(1)
    }
    fn min_signed(v: i64) -> usize {
        (1..=8).find(|&n| {
            let shift = 64 - 8 * n as u32;
            ((v << shift) >> shift) == v
        }).unwrap()
    }
    let mut out = Vec::new();
    let mut prev = 0i64;
    for r in runs {
        let ls = min_unsigned(r.len);
        let (os, delta) = match r.lcn {
            Some(l) => {
                let d = l as i64 - prev;
                prev = l as i64;
                (min_signed(d), d)
            }
            None => (0, 0),
        };
        out.push(((os as u8) << 4) | ls as u8);
        out.extend_from_slice(&r.len.to_le_bytes()[..ls]);
        out.extend_from_slice(&delta.to_le_bytes()[..os]);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: u64 = u64::MAX / 4;

    #[test]
    fn single_run() {
        let runs = decode_runs(&[0x21, 0x18, 0x34, 0x56, 0x00], BIG).unwrap();
        assert_eq!(runs, vec![Run { len: 0x18, lcn: Some(0x5634) }]);
    }

    #[test]
    fn negative_delta() {
        let runs = decode_runs(&[0x11, 0x30, 0x60, 0x11, 0x10, 0xF0, 0x00], BIG).unwrap();
        assert_eq!(runs, vec![Run { len: 0x30, lcn: Some(0x60) }, Run { len: 0x10, lcn: Some(0x50) }]);
    }

    #[test]
    fn sparse_run_keeps_lcn() {
        let runs = decode_runs(&[0x11, 0x04, 0x20, 0x01, 0x10, 0x11, 0x04, 0x08, 0x00], BIG).unwrap();
        assert_eq!(
            runs,
            vec![
                Run { len: 4, lcn: Some(0x20) },
                Run { len: 0x10, lcn: None },
                Run { len: 4, lcn: Some(0x28) },
            ]
        );
    }

    #[test]
    fn eight_byte_offset() {
        let mut p = vec![0x81, 0x01];
        p.extend_from_slice(&0x0000_0100_0000_0000u64.to_le_bytes());
        p.push(0);
        assert_eq!(decode_runs(&p, BIG).unwrap(), vec![Run { len: 1, lcn: Some(0x0000_0100_0000_0000) }]);
    }

    #[test]
    fn rejects_malformed() {
        assert!(decode_runs(&[0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], BIG).is_err()); // L > 8
        assert!(decode_runs(&[0x90, 0x01], BIG).is_err()); // O > 8 and L == 0
        assert!(decode_runs(&[0x21, 0x18, 0x34], BIG).is_err()); // truncated
        assert!(decode_runs(&[0x11, 0x01, 0x05], BIG).is_err()); // no terminator
        assert!(decode_runs(&[0x11, 0x01, 0xF0, 0x00], BIG).is_err()); // negative LCN
        assert!(decode_runs(&[0x11, 0x10, 0x7F, 0x00], 0x80).is_err()); // past the volume
        assert!(decode_runs(&[0x11, 0x00, 0x05, 0x00], BIG).is_err()); // zero length
    }

    #[test]
    fn encode_roundtrip() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..500 {
            let n = (next() % 8 + 1) as usize;
            let runs: Vec<Run> = (0..n)
                .map(|_| Run {
                    len: next() % 100_000 + 1,
                    lcn: if next() % 5 == 0 { None } else { Some(next() % (1 << 40)) },
                })
                .collect();
            assert_eq!(decode_runs(&encode_runs(&runs), BIG).unwrap(), runs);
        }
    }

    #[test]
    fn extent_map() {
        let mut ex = Extents::default();
        ex.insert_runs(0, &[Run { len: 4, lcn: Some(100) }, Run { len: 2, lcn: Some(50) }]);
        ex.insert_runs(6, &[Run { len: 10, lcn: Some(200) }]);
        assert_eq!(ex.contiguous_clusters(), 16);
        assert_eq!(ex.map(0), Some((Some(100), 4)));
        assert_eq!(ex.map(3), Some((Some(103), 1)));
        assert_eq!(ex.map(5), Some((Some(51), 1)));
        assert_eq!(ex.map(15), Some((Some(209), 1)));
        assert_eq!(ex.map(16), None);
        let mut gap = Extents::default();
        gap.insert_runs(0, &[Run { len: 4, lcn: Some(1) }]);
        gap.insert_runs(8, &[Run { len: 4, lcn: Some(9) }]);
        assert_eq!(gap.contiguous_clusters(), 4);
    }
}
