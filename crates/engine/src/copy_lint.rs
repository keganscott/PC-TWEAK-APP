//! Test support: one definition of "copy that promises a result", shared with
//! the UI copy lint (`scripts/check-ui-copy.mjs`) through
//! `scripts/claim-words.json`, so the engine and the UI cannot drift apart.

const CLAIM_WORDS_JSON: &str = include_str!("../../../scripts/claim-words.json");

pub fn claim_words() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(CLAIM_WORDS_JSON).expect("scripts/claim-words.json is valid JSON");
    v["words"]
        .as_array()
        .expect("claim-words.json has a words array")
        .iter()
        .map(|w| w.as_str().expect("every claim word is a string").to_ascii_lowercase())
        .collect()
}

/// Names of Windows and driver features that contain a claim word (lower
/// case).
fn feature_names() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(CLAIM_WORDS_JSON).expect("scripts/claim-words.json is valid JSON");
    v["names"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_str())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default()
}

/// The first claim word in `text`, if any. An entry that starts with a letter
/// or digit only matches at the start of a word ("lag" in "laggy", not "flag").
/// The feature names in the shared list are removed first.
pub fn find_claim(text: &str, words: &[String]) -> Option<String> {
    let mut lower = text.to_ascii_lowercase();
    for name in feature_names() {
        lower = lower.replace(&name, " ");
    }
    words.iter().find(|w| matches_at_word_start(&lower, w)).cloned()
}

fn matches_at_word_start(haystack: &str, word: &str) -> bool {
    let word_like = word.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    haystack.match_indices(word).any(|(i, _)| {
        !word_like
            || haystack[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// Double-quoted string literals on lines that are not `//` comments. Good
/// enough for this crate's copy, which never puts quotes inside a literal.
pub fn string_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            out.push(after[..end].to_owned());
            rest = &after[end + 1..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_list_loads_and_is_lower_case() {
        let w = claim_words();
        assert!(w.len() >= 15);
        assert!(w.iter().all(|x| *x == x.to_ascii_lowercase() && !x.is_empty()));
    }

    #[test]
    fn words_match_at_word_starts_and_symbols_anywhere() {
        let w = claim_words();
        assert_eq!(find_claim("No more lag", &w).as_deref(), Some("lag"));
        assert_eq!(find_claim("Laggy games", &w).as_deref(), Some("lag"));
        assert_eq!(find_claim("Feature flag set", &w), None);
        assert_eq!(find_claim("This improves things", &w).as_deref(), Some("improv"));
        assert_eq!(find_claim("Up to 30% more", &w).as_deref(), Some("% "));
        assert_eq!(find_claim("Higher FPS", &w).as_deref(), Some("fps"));
        assert_eq!(find_claim("Turns off pointer acceleration", &w), None);
    }

    /// A Windows or driver feature's own name says nothing about a result;
    /// the same word anywhere else in the text is still caught.
    #[test]
    fn feature_names_are_not_claims() {
        let w = claim_words();
        let path = r"\Microsoft\Windows\Customer Experience Improvement Program\Consolidator";
        assert_eq!(find_claim(path, &w), None);
        assert_eq!(
            find_claim("The Customer Experience Improvement Program improves games", &w).as_deref(),
            Some("improv")
        );
        assert_eq!(
            find_claim("Radeon Anti-Lag: on (Anti-Lag, not Anti-Lag Next)", &w),
            None
        );
        assert_eq!(find_claim("Radeon Anti-Lag cuts lag", &w).as_deref(), Some("lag"));
    }

    #[test]
    fn literals_are_taken_from_code_lines_only() {
        let src = "// \"boost\" in a comment\nlet a = \"one\"; let b = \"two\";";
        assert_eq!(string_literals(src), vec!["one".to_owned(), "two".to_owned()]);
    }
}
