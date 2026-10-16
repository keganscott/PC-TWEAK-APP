//! Editing one `"key" "value"` line of a Valve KeyValues text file (game
//! settings, plan section 8): Apex Legends' `videoconfig.txt` and Counter-
//! Strike 2's `cs2_video.txt` keep their video settings this way, one flat
//! block inside a named outer one:
//!
//! ```text
//! "VideoConfig"
//! {
//!     "setting.mat_vsync"     "0"
//! }
//! ```
//!
//! Only keys directly inside the outer block are read or changed, and only the
//! characters between the value's quotes are replaced, so every other byte
//! (comments, order, tabs, line endings, the encoding) stays as it was. A key
//! that appears more than once at that level, an unquoted value, a value that
//! would need escaping, and a file that does not parse are refused rather
//! than guessed at. Keys match case-insensitively, as the KeyValues reader
//! does.

use super::error::{EngineError, Result};

fn bad(detail: impl Into<String>) -> EngineError {
    EngineError::Internal { detail: detail.into() }
}

#[derive(Debug, PartialEq, Eq)]
enum Tok {
    /// A quoted string: its contents' byte range (between the quotes).
    Quoted {
        start: usize,
        end: usize,
    },
    /// An unquoted word: its byte range.
    Bare {
        start: usize,
        end: usize,
    },
    Open,
    Close,
}

/// Each token with the byte offset where it starts.
fn tokens(text: &str) -> Result<Vec<(usize, Tok)>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                out.push((i, Tok::Open));
                i += 1;
            }
            b'}' => {
                out.push((i, Tok::Close));
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                loop {
                    match b.get(j) {
                        None => return Err(bad("a quoted string in the settings file never ends")),
                        Some(b'\\') => j += 2,
                        Some(b'"') => break,
                        Some(_) => j += 1,
                    }
                }
                out.push((i, Tok::Quoted { start, end: j }));
                i = j + 1;
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}' | b'"') {
                    i += 1;
                }
                out.push((start, Tok::Bare { start, end: i }));
            }
        }
    }
    Ok(out)
}

/// A key directly inside the outer block, and where its value is.
struct Entry {
    key: (usize, usize),
    value: Option<(usize, usize)>,
}

/// The outer block's entries, and the offset of its closing brace.
fn outer(text: &str) -> Result<(Vec<Entry>, usize)> {
    let toks = tokens(text)?;
    let mut it = toks.into_iter().peekable();
    match (it.next(), it.next()) {
        (Some((_, Tok::Quoted { .. } | Tok::Bare { .. })), Some((_, Tok::Open))) => {}
        _ => return Err(bad("the settings file does not start with a named block")),
    }
    let mut entries = Vec::new();
    loop {
        let Some((at, tok)) = it.next() else {
            return Err(bad("the settings file's block never closes"));
        };
        let key = match tok {
            Tok::Close => {
                if it.next().is_some() {
                    return Err(bad("the settings file has more after its block"));
                }
                return Ok((entries, at));
            }
            Tok::Quoted { start, end } | Tok::Bare { start, end } => (start, end),
            Tok::Open => return Err(bad("the settings file has a block without a name")),
        };
        match it.next() {
            Some((_, Tok::Quoted { start, end })) => entries.push(Entry {
                key,
                value: Some((start, end)),
            }),
            Some((_, Tok::Bare { .. })) => entries.push(Entry { key, value: None }),
            Some((_, Tok::Open)) => {
                // A nested block: skipped, its keys are not at this level.
                let mut depth = 1;
                while depth > 0 {
                    match it.next() {
                        Some((_, Tok::Open)) => depth += 1,
                        Some((_, Tok::Close)) => depth -= 1,
                        Some(_) => {}
                        None => return Err(bad("a block in the settings file never closes")),
                    }
                }
            }
            _ => return Err(bad("a key in the settings file has no value")),
        }
    }
}

/// The one entry for `key` at the outer level, if there is one.
fn find<'a>(text: &str, entries: &'a [Entry], key: &str) -> Result<Option<&'a Entry>> {
    let mut found = None;
    for e in entries {
        if text[e.key.0..e.key.1].eq_ignore_ascii_case(key) {
            if found.is_some() {
                return Err(bad(format!("\"{key}\" appears more than once; not edited")));
            }
            found = Some(e);
        }
    }
    Ok(found)
}

/// The value of `key` in the outer block, `None` when it is not set.
pub fn get(text: &str, key: &str) -> Result<Option<String>> {
    let (entries, _) = outer(text)?;
    match find(text, &entries, key)? {
        None => Ok(None),
        Some(Entry {
            value: Some((s, e)), ..
        }) => Ok(Some(text[*s..*e].to_owned())),
        Some(Entry { value: None, .. }) => Err(bad(format!("\"{key}\" has an unquoted value; not edited"))),
    }
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

/// Set `key` to `value` in the outer block: only the characters between the
/// existing value's quotes change, or a line is added before the block's
/// closing brace.
pub fn set(text: &str, key: &str, value: &str) -> Result<String> {
    let unsafe_chars = ['"', '\\', '\r', '\n', '{', '}'];
    if key.trim().is_empty() || key.contains(unsafe_chars) || value.contains(unsafe_chars) {
        return Err(bad(format!("refusing to write {key:?} {value:?}")));
    }
    let (entries, close) = outer(text)?;
    match find(text, &entries, key)? {
        Some(Entry {
            value: Some((s, e)), ..
        }) => Ok(format!("{}{value}{}", &text[..*s], &text[*e..])),
        Some(Entry { value: None, .. }) => Err(bad(format!("\"{key}\" has an unquoted value; not edited"))),
        None => {
            // Start the new line where the closing brace's line starts, so
            // the brace keeps its own indentation.
            let line_start = text[..close].rfind('\n').map_or(0, |i| i + 1);
            let indent_only = text[line_start..close].chars().all(|c| c == ' ' || c == '\t');
            let nl = newline(text);
            let line = format!("\t\"{key}\"\t\t\"{value}\"{nl}");
            Ok(if indent_only {
                format!("{}{line}{}", &text[..line_start], &text[line_start..])
            } else {
                format!("{}{nl}{line}{}", &text[..close], &text[close..])
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APEX: &str = "\"VideoConfig\"\r\n{\r\n\t\"setting.cl_gib_allow\"\t\t\"0\"\r\n\t// a note\r\n\t\"setting.mat_vsync_mode\"\t\t\"1\"\r\n\t\"setting.defaultres\"\t\t\"1920\"\r\n}\r\n";

    #[test]
    fn reads_a_value_at_the_outer_level_case_insensitively() {
        assert_eq!(get(APEX, "setting.MAT_VSYNC_MODE").unwrap().as_deref(), Some("1"));
        assert_eq!(get(APEX, "setting.missing").unwrap(), None);
    }

    #[test]
    fn changing_a_value_keeps_every_other_byte() {
        let out = set(APEX, "setting.mat_vsync_mode", "0").unwrap();
        assert_eq!(
            out,
            APEX.replace(
                "\"setting.mat_vsync_mode\"\t\t\"1\"",
                "\"setting.mat_vsync_mode\"\t\t\"0\""
            )
        );
        assert_eq!(set(&out, "setting.mat_vsync_mode", "1").unwrap(), APEX);
    }

    #[test]
    fn a_missing_key_is_added_before_the_closing_brace_with_the_files_newlines() {
        let out = set(APEX, "setting.new", "2").unwrap();
        assert!(out.ends_with("\t\"setting.defaultres\"\t\t\"1920\"\r\n\t\"setting.new\"\t\t\"2\"\r\n}\r\n"));
        assert_eq!(get(&out, "setting.new").unwrap().as_deref(), Some("2"));

        let lf = "\"a\"\n{\n\t\"k\"\t\"v\"\n}";
        assert_eq!(
            set(lf, "n", "1").unwrap(),
            "\"a\"\n{\n\t\"k\"\t\"v\"\n\t\"n\"\t\t\"1\"\n}"
        );
        let one_line = "\"a\" { \"k\" \"v\" }";
        assert_eq!(
            get(&set(one_line, "n", "1").unwrap(), "n").unwrap().as_deref(),
            Some("1")
        );
    }

    #[test]
    fn keys_in_a_nested_block_are_not_the_outer_ones() {
        let text = "\"a\"\n{\n\t\"inner\"\n\t{\n\t\t\"k\"\t\"1\"\n\t}\n\t\"k\"\t\"2\"\n}\n";
        assert_eq!(get(text, "k").unwrap().as_deref(), Some("2"));
        let out = set(text, "k", "3").unwrap();
        assert!(out.contains("\t\t\"k\"\t\"1\"") && out.contains("\t\"k\"\t\"3\""));
    }

    #[test]
    fn ambiguous_or_broken_files_are_refused_and_left_alone() {
        let twice = "\"a\"\n{\n\t\"k\"\t\"1\"\n\t\"K\"\t\"2\"\n}\n";
        assert!(get(twice, "k").is_err());
        assert!(set(twice, "k", "0").is_err());
        assert!(get("\"a\"\n{\n\t\"k\"\t1\n}\n", "k").is_err());
        assert!(set("\"a\"\n{\n\t\"k\"\t1\n}\n", "k", "0").is_err());
        for broken in [
            "",
            "\"a\"",
            "\"a\" {",
            "\"a\" { \"k\" }",
            "\"a\" { \"k\" \"v }",
            "{ }",
            "\"a\" { } \"b\" { }",
        ] {
            assert!(get(broken, "k").is_err(), "{broken:?}");
        }
        assert!(set(APEX, "k", "a\"b").is_err());
        assert!(set(APEX, "k", "a\nb").is_err());
        assert!(set(APEX, "", "1").is_err());
    }

    #[test]
    fn escaped_quotes_and_comments_with_braces_do_not_confuse_the_reader() {
        let text = "\"a\"\n{\n\t// } not a brace \"\n\t\"x\"\t\"say \\\"hi\\\"\"\n\t\"k\"\t\"1\"\n}\n";
        assert_eq!(get(text, "k").unwrap().as_deref(), Some("1"));
        assert_eq!(get(text, "x").unwrap().as_deref(), Some("say \\\"hi\\\""));
        assert_eq!(
            set(text, "k", "0").unwrap(),
            text.replace("\"k\"\t\"1\"", "\"k\"\t\"0\"")
        );
    }

    proptest::proptest! {
        #[test]
        fn setting_a_value_then_setting_it_back_restores_the_file(v in "[0-9a-z.]{0,8}") {
            let out = set(APEX, "setting.defaultres", &v).unwrap();
            proptest::prop_assert_eq!(get(&out, "setting.defaultres").unwrap(), Some(v));
            proptest::prop_assert_eq!(set(&out, "setting.defaultres", "1920").unwrap(), APEX);
        }
    }
}
