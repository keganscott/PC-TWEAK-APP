//! Editing one `Key=Value` line of an INI file (game settings, plan section 8)
//! while leaving every other byte as it was: comments, order, blank lines,
//! line endings, the byte-order mark and the encoding (UTF-8 or UTF-16LE).
//!
//! Section and key names match case-insensitively, as Windows' own INI
//! functions do. Unreal Engine files (Fortnite's `GameUserSettings.ini`) also
//! use array lines (`+Key=`, `-Key=`, `.Key=`, `!Key=`) and may repeat a key;
//! a key that appears that way, or more than once in its section, is refused
//! as ambiguous rather than guessed at.

use super::error::{EngineError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16LeBom,
}

/// A decoded INI file and how to write it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniText {
    pub encoding: Encoding,
    pub text: String,
}

fn bad(detail: impl Into<String>) -> EngineError {
    EngineError::Internal { detail: detail.into() }
}

pub fn decode(bytes: &[u8]) -> Result<IniText> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return Err(bad("UTF-16 INI file has an odd number of bytes"));
        }
        let units: Vec<u16> = rest.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&units).map_err(|_| bad("INI file is not valid UTF-16"))?;
        return Ok(IniText {
            encoding: Encoding::Utf16LeBom,
            text,
        });
    }
    let (encoding, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (Encoding::Utf8Bom, rest),
        None => (Encoding::Utf8, bytes),
    };
    let text = String::from_utf8(body.to_vec()).map_err(|_| bad("INI file is neither UTF-8 nor UTF-16"))?;
    Ok(IniText { encoding, text })
}

pub fn encode(ini: &IniText) -> Vec<u8> {
    match ini.encoding {
        Encoding::Utf8 => ini.text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], ini.text.as_bytes()].concat(),
        Encoding::Utf16LeBom => {
            let mut out = vec![0xFF, 0xFE];
            out.extend(ini.text.encode_utf16().flat_map(u16::to_le_bytes));
            out
        }
    }
}

/// One line with its terminator, as found.
struct Line<'a> {
    body: &'a str,
    end: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (body, end, next) = match rest.find('\n') {
            Some(i) if i > 0 && rest.as_bytes()[i - 1] == b'\r' => (&rest[..i - 1], &rest[i - 1..=i], &rest[i + 1..]),
            Some(i) => (&rest[..i], &rest[i..=i], &rest[i + 1..]),
            None => (rest, "", ""),
        };
        out.push(Line { body, end });
        rest = next;
    }
    out
}

/// The newline the file mostly uses, `\r\n` when there is none yet.
fn newline(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if lf > crlf {
        "\n"
    } else {
        "\r\n"
    }
}

fn section_name(body: &str) -> Option<&str> {
    let t = body.trim();
    t.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

/// `(key, value)` of a `Key=Value` line, with any array prefix kept on the key.
fn key_value(body: &str) -> Option<(&str, &str)> {
    let t = body.trim_start();
    if t.starts_with(';') || t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let eq = t.find('=')?;
    Some((t[..eq].trim(), &t[eq + 1..]))
}

/// Where a key is in a section: the index of the one line that sets it, the
/// index after the section's last non-blank line, and whether the section
/// exists. Refuses ambiguous keys.
struct Located {
    key_line: Option<usize>,
    insert_at: Option<usize>,
}

fn locate(lines: &[Line<'_>], section: &str, key: &str) -> Result<Located> {
    let mut in_section = false;
    let mut found = Located {
        key_line: None,
        insert_at: None,
    };
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = section_name(line.body) {
            in_section = name.eq_ignore_ascii_case(section);
            if in_section {
                found.insert_at = Some(i + 1);
            }
            continue;
        }
        if !in_section {
            continue;
        }
        if !line.body.trim().is_empty() {
            found.insert_at = Some(i + 1);
        }
        let Some((k, _)) = key_value(line.body) else { continue };
        let bare = k.trim_start_matches(['+', '-', '.', '!']);
        if !bare.eq_ignore_ascii_case(key) {
            continue;
        }
        if bare.len() != k.len() {
            return Err(bad(format!("[{section}] {key} is an array entry ({k}); not edited")));
        }
        if found.key_line.is_some() {
            return Err(bad(format!("[{section}] {key} appears more than once; not edited")));
        }
        found.key_line = Some(i);
    }
    Ok(found)
}

/// The value of `[section] key`, `None` when it is not set.
pub fn get(text: &str, section: &str, key: &str) -> Result<Option<String>> {
    let lines = lines(text);
    let at = locate(&lines, section, key)?;
    Ok(at
        .key_line
        .and_then(|i| key_value(lines[i].body))
        .map(|(_, v)| v.to_owned()))
}

/// Set `[section] key=value`: the existing line is rewritten in place (its key
/// spelling and indentation kept), or a line is added at the end of the
/// section, or the section is added at the end of the file.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> Result<String> {
    if value.contains(['\r', '\n']) || key.contains(['=', '\r', '\n', '[', ']']) || key.trim().is_empty() {
        return Err(bad(format!("refusing to write [{section}] {key:?}={value:?}")));
    }
    let nl = newline(text);
    let lines = lines(text);
    let at = locate(&lines, section, key)?;
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 8);
    match (at.key_line, at.insert_at) {
        (Some(i), _) => {
            for (j, line) in lines.iter().enumerate() {
                if j == i {
                    let eq = line.body.find('=').expect("a key line has '='");
                    out.push_str(&line.body[..=eq]);
                    out.push_str(value);
                } else {
                    out.push_str(line.body);
                }
                out.push_str(line.end);
            }
        }
        (None, Some(insert)) => {
            for (j, line) in lines.iter().enumerate() {
                if j == insert {
                    out.push_str(&format!("{key}={value}{nl}"));
                }
                out.push_str(line.body);
                // The section's last line may have been the file's last line,
                // with no newline after it.
                out.push_str(if line.end.is_empty() && j + 1 == insert {
                    nl
                } else {
                    line.end
                });
            }
            if insert == lines.len() {
                out.push_str(&format!("{key}={value}{nl}"));
            }
        }
        (None, None) => {
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(nl);
            }
            if !text.is_empty() {
                out.push_str(nl);
            }
            out.push_str(&format!("[{section}]{nl}{key}={value}{nl}"));
        }
    }
    Ok(out)
}

/// Remove the line that sets `[section] key`. Unchanged when it is not set.
pub fn remove(text: &str, section: &str, key: &str) -> Result<String> {
    let lines = lines(text);
    let at = locate(&lines, section, key)?;
    let Some(i) = at.key_line else {
        return Ok(text.to_owned());
    };
    Ok(lines
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .map(|(_, l)| format!("{}{}", l.body, l.end))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UE: &str = "[/Script/FortniteGame.FortGameUserSettings]\r\nResolutionSizeX=1920\r\nbUseVSync=True\r\n\r\n[ScalabilityGroups]\r\nsg.ResolutionQuality=100\r\n+Paths=A\r\n+Paths=B\r\n";

    #[test]
    fn reads_values_case_insensitively_and_keeps_them_raw() {
        assert_eq!(
            get(UE, "/script/fortnitegame.fortgameusersettings", "busevsync")
                .unwrap()
                .as_deref(),
            Some("True")
        );
        assert_eq!(
            get(UE, "ScalabilityGroups", "sg.ResolutionQuality").unwrap().as_deref(),
            Some("100")
        );
        assert_eq!(get(UE, "ScalabilityGroups", "Missing").unwrap(), None);
        assert_eq!(get(UE, "NoSuchSection", "bUseVSync").unwrap(), None);
        assert_eq!(
            get("[A]\nk= spaced value \n", "A", "k").unwrap().as_deref(),
            Some(" spaced value ")
        );
    }

    #[test]
    fn set_rewrites_only_the_one_line() {
        let out = set(UE, "/Script/FortniteGame.FortGameUserSettings", "bUseVSync", "False").unwrap();
        assert_eq!(out, UE.replace("bUseVSync=True", "bUseVSync=False"));
        // The key keeps its spelling and indentation.
        assert_eq!(set("[A]\n  Key = 1\n", "a", "key", "2").unwrap(), "[A]\n  Key =2\n");
    }

    #[test]
    fn set_adds_a_missing_key_to_the_end_of_its_section_or_a_new_section() {
        let out = set(UE, "/Script/FortniteGame.FortGameUserSettings", "FrameRateLimit", "0").unwrap();
        assert_eq!(
            out,
            UE.replace("bUseVSync=True\r\n", "bUseVSync=True\r\nFrameRateLimit=0\r\n"),
            "after the section's last line, before the blank line"
        );
        assert_eq!(
            set("[A]\nx=1", "A", "y", "2").unwrap(),
            "[A]\nx=1\ny=2\n",
            "no newline at the end"
        );
        assert_eq!(
            set("[A]\r\nx=1\r\n", "B", "y", "2").unwrap(),
            "[A]\r\nx=1\r\n\r\n[B]\r\ny=2\r\n"
        );
        assert_eq!(set("", "B", "y", "2").unwrap(), "[B]\r\ny=2\r\n");
        assert_eq!(set("[A]\n", "A", "y", "2").unwrap(), "[A]\ny=2\n", "empty section");
    }

    #[test]
    fn remove_takes_out_only_that_line() {
        let out = remove(UE, "/Script/FortniteGame.FortGameUserSettings", "ResolutionSizeX").unwrap();
        assert_eq!(out, UE.replace("ResolutionSizeX=1920\r\n", ""));
        assert_eq!(remove(UE, "A", "missing").unwrap(), UE);
        // set then remove of a new key gives back the original file.
        let added = set(UE, "ScalabilityGroups", "sg.ShadowQuality", "0").unwrap();
        assert_eq!(remove(&added, "ScalabilityGroups", "sg.ShadowQuality").unwrap(), UE);
    }

    #[test]
    fn ambiguous_keys_and_unsafe_values_are_refused() {
        assert!(get(UE, "ScalabilityGroups", "Paths").is_err(), "array entry");
        assert!(set(UE, "ScalabilityGroups", "Paths", "C").is_err());
        assert!(get("[A]\nk=1\nK=2\n", "A", "k").is_err(), "repeated");
        assert!(
            set("[A]\n", "A", "k", "1\n[Evil]").is_err(),
            "a value may not add lines"
        );
        assert!(set("[A]\n", "A", "k=x", "1").is_err());
        assert!(set("[A]\n", "A", " ", "1").is_err());
    }

    #[test]
    fn encodings_round_trip_byte_for_byte() {
        for bytes in [
            UE.as_bytes().to_vec(),
            [&[0xEF, 0xBB, 0xBF][..], UE.as_bytes()].concat(),
            [vec![0xFF, 0xFE], UE.encode_utf16().flat_map(u16::to_le_bytes).collect()].concat(),
        ] {
            let ini = decode(&bytes).unwrap();
            assert_eq!(encode(&ini), bytes);
        }
        assert!(decode(&[0xFF, 0xFE, 0x41]).is_err(), "odd UTF-16");
        assert!(decode(&[0xC3, 0x28]).is_err(), "not UTF-8");
    }

    #[test]
    fn comments_and_lines_that_only_look_like_keys_are_left_alone() {
        let text = "; [A]\n[A]\n; k=commented\n# k=hash\nk=1\n";
        assert_eq!(get(text, "A", "k").unwrap().as_deref(), Some("1"));
        assert_eq!(
            set(text, "A", "k", "2").unwrap(),
            "; [A]\n[A]\n; k=commented\n# k=hash\nk=2\n"
        );
    }

    mod props {
        use proptest::prelude::*;

        use super::super::*;

        /// Plain INI lines: sections, keys, comments and blank lines, with
        /// either newline style.
        fn line() -> impl Strategy<Value = String> {
            prop_oneof![
                "[A-Za-z]{1,6}".prop_map(|s| format!("[{s}]")),
                ("[A-Za-z]{1,6}", "[ -~&&[^=\\[\\]]]{0,8}").prop_map(|(k, v)| format!("{k}={v}")),
                "; [ -~]{0,10}".prop_map(String::from),
                Just(String::new()),
            ]
        }

        fn file() -> impl Strategy<Value = String> {
            (prop::collection::vec(line(), 0..12), any::<bool>()).prop_map(|(lines, crlf)| {
                lines
                    .iter()
                    .map(|l| format!("{l}{}", if crlf { "\r\n" } else { "\n" }))
                    .collect()
            })
        }

        proptest! {
            #[test]
            fn adding_then_removing_a_new_key_gives_back_the_same_bytes(text in file()) {
                // A key no generated line can contain.
                let (section, key) = ("Zz9", "PeakTweaksTestKey");
                let added = set(&text, section, key, "1").unwrap();
                let got = get(&added, section, key).unwrap();
                prop_assert_eq!(got.as_deref(), Some("1"));
                let back = remove(&added, section, key).unwrap();
                // A section created for the key stays (empty), so compare the
                // file with that section header and its separator dropped.
                let header_crlf = format!("[{section}]\r\n");
                let header_lf = format!("[{section}]\n");
                let cleaned = back.replace(&header_crlf, "").replace(&header_lf, "");
                prop_assert!(cleaned.starts_with(&text), "{:?} -> {:?}", text, back);
            }

            #[test]
            fn setting_an_existing_key_changes_nothing_else(text in file(), value in "[a-z0-9]{0,6}") {
                let lines: Vec<&str> = text.lines().collect();
                // Pick the first key line that is unambiguous, if any.
                let mut section = None;
                for l in &lines {
                    if let Some(name) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                        section = Some(name.to_owned());
                    } else if let (Some(sec), Some((k, _))) = (&section, l.split_once('=')) {
                        if let Ok(Some(_)) = get(&text, sec, k) {
                            let out = set(&text, sec, k, &value).unwrap();
                            prop_assert_eq!(get(&out, sec, k).unwrap(), Some(value.clone()));
                            prop_assert_eq!(out.lines().count(), text.lines().count());
                            let changed = out.lines().zip(text.lines()).filter(|(a, b)| a != b).count();
                            prop_assert!(changed <= 1);
                            break;
                        }
                    }
                }
            }
        }
    }
}
