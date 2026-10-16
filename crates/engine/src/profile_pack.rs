//! Profile packs (plan section 8): game settings tools delivered as signed
//! data, so a new game's lines can ship without an app release. A pack is a
//! JSON file in a signed envelope (`signed.rs`, purpose `profile-pack`); each
//! setting in it becomes a `GameIniSetting` exactly like the built-in ones
//! (`tweaks/game_ini.rs`): the same held-folder file writer, whole-file
//! backup and journal record before the change, the same anti-cheat gate.
//!
//! The signature says who wrote the pack; it is not trusted for anything
//! else. Every pack is checked again here and refused whole on the first
//! problem: the file must be a settings file under the user's own profile
//! folder (`ALLOWED_ROOTS`), keys and values must be plain one-line text the
//! file writers accept, every setting cites a public https source, no text
//! may promise a result (`copy_lint`, the same list the UI lint uses), and no
//! id may shadow a built-in tool or another pack's.
//!
//! Packs are read from `ProgramData\PeakTweaks\packs`, which only
//! administrators can write (`secure_dir`). This build trusts no key yet
//! (`PACK_KEYS` is empty, NOTES N134), so no pack loads outside tests. Not
//! built: the signed updater that fetches packs, and "unverified for this
//! version", which needs the installed game's build number (N134).

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::signed::{self, TrustedKey};
use crate::tweaks::game_ini::{FileFormat, GameIniSetting, IniEdit};
use crate::types::SafetyTier;

pub const PURPOSE: &str = "profile-pack";

/// Public keys whose packs this build loads. Empty until a production key is
/// made and its private half kept off this public repository (NOTES N134).
pub const PACK_KEYS: &[TrustedKey] = &[];

/// Settings files may only be under these folders of the user's profile.
const ALLOWED_ROOTS: &[&str] = &[
    r"<profile>\AppData\Local\",
    r"<profile>\AppData\Roaming\",
    r"<profile>\Saved Games\",
    r"<profile>\Documents\My Games\",
];
const ALLOWED_EXTENSIONS: &[&str] = &["ini", "txt", "cfg"];
const MAX_PACK_BYTES: u64 = 256 * 1024;
const MAX_PROFILES: usize = 32;
const MAX_SETTINGS: usize = 32;
const MAX_EDITS: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackFile {
    schema: u32,
    name: String,
    profiles: Vec<ProfileFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    game_id: String,
    stamp: Stamp,
    settings: Vec<SettingFile>,
}

/// The verified stamp (plan section 8 gate): which game build, when, on
/// which class of PC, and the proof run behind it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    game_build: String,
    verified_on: String,
    rig_class: String,
    proof_run_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingFile {
    id: String,
    name: String,
    summary: String,
    file: String,
    format: Format,
    edits: Vec<EditFile>,
    safety: Safety,
    tradeoff: Option<String>,
    source: String,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Format {
    Ini,
    KeyValues,
}

/// Packs may not ship anything riskier than Moderate.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Safety {
    Safe,
    Moderate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFile {
    #[serde(default)]
    section: String,
    key: String,
    value: String,
}

/// What loading the packs folder gave: the tools, and each pack refused with
/// why.
#[derive(Default)]
pub struct Loaded {
    pub tools: Vec<GameIniSetting>,
    pub refused: Vec<String>,
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn plain(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.trim().is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(format!("{what} {s:?} is empty, too long or has control characters"));
    }
    Ok(())
}

fn check_file(file: &str) -> Result<(), String> {
    let bad = || format!("file {file:?} is not a settings file under the user's own folders");
    let rest = ALLOWED_ROOTS
        .iter()
        .find_map(|root| file.strip_prefix(root))
        .ok_or_else(bad)?;
    let parts: Vec<&str> = rest.split('\\').collect();
    let names_ok = parts.iter().all(|p| {
        !p.is_empty()
            && *p != "."
            && *p != ".."
            && !p.ends_with('.')
            && !p.ends_with(' ')
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '(' | ')'))
    });
    let ext_ok = parts
        .last()
        .and_then(|n| n.rsplit_once('.'))
        .is_some_and(|(_, ext)| ALLOWED_EXTENSIONS.iter().any(|a| ext.eq_ignore_ascii_case(a)));
    (names_ok && ext_ok).then_some(()).ok_or_else(bad)
}

fn check_edit(format: Format, e: &EditFile) -> Result<(), String> {
    let forbidden = ['"', '\\', '{', '}', '[', ']', '=', ';'];
    let line_safe = |what: &str, s: &str, allow_empty: bool| {
        if (s.is_empty() && !allow_empty)
            || s.len() > 200
            || s.chars().any(|c| c.is_control() || forbidden.contains(&c))
        {
            Err(format!("{what} {s:?} cannot be written safely"))
        } else {
            Ok(())
        }
    };
    line_safe("key", &e.key, false)?;
    line_safe("value", &e.value, true)?;
    match format {
        Format::Ini if e.section.is_empty() || e.section.len() > 200 || e.section.contains(['[', ']', '\r', '\n']) => {
            Err(format!("an INI line for {:?} needs a section", e.key))
        }
        Format::KeyValues if !e.section.is_empty() => Err(format!("a KeyValues line for {:?} has a section", e.key)),
        _ => Ok(()),
    }
}

/// The tools in one pack's payload (already verified), or why it is refused.
fn parse(payload: &[u8], taken: &HashSet<String>, claim_words: &[String]) -> Result<Vec<GameIniSetting>, String> {
    let pack: PackFile = serde_json::from_slice(payload).map_err(|e| format!("not a profile pack ({e})"))?;
    if pack.schema != 1 {
        return Err(format!("pack schema {} is newer than this build reads", pack.schema));
    }
    plain("pack name", &pack.name, 100)?;
    if pack.profiles.is_empty() || pack.profiles.len() > MAX_PROFILES {
        return Err("a pack holds 1 to 32 game profiles".into());
    }
    let no_claim = |what: &str, s: &str| match crate::copy_lint::find_claim(s, claim_words) {
        Some(w) => Err(format!("{what} promises a result ({w:?})")),
        None => Ok(()),
    };
    let mut ids = HashSet::new();
    let mut out = Vec::new();
    for p in pack.profiles {
        if !crate::env::ALL_GAMES.iter().any(|g| g.id == p.game_id) {
            return Err(format!("unknown game {:?}", p.game_id));
        }
        plain("game build", &p.stamp.game_build, 64)?;
        plain("rig class", &p.stamp.rig_class, 64)?;
        let date = &p.stamp.verified_on;
        let date_ok = date.len() == 10
            && date
                .char_indices()
                .all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
        if !date_ok {
            return Err(format!("verified_on {date:?} is not YYYY-MM-DD"));
        }
        if let Some(run) = &p.stamp.proof_run_id {
            plain("proof run id", run, 64)?;
        }
        if p.settings.is_empty() || p.settings.len() > MAX_SETTINGS {
            return Err(format!("{} has no settings or more than 32", p.game_id));
        }
        for s in p.settings {
            let prefix = format!("game.{}.", p.game_id);
            let suffix_ok = s.id.strip_prefix(&prefix).is_some_and(|rest| {
                !rest.is_empty()
                    && rest.len() <= 40
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
            });
            if !suffix_ok {
                return Err(format!("tool id {:?} is not {prefix}<lower-case name>", s.id));
            }
            if taken.contains(&s.id) || !ids.insert(s.id.clone()) {
                return Err(format!("tool id {:?} is already in use", s.id));
            }
            plain("name", &s.name, 80)?;
            plain("summary", &s.summary, 600)?;
            no_claim("name", &s.name)?;
            no_claim("summary", &s.summary)?;
            if let Some(t) = &s.tradeoff {
                plain("trade-off", t, 300)?;
                no_claim("trade-off", t)?;
            }
            if !s.source.starts_with("https://") || s.source.len() > 300 || s.source.contains(char::is_whitespace) {
                return Err(format!("{} has no https source", s.id));
            }
            check_file(&s.file)?;
            if s.edits.is_empty() || s.edits.len() > MAX_EDITS {
                return Err(format!("{} sets no lines or more than 16", s.id));
            }
            for e in &s.edits {
                check_edit(s.format, e)?;
            }
            let stamp = &p.stamp;
            let summary = format!(
                "{} Checked by PeakTweaks for game build {} on {} ({}); source: {}.",
                s.summary.trim_end(),
                stamp.game_build,
                stamp.verified_on,
                stamp.rig_class,
                s.source
            );
            let edits: Vec<IniEdit> = s
                .edits
                .into_iter()
                .map(|e| IniEdit {
                    section: leak(e.section),
                    key: leak(e.key),
                    value: leak(e.value),
                })
                .collect();
            out.push(GameIniSetting {
                id: leak(s.id),
                game_id: crate::env::ALL_GAMES
                    .iter()
                    .find(|g| g.id == p.game_id)
                    .map(|g| g.id)
                    .expect("checked above"),
                name: leak(s.name),
                summary: leak(summary),
                file: leak(s.file),
                format: match s.format {
                    Format::Ini => FileFormat::Ini,
                    Format::KeyValues => FileFormat::KeyValues,
                },
                edits: Box::leak(edits.into_boxed_slice()),
                safety: match s.safety {
                    Safety::Safe => SafetyTier::Safe,
                    Safety::Moderate => SafetyTier::Moderate,
                },
                tradeoff: s.tradeoff.map(leak),
                cleared: false,
            });
        }
    }
    Ok(out)
}

/// Every `*.ptpack` file in `dir`, in name order. `taken` is the ids already
/// in the catalogue. A missing folder is no packs.
pub fn load_dir(dir: &Path, keys: &[TrustedKey], taken: &HashSet<String>) -> Loaded {
    let mut loaded = Loaded::default();
    if keys.is_empty() {
        return loaded;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return loaded;
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ptpack")))
        .collect();
    files.sort();
    let words = crate::copy_lint::claim_words();
    let mut taken = taken.clone();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = (|| {
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if !meta.is_file() || meta.len() > MAX_PACK_BYTES {
                return Err("not a plain file of at most 256 KB".to_owned());
            }
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let payload = signed::verify(&text, PURPOSE, keys).map_err(|r| r.0)?;
            parse(&payload, &taken, &words)
        })();
        match result {
            Ok(tools) => {
                taken.extend(tools.iter().map(|t| t.id.to_owned()));
                loaded.tools.extend(tools);
            }
            Err(why) => loaded.refused.push(format!("{name}: {why}")),
        }
    }
    loaded
}

/// The tools of the packs in `dir` for offered games (`env::KNOWN_GAMES`),
/// added after `catalogue`, whose ids they may not take.
pub fn offered(dir: &Path, catalogue: &[Box<dyn crate::types::Tweak>]) -> Loaded {
    let taken: HashSet<String> = catalogue.iter().map(|t| t.id().to_owned()).collect();
    let mut loaded = load_dir(dir, PACK_KEYS, &taken);
    loaded.tools.retain(|t| crate::env::is_offered(t.game_id));
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed::tests::{test_keys, TEST_SEED};
    use crate::testutil::{set_profile_dir, Harness};
    use crate::types::{Tweak, TweakState};

    fn pack(edit: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "schema": 1,
            "name": "Test pack",
            "profiles": [{
                "game_id": "fortnite",
                "stamp": {"game_build": "++Fortnite+Release-38.10", "verified_on": "2026-10-16",
                          "rig_class": "desktop", "proof_run_id": null},
                "settings": [{
                    "id": "game.fortnite.motion_blur",
                    "name": "Fortnite: Motion blur off",
                    "summary": "Turns Fortnite's own Motion Blur option off.",
                    "file": r"<profile>\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini",
                    "format": "ini",
                    "edits": [edit],
                    "safety": "safe",
                    "tradeoff": null,
                    "source": "https://www.epicgames.com/help/en-US/c-Category_Fortnite"
                }]
            }]
        })
    }

    fn good() -> serde_json::Value {
        pack(
            serde_json::json!({"section": "/Script/FortniteGame.FortGameUserSettings",
                                "key": "bMotionBlur", "value": "False"}),
        )
    }

    fn write(dir: &Path, name: &str, payload: &serde_json::Value) {
        let text = signed::sign(&TEST_SEED, "test", PURPOSE, payload.to_string().as_bytes());
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn load(dir: &Path) -> Loaded {
        load_dir(dir, &test_keys(), &HashSet::from(["game.fortnite.vsync".to_owned()]))
    }

    #[test]
    fn a_signed_pack_becomes_a_game_settings_tool_with_its_stamp_and_source() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ptpack", &good());
        let loaded = load(dir.path());
        assert!(loaded.refused.is_empty(), "{:?}", loaded.refused);
        let [tool] = loaded.tools.as_slice() else {
            panic!("one tool")
        };
        assert_eq!(tool.id(), "game.fortnite.motion_blur");
        let m = tool.metadata();
        assert!(m
            .summary
            .contains("game build ++Fortnite+Release-38.10 on 2026-10-16 (desktop)"));
        assert!(m.summary.contains("source: https://www.epicgames.com/"));
        assert_eq!(m.safety, SafetyTier::Safe);
    }

    #[test]
    fn no_key_means_no_packs_are_read() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ptpack", &good());
        let loaded = load_dir(dir.path(), PACK_KEYS, &HashSet::new());
        assert!(loaded.tools.is_empty() && loaded.refused.is_empty());
    }

    #[test]
    fn unsigned_or_unsafe_packs_are_refused_whole() {
        let dir = tempfile::tempdir().unwrap();
        // Unsigned, and signed by a key the build does not trust.
        std::fs::write(dir.path().join("1.ptpack"), good().to_string()).unwrap();
        std::fs::write(
            dir.path().join("2.ptpack"),
            signed::sign(&[9; 32], "test", PURPOSE, good().to_string().as_bytes()),
        )
        .unwrap();
        let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
        let set = |path: &[&str], v: serde_json::Value| {
            let mut p = good();
            let mut at = &mut p["profiles"][0];
            for (i, k) in path.iter().enumerate() {
                if i + 1 == path.len() {
                    at[*k] = v.clone();
                } else {
                    at = if *k == "s" { &mut at["settings"][0] } else { &mut at[*k] };
                }
            }
            p
        };
        cases.push((
            "3",
            set(&["s", "file"], r"C:\Windows\System32\drivers\etc\hosts".into()),
        ));
        cases.push(("4", set(&["s", "file"], r"<profile>\AppData\Local\..\..\x.ini".into())));
        cases.push(("5", set(&["s", "file"], r"<profile>\AppData\Local\Game\run.exe".into())));
        cases.push(("6", set(&["s", "summary"], "Gives you more FPS.".into())));
        cases.push(("7", set(&["s", "id"], "game.fortnite.vsync".into())));
        cases.push(("8", set(&["s", "id"], "game.apex.x".into())));
        cases.push(("9", set(&["s", "source"], "http://example.com".into())));
        cases.push(("10", set(&["s", "safety"], "advanced".into())));
        cases.push(("11", set(&["game_id"], "notagame".into())));
        cases.push(("12", set(&["stamp", "verified_on"], "16/10/2026".into())));
        cases.push((
            "13",
            pack(serde_json::json!({"section": "/Script/X", "key": "a", "value": "1\r\n[Evil]"})),
        ));
        cases.push(("14", pack(serde_json::json!({"key": "a", "value": "1"}))));
        cases.push(("15", set(&["s", "unknown"], 1.into())));
        for (name, payload) in &cases {
            write(dir.path(), &format!("{name}.ptpack"), payload);
        }
        let loaded = load(dir.path());
        assert!(
            loaded.tools.is_empty(),
            "{:?}",
            loaded.tools.iter().map(|t| t.id).collect::<Vec<_>>()
        );
        assert_eq!(loaded.refused.len(), cases.len() + 2, "{:#?}", loaded.refused);
    }

    #[test]
    fn two_packs_cannot_claim_the_same_tool() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ptpack", &good());
        write(dir.path(), "b.ptpack", &good());
        let loaded = load(dir.path());
        assert_eq!(loaded.tools.len(), 1);
        assert!(
            loaded.refused[0].starts_with("b.ptpack: tool id"),
            "{:?}",
            loaded.refused
        );
    }

    #[test]
    fn a_pack_tool_changes_its_line_and_undo_puts_the_file_back() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ptpack", &good());
        let mut tool = load(dir.path()).tools.pop().unwrap();
        tool.cleared = true;
        let mut h = Harness::new(vec![Box::new(tool)]);
        set_profile_dir(&h.fake, r"C:\Users\Kegan");
        let on_disk = r"C:\Users\Kegan\AppData\Local\FortniteGame\Saved\Config\WindowsClient\GameUserSettings.ini";
        let original = "[/Script/FortniteGame.FortGameUserSettings]\r\nbMotionBlur=True\r\n";
        h.sys.set_file(on_disk, original.as_bytes());
        h.engine.apply("game.fortnite.motion_blur").unwrap();
        assert_eq!(
            h.sys.file(on_disk).unwrap(),
            original.replace("True", "False").as_bytes()
        );
        let state = |h: &Harness| {
            h.engine
                .list()
                .unwrap()
                .into_iter()
                .find(|t| t.metadata.id == "game.fortnite.motion_blur")
                .unwrap()
                .state
        };
        assert_eq!(state(&h), TweakState::Applied);
        h.engine.revert("game.fortnite.motion_blur").unwrap();
        assert_eq!(h.sys.file(on_disk).unwrap(), original.as_bytes());
    }

    #[test]
    fn a_pack_tool_stays_behind_the_anti_cheat_gate() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.ptpack", &good());
        let tool = load(dir.path()).tools.pop().unwrap();
        assert!(!tool.cleared);
        let env = crate::types::SystemEnv::default();
        assert!(matches!(
            tool.evaluate_predicate(&env),
            crate::types::PredicateOutcome::Block(_)
        ));
    }
}
