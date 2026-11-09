//! Small formatting helpers shared by the CLI and the GUI.

/// Formats a byte count with binary units, e.g. `1.50 GiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let decimals = if value >= 100.0 { 0 } else if value >= 10.0 { 1 } else { 2 };
    format!("{value:.decimals$} {}", UNITS[unit])
}

/// Formats an integer with thousands separators, e.g. `1,234,567`.
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Percentage of `part` in `whole` (0 when `whole` is 0).
pub fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 * 100.0 / whole as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.00 KiB");
        assert_eq!(format_size(1536), "1.50 KiB");
        assert_eq!(format_size(15 * 1024 * 1024), "15.0 MiB");
        assert_eq!(format_size(500 * 1024 * 1024 * 1024), "500 GiB");
    }

    #[test]
    fn counts() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
