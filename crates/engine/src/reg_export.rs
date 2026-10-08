//! `.reg` backup export.
//!
//! Written as UTF-16LE with a BOM, which is what `regedit.exe` requires for the
//! "Windows Registry Editor Version 5.00" header. A UTF-8 `.reg` file imports
//! as mojibake and is a classic silent-corruption bug.

use std::path::{Path, PathBuf};

use super::error::Result;
use super::fsutil;
use super::types::RawValue;

/// Emit a `.reg` file restoring one value to `previous`, and make it durable.
pub fn write_reg_backup(
    dir: &Path,
    file_stem: &str,
    display_path: &str,
    value_name: &str,
    previous: Option<&RawValue>,
) -> Result<PathBuf> {
    fsutil::create_dir_durable(dir)?;
    let path = dir.join(format!("{file_stem}.reg"));
    fsutil::write_durable(&path, &reg_file_bytes(display_path, value_name, previous))?;
    Ok(path)
}

/// One file that restores a whole applied change: every value's prior state,
/// newest write first, so importing it once undoes the change by hand.
/// `values` are `(display_path, value_name, previous)` in write order.
pub fn write_session_backup(
    dir: &Path,
    file_stem: &str,
    values: &[(&str, &str, Option<&RawValue>)],
) -> Result<PathBuf> {
    fsutil::create_dir_durable(dir)?;
    let path = dir.join(format!("{file_stem}.reg"));
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    for (display_path, name, previous) in values.iter().rev() {
        text.push_str(&format!(
            "[{display_path}]\r\n{}\r\n\r\n",
            reg_value_line(name, *previous)
        ));
    }
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    fsutil::write_durable(&path, &bytes)?;
    Ok(path)
}

/// The exact bytes of a backup file: BOM, header, key, one value line.
pub fn reg_file_bytes(display_path: &str, value_name: &str, previous: Option<&RawValue>) -> Vec<u8> {
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    text.push_str(&format!("[{display_path}]\r\n"));
    text.push_str(&format!("{}\r\n", reg_value_line(value_name, previous)));

    let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A string is safe to write as a quoted `.reg` string only if it round-trips
/// exactly (single trailing NUL, valid UTF-16) and has no control characters,
/// which would break the line. Anything else uses typed hex, which carries the
/// exact bytes. One thing it cannot carry: `reg.exe import` appends a NUL to a
/// string value stored without one (measured, registry::contract_tests).
/// In-app Undo restores from the journal and is exact either way.
fn is_plain_string(v: &RawValue) -> bool {
    v.vtype == 1
        && v.as_sz()
            .is_some_and(|s| !s.chars().any(char::is_control) && RawValue::sz(&s) == *v)
}

/// One `.reg` value line. `None` produces a deletion directive. The empty
/// name is the key's default value, written `@`.
pub fn reg_value_line(name: &str, value: Option<&RawValue>) -> String {
    let lhs = if name.is_empty() {
        "@".to_owned()
    } else {
        format!("\"{}\"", escape(name))
    };

    let Some(v) = value else {
        // The value did not exist. Restoring means removing it.
        return format!("{lhs}=-");
    };

    match v.vtype {
        1 if is_plain_string(v) => {
            let s = v.as_sz().unwrap_or_default();
            format!("{lhs}=\"{}\"", escape(&s))
        }
        4 if v.bytes.len() == 4 => format!("{lhs}=dword:{:08x}", v.as_dword().unwrap_or(0)),
        3 => format!("{lhs}=hex:{}", hex_wrapped(&v.bytes, lhs.len() + 5)),
        // Everything else uses the typed hex form, exact for any bytes:
        // hex(1) sz that is not plain, hex(2) expand_sz, hex(4) malformed
        // dword, hex(7) multi_sz, hex(b) qword.
        other => format!("{lhs}=hex({:x}):{}", other, hex_wrapped(&v.bytes, lhs.len() + 10)),
    }
}

/// Comma-separated hex with `\` continuations. `.reg` lines wrap at 80 columns;
/// regedit tolerates longer but other parsers do not, so we conform.
pub fn hex_wrapped(bytes: &[u8], first_line_prefix: usize) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut col = first_line_prefix;

    for (i, b) in bytes.iter().enumerate() {
        let last = i == bytes.len() - 1;
        let chunk = if last { format!("{b:02x}") } else { format!("{b:02x},") };

        if col + chunk.len() > 76 {
            out.push_str("\\\r\n  ");
            col = 2;
        }
        col += chunk.len();
        out.push_str(&chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_value_becomes_a_deletion_directive() {
        assert_eq!(
            reg_value_line("Win32PrioritySeparation", None),
            "\"Win32PrioritySeparation\"=-"
        );
    }

    #[test]
    fn the_default_value_is_written_as_at() {
        assert_eq!(reg_value_line("", None), "@=-");
        assert_eq!(reg_value_line("", Some(&RawValue::sz(""))), "@=\"\"");
        assert_eq!(reg_value_line("", Some(&RawValue::dword(1))), "@=dword:00000001");
    }

    #[test]
    fn dword_is_eight_hex_digits() {
        let line = reg_value_line("X", Some(&RawValue::dword(0x26)));
        assert_eq!(line, "\"X\"=dword:00000026");
    }

    #[test]
    fn sz_roundtrips_through_utf16() {
        let v = RawValue::sz("0");
        assert_eq!(v.as_sz().as_deref(), Some("0"));
        assert_eq!(reg_value_line("MouseSpeed", Some(&v)), "\"MouseSpeed\"=\"0\"");
    }

    #[test]
    fn long_binary_values_wrap() {
        let v = RawValue {
            vtype: 3,
            bytes: vec![0xAB; 64],
        };
        let line = reg_value_line("Curve", Some(&v));
        assert!(line.contains("\\\r\n  "), "expected a continuation, got: {line}");
        for l in line.split("\r\n") {
            assert!(l.len() <= 80, "line too long ({}): {l}", l.len());
        }
    }

    #[test]
    fn backslashes_and_quotes_in_names_and_strings_are_escaped() {
        assert_eq!(reg_value_line("a\\b", None), "\"a\\\\b\"=-");
        assert_eq!(
            reg_value_line("n\"q", Some(&RawValue::sz(r#"C:\x"y"#))),
            r#""n\"q"="C:\\x\"y""#
        );
    }

    #[test]
    fn malformed_dword_is_exported_as_typed_hex_not_zero() {
        let v = RawValue {
            vtype: 4,
            bytes: vec![1, 2, 3],
        };
        assert_eq!(reg_value_line("X", Some(&v)), "\"X\"=hex(4):01,02,03");
    }

    #[test]
    fn strings_that_do_not_round_trip_use_typed_hex() {
        // Odd byte count: not valid UTF-16.
        let odd = RawValue {
            vtype: 1,
            bytes: vec![0x41, 0x00, 0x00],
        };
        assert_eq!(reg_value_line("X", Some(&odd)), "\"X\"=hex(1):41,00,00");
        // Control character would break the line.
        let nl = RawValue::sz("a\nb");
        assert!(reg_value_line("X", Some(&nl)).starts_with("\"X\"=hex(1):61,00,0a,00"));
        // Data after the first NUL.
        let mut trailing = RawValue::sz("a").bytes;
        trailing.extend_from_slice(&[0x62, 0x00, 0x00, 0x00]);
        let v = RawValue {
            vtype: 1,
            bytes: trailing,
        };
        assert!(reg_value_line("X", Some(&v)).starts_with("\"X\"=hex(1):"));
    }

    #[test]
    fn typed_hex_for_other_types() {
        let q = RawValue {
            vtype: 11,
            bytes: 5u64.to_le_bytes().to_vec(),
        };
        assert_eq!(reg_value_line("Q", Some(&q)), "\"Q\"=hex(b):05,00,00,00,00,00,00,00");
        let empty = RawValue {
            vtype: 3,
            bytes: vec![],
        };
        assert_eq!(reg_value_line("E", Some(&empty)), "\"E\"=hex:");
    }

    #[test]
    fn file_is_utf16le_with_bom_crlf_and_deletion_directive() {
        let bytes = reg_file_bytes(r"HKEY_USERS\S-1-5-21\Control Panel\Mouse", "MouseSpeed", None);
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units = crate::types::utf16le_units(&bytes[2..]);
        let text = String::from_utf16(&units).unwrap();
        assert_eq!(
            text,
            "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_USERS\\S-1-5-21\\Control Panel\\Mouse]\r\n\"MouseSpeed\"=-\r\n"
        );
    }
}
