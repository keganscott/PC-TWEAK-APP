//! The support bundle (NOTES N137): one zip file with what this PC's
//! PeakTweaks knows — the system audit, the tools and their states, the
//! change record, the settings and the play history — so a tester can send
//! one file instead of screenshots. Nothing leaves the PC by itself; the
//! app writes the file and shows where it is.
//!
//! The zip is written by hand (stored entries, no compression): the texts
//! are small, and a new crate for this alone is not worth its weight in the
//! no-networking build. CRC-32 is the zip standard's own checksum.

use serde::Serialize;
use ts_rs::TS;

use crate::error::{EngineError, Result};

/// What was written, for the UI to show.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundle {
    /// The full path of the zip file.
    pub path: String,
    /// The files inside it, in order.
    pub entries: Vec<String>,
    /// The zip's size in bytes.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// CRC-32 (IEEE, as zip uses), table built at compile time.
fn crc32(bytes: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    }
    static TABLE: [u32; 256] = table();
    let mut crc = !0u32;
    for &b in bytes {
        crc = TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// (year, month, day, hour, minute, second) in UTC for a Unix millisecond
/// stamp: the civil-from-days conversion, checked against known dates in
/// the tests.
pub fn civil(unix_ms: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs = unix_ms / 1000;
    let days = (secs / 86_400) as i64;
    let day_secs = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (
        year as u32,
        month as u32,
        day as u32,
        (day_secs / 3600) as u32,
        ((day_secs % 3600) / 60) as u32,
        (day_secs % 60) as u32,
    )
}

/// The bundle's file name for a moment: `peaktweaks-support-20261016-043000.zip`.
pub fn bundle_name(unix_ms: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(unix_ms);
    format!("peaktweaks-support-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.zip")
}

fn dos_time(unix_ms: u64) -> (u16, u16) {
    let (_, mo, d, h, mi, s) = civil(unix_ms);
    let (y, ..) = civil(unix_ms);
    let date = (((y.saturating_sub(1980)) & 0x7F) << 9) | ((mo & 0xF) << 5) | (d & 0x1F);
    let time = ((h & 0x1F) << 11) | ((mi & 0x3F) << 5) | ((s / 2) & 0x1F);
    (date as u16, time as u16)
}

/// A stored (uncompressed) zip of `(name, text)` entries.
pub fn zip_stored(entries: &[(String, String)], unix_ms: u64) -> Result<Vec<u8>> {
    let (date, time) = dos_time(unix_ms);
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, text) in entries {
        let data = text.as_bytes();
        if data.len() > u32::MAX as usize || name.len() > u16::MAX as usize {
            return Err(EngineError::Internal {
                detail: "a support bundle entry is too large".into(),
            });
        }
        let crc = crc32(data);
        let offset = out.len() as u32;
        let size = data.len() as u32;
        // Local file header.
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        // Central directory entry.
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        // 2026-10-15 22:50:00 UTC.
        assert_eq!(civil(1_792_104_600_000), (2026, 10, 15, 22, 50, 0));
        assert_eq!(bundle_name(1_792_104_600_000), "peaktweaks-support-20261015-225000.zip");
    }

    #[test]
    fn a_zip_reads_back_with_its_entries_and_checksums() {
        let entries = vec![
            ("a.json".to_string(), "{\"x\":1}".to_string()),
            ("b.txt".to_string(), "hello".to_string()),
        ];
        let zip = zip_stored(&entries, 1_792_104_600_000).unwrap();
        // End of central directory: last 22 bytes.
        let eocd = &zip[zip.len() - 22..];
        assert_eq!(&eocd[0..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 2);
        // First local header names the first entry, and its data follows.
        assert_eq!(&zip[0..4], b"PK\x03\x04");
        let name_len = u16::from_le_bytes([zip[26], zip[27]]) as usize;
        assert_eq!(&zip[30..30 + name_len], b"a.json");
        let size = u32::from_le_bytes([zip[18], zip[19], zip[20], zip[21]]) as usize;
        let data = &zip[30 + name_len..30 + name_len + size];
        assert_eq!(data, b"{\"x\":1}");
        let crc = u32::from_le_bytes([zip[14], zip[15], zip[16], zip[17]]);
        assert_eq!(crc, crc32(data));
    }
}
