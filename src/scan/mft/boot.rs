//! NTFS boot sector (the first sector of the volume).

use super::error::{OrCorrupt, ParseError};
use super::le::{i8_at, slice_at, u8_at, u16_at, u64_at};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootSector {
    pub bytes_per_sector: u32,
    pub cluster_size: u64,
    pub total_clusters: u64,
    pub mft_lcn: u64,
    /// Size of one MFT FILE record in bytes (usually 1024 or 4096).
    pub record_size: u32,
    pub serial: u64,
}

impl BootSector {
    pub fn volume_bytes(&self) -> u64 {
        self.total_clusters.saturating_mul(self.cluster_size)
    }
}

pub fn parse_boot_sector(b: &[u8]) -> Result<BootSector, ParseError> {
    if slice_at(b, 3, 8) != Some(b"NTFS    ") {
        return Err(ParseError("not an NTFS boot sector"));
    }
    if u16_at(b, 0x1FE) != Some(0xAA55) {
        return Err(ParseError("missing boot sector signature"));
    }
    let bytes_per_sector = u32::from(u16_at(b, 0x0B).or_corrupt("boot sector")?);
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return Err(ParseError("invalid bytes per sector"));
    }
    let spc_raw = u8_at(b, 0x0D).or_corrupt("boot sector")?;
    let sectors_per_cluster: u64 = match spc_raw {
        0 => return Err(ParseError("invalid sectors per cluster")),
        1..=0x80 => u64::from(spc_raw),
        // Values above 0x80 encode 2^(256 - v); used for clusters larger than 64 KiB.
        _ => {
            let shift = 256 - u32::from(spc_raw);
            if shift > 20 {
                return Err(ParseError("invalid sectors per cluster"));
            }
            1u64 << shift
        }
    };
    if !sectors_per_cluster.is_power_of_two() {
        return Err(ParseError("invalid sectors per cluster"));
    }
    let cluster_size = u64::from(bytes_per_sector) * sectors_per_cluster;
    if cluster_size > 2 * 1024 * 1024 {
        return Err(ParseError("cluster size too large"));
    }
    let total_sectors = u64_at(b, 0x28).or_corrupt("boot sector")?;
    let total_clusters = total_sectors / sectors_per_cluster;
    let mft_lcn = u64_at(b, 0x30).or_corrupt("boot sector")?;
    if total_clusters == 0 || mft_lcn == 0 || mft_lcn >= total_clusters {
        return Err(ParseError("$MFT location outside the volume"));
    }
    let cpr = i8_at(b, 0x40).or_corrupt("boot sector")?;
    let record_size: u64 = match cpr {
        0 => return Err(ParseError("invalid MFT record size")),
        1.. => u64::from(cpr.unsigned_abs()) * cluster_size,
        // Negative values encode 2^(-v) bytes, e.g. 0xF6 (-10) = 1024.
        _ => {
            let shift = u32::from(cpr.unsigned_abs());
            if shift > 16 {
                return Err(ParseError("invalid MFT record size"));
            }
            1u64 << shift
        }
    };
    if !record_size.is_power_of_two() || !(512..=65536).contains(&record_size) {
        return Err(ParseError("invalid MFT record size"));
    }
    let serial = u64_at(b, 0x48).or_corrupt("boot sector")?;
    Ok(BootSector {
        bytes_per_sector,
        cluster_size,
        total_clusters,
        mft_lcn,
        record_size: record_size as u32,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn boot_bytes(bps: u16, spc: u8, cpr: u8, total_sectors: u64, mft_lcn: u64) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        b[0x0B..0x0D].copy_from_slice(&bps.to_le_bytes());
        b[0x0D] = spc;
        b[0x28..0x30].copy_from_slice(&total_sectors.to_le_bytes());
        b[0x30..0x38].copy_from_slice(&mft_lcn.to_le_bytes());
        b[0x40] = cpr;
        b[0x48..0x50].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        b[0x1FE] = 0x55;
        b[0x1FF] = 0xAA;
        b
    }

    #[test]
    fn typical_4k_clusters_1k_records() {
        let bs = parse_boot_sector(&boot_bytes(512, 8, 0xF6, 1_000_000, 786_432 / 8)).unwrap();
        assert_eq!(bs.cluster_size, 4096);
        assert_eq!(bs.record_size, 1024);
        assert_eq!(bs.total_clusters, 125_000);
        assert_eq!(bs.serial, 0x1122_3344_5566_7788);
    }

    #[test]
    fn record_size_encodings() {
        assert_eq!(parse_boot_sector(&boot_bytes(4096, 1, 0xF4, 1000, 10)).unwrap().record_size, 4096);
        // Positive: clusters per record (1 cluster of 1024 bytes).
        let bs = parse_boot_sector(&boot_bytes(512, 2, 1, 4000, 10)).unwrap();
        assert_eq!((bs.cluster_size, bs.record_size), (1024, 1024));
        assert!(parse_boot_sector(&boot_bytes(512, 8, 0, 1000, 10)).is_err());
    }

    #[test]
    fn large_cluster_encoding() {
        // 0x80 = 128 sectors (64 KiB); 0xF8 = 2^8 = 256 sectors (128 KiB).
        assert_eq!(parse_boot_sector(&boot_bytes(512, 0x80, 0xF6, 1 << 24, 10)).unwrap().cluster_size, 65536);
        assert_eq!(parse_boot_sector(&boot_bytes(512, 0xF8, 0xF6, 1 << 24, 10)).unwrap().cluster_size, 131072);
    }

    #[test]
    fn rejects_bad_headers() {
        let mut b = boot_bytes(512, 8, 0xF6, 1000, 10);
        b[3..11].copy_from_slice(b"EXFAT   ");
        assert!(parse_boot_sector(&b).is_err());
        let mut b = boot_bytes(512, 8, 0xF6, 1000, 10);
        b[0x1FF] = 0;
        assert!(parse_boot_sector(&b).is_err());
        assert!(parse_boot_sector(&boot_bytes(500, 8, 0xF6, 1000, 10)).is_err());
        assert!(parse_boot_sector(&boot_bytes(512, 8, 0xF6, 1000, 5000)).is_err());
        assert!(parse_boot_sector(&[0u8; 100]).is_err());
    }
}
