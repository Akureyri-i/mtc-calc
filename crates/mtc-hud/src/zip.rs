//! A zip archive of stored (uncompressed) files: a problem report as one file a player can send
//! (a chat program takes a file, not a folder). Its screenshots are compressed already.

use std::path::Path;

/// The CRC-32 the zip format checks each file by (IEEE, reflected).
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    !data.iter().fold(!0u32, |c, &b| table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8))
}

/// Now in the DOS time and date the zip headers carry (UTC).
fn dos_now() -> (u16, u16) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = ((rest / 3_600) << 11 | (rest % 3_600 / 60) << 5 | (rest % 60) / 2) as u16;
    let date = (((year - 1980).clamp(0, 127) as u16) << 9) | ((month as u16) << 5) | day as u16;
    (time, date)
}

/// Writes `files` (name in the archive, contents) as a zip at `path`.
pub fn write_stored(path: &Path, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let (time, date) = dos_now();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let (crc, size, name_len) = (crc32(data), data.len() as u32, name.len() as u16);
        // Version 2.0, UTF-8 names (flag bit 11), stored.
        let common = |b: &mut Vec<u8>| {
            for v in [20u16, 0x0800, 0, time, date] {
                b.extend(v.to_le_bytes());
            }
            for v in [crc, size, size] {
                b.extend(v.to_le_bytes());
            }
            b.extend(name_len.to_le_bytes());
            b.extend(0u16.to_le_bytes());
        };
        out.extend(0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        common(&mut central);
        // Comment length, disk, internal and external attributes, the local header's offset.
        for v in [0u16, 0, 0] {
            central.extend(v.to_le_bytes());
        }
        central.extend(0u32.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let (cd_offset, cd_size, count) = (out.len() as u32, central.len() as u32, files.len() as u16);
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, count, count] {
        out.extend(v.to_le_bytes());
    }
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }

    /// The archive's layout: two local headers with their names and data, the central directory
    /// pointing at them, the end record counting them.
    #[test]
    fn a_stored_archive_lays_out_its_files() {
        let path = std::env::temp_dir().join(format!("mtc-zip-test-{}.zip", std::process::id()));
        super::write_stored(&path, &[("report.txt".into(), b"hello".to_vec()), ("log.txt".into(), b"line\n".to_vec())]).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        // `MTC_ZIP_TEST_KEEP`: left for another reader to open.
        if std::env::var_os("MTC_ZIP_TEST_KEEP").is_none() {
            let _ = std::fs::remove_file(&path);
        }
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        assert_eq!(u32_at(0), 0x0403_4b50);
        assert_eq!(&bytes[30..40], b"report.txt");
        assert_eq!(&bytes[40..45], b"hello");
        let end = bytes.len() - 22;
        assert_eq!((u32_at(end), u16_at(end + 10)), (0x0605_4b50, 2));
        let cd = u32_at(end + 16) as usize;
        assert_eq!(u32_at(cd), 0x0201_4b50);
        assert_eq!(u32_at(cd + 16), super::crc32(b"hello"));
        assert_eq!(u32_at(cd + 42), 0, "the first file's offset");
    }
}
