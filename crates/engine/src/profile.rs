//! Pure helpers for locating a user's profile folder from registry data.

use std::path::PathBuf;

/// Expand `%NAME%` references using `lookup`. Unknown names are left as written,
/// the way `ExpandEnvironmentStrings` does, so a bad value stays visible.
pub fn expand_percent_vars(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                // A lone or doubled '%': keep it and move on.
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where WebView2 keeps its data when the elevated identity is not the
/// interactive user: inside the interactive user's Local AppData, a place that
/// user can also clean up.
pub fn webview2_dir_in_profile(profile: &str) -> PathBuf {
    PathBuf::from(profile)
        .join("AppData")
        .join("Local")
        .join("PeakTweaks")
        .join("WebView2")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "systemdrive" => Some("C:".into()),
            "x" => Some("1".into()),
            _ => None,
        }
    }

    #[test]
    fn expands_known_variables_and_keeps_unknown_ones() {
        assert_eq!(
            expand_percent_vars(r"%SystemDrive%\Users\Kegan", env),
            r"C:\Users\Kegan"
        );
        assert_eq!(expand_percent_vars(r"%Nope%\a", env), r"%Nope%\a");
        assert_eq!(expand_percent_vars("100%", env), "100%");
        assert_eq!(expand_percent_vars("a%%b", env), "a%%b");
        assert_eq!(expand_percent_vars("%x%%x%", env), "11");
        assert_eq!(expand_percent_vars("plain", env), "plain");
    }

    #[test]
    fn webview2_dir_sits_under_local_appdata() {
        let p = webview2_dir_in_profile(r"C:\Users\Kegan");
        assert!(p.ends_with(std::path::Path::new("AppData/Local/PeakTweaks/WebView2")));
    }
}
