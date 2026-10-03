//! One behaviour contract for `RegistryBackend`, run against the in-memory fake
//! on every OS and against the real registry (`WinRegistry`) on Windows.
//!
//! Every engine test runs on the fake, so the fake is only worth trusting if it
//! behaves like Windows. This suite is the check: the same assertions, both
//! backends. On Windows it also drives the whole engine (apply, revert) against
//! a scratch key under HKCU and imports the `.reg` backups with Windows' own
//! `reg.exe`, which is the only proof that a backup really restores.
//!
//! Scratch keys are `HKCU\Software\PeakTweaksTest-<pid>-<name>`, removed when
//! the test ends, pass or fail. Nothing outside them is touched.

use super::{Hive, RegistryBackend};
use crate::error::EngineError;
use crate::types::RawValue;

const HIVE: Hive = Hive::CurrentUser;

fn utf16z(parts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend(p.encode_utf16().flat_map(u16::to_le_bytes));
        out.extend([0, 0]);
    }
    out
}

/// One value of every type the engine writes, with awkward contents.
fn samples() -> Vec<(&'static str, RawValue)> {
    let raw = |vtype, bytes| RawValue { vtype, bytes };
    vec![
        ("Sz", RawValue::sz(r#"C:\Path with "quotes" and ünïcode ✓"#)),
        (
            "SzNoTerminator",
            raw(1, "abc".encode_utf16().flat_map(u16::to_le_bytes).collect()),
        ),
        ("SzEmpty", RawValue::sz("")),
        ("ExpandSz", raw(2, utf16z(&[r"%SystemRoot%\System32"]))),
        (
            "ExpandSzNoTerminator",
            raw(2, "%TEMP%".encode_utf16().flat_map(u16::to_le_bytes).collect()),
        ),
        ("Binary", raw(3, (0..=255u8).collect())),
        ("BinaryEmpty", raw(3, vec![])),
        ("Dword", RawValue::dword(0xDEAD_BEEF)),
        ("DwordZero", RawValue::dword(0)),
        (
            "MultiSz",
            raw(7, {
                let mut b = utf16z(&["first", "", "third"]);
                b.extend([0, 0]);
                b
            }),
        ),
        ("Qword", raw(11, 0x0123_4567_89AB_CDEFu64.to_le_bytes().to_vec())),
        ("MultiSzNoTerminator", raw(7, utf16z(&["a"])[..2].to_vec())),
    ]
}

/// The contract. `base` is a key this test owns and that does not exist yet.
/// `seed_foreign` writes a value of an arbitrary type the way another program
/// would, bypassing the backend's own type check.
fn contract(reg: &dyn RegistryBackend, base: &str, seed_foreign: &dyn Fn(&str, &str, RawValue)) -> usize {
    let mut checks = 0;
    let mut check = |ok: bool, what: &str| {
        assert!(ok, "contract: {what}");
        checks += 1;
    };

    // Absent things read as absent, not as errors.
    check(!reg.key_exists(HIVE, base).unwrap(), "fresh key is absent");
    check(
        reg.read_value(HIVE, base, "Nope").unwrap().is_none(),
        "value under absent key reads None",
    );
    check(
        !reg.delete_value(HIVE, base, "Nope").unwrap(),
        "delete under absent key is false",
    );
    check(
        !reg.delete_key_if_empty(HIVE, base).unwrap(),
        "delete absent key is false",
    );
    check(
        !reg.delete_key_if_empty(HIVE, "").unwrap(),
        "a hive root is never deleted",
    );

    // Byte-exact round trips of every supported type, creating parents.
    let vals = format!(r"{base}\Values\Deeper");
    for (name, v) in samples() {
        reg.write_value(HIVE, &vals, name, &v).unwrap();
        let got = reg.read_value(HIVE, &vals, name).unwrap();
        check(
            got == Some(v.clone()),
            &format!("{name} round-trips: wrote {v:?}, read {got:?}"),
        );
    }
    check(
        reg.key_exists(HIVE, &format!(r"{base}\Values")).unwrap(),
        "write creates missing parents",
    );

    // Writes are exact every time, not by luck of what follows the buffer in
    // memory (WinRegistry::set_exact): rewrite the unterminated strings often.
    for (name, v) in samples()
        .into_iter()
        .filter(|(_, v)| matches!(v.vtype, 1 | 2 | 7) && !v.bytes.ends_with(&[0, 0]))
    {
        let exact = (0..50).all(|_| {
            reg.write_value(HIVE, &vals, name, &v).unwrap();
            reg.read_value(HIVE, &vals, name).unwrap() == Some(v.clone())
        });
        check(exact, &format!("{name} is stored exactly on every one of 50 writes"));
    }

    // Overwrite with a different type replaces type and bytes.
    reg.write_value(HIVE, &vals, "Dword", &RawValue::sz("now a string"))
        .unwrap();
    check(
        reg.read_value(HIVE, &vals, "Dword").unwrap() == Some(RawValue::sz("now a string")),
        "overwrite changes the type",
    );

    // Keys and value names are case-insensitive.
    let upper = vals.to_uppercase();
    check(
        reg.read_value(HIVE, &upper, "QWORD").unwrap()
            == samples().into_iter().find(|(n, _)| *n == "Qword").map(|(_, v)| v),
        "keys and names ignore case",
    );
    check(reg.key_exists(HIVE, &upper).unwrap(), "key_exists ignores case");

    // Types we cannot restore are refused before anything is created.
    for vtype in [0u32, 5, 6, 8, 9, 10, 12] {
        let path = format!(r"{base}\Refused{vtype}");
        let err = reg
            .write_value(
                HIVE,
                &path,
                "X",
                &RawValue {
                    vtype,
                    bytes: vec![1, 2, 3, 4],
                },
            )
            .unwrap_err();
        check(
            matches!(err, EngineError::UnsupportedValueType { vtype: t, .. } if t == vtype),
            &format!("vtype {vtype} is refused: {err:?}"),
        );
        check(
            !reg.key_exists(HIVE, &path).unwrap(),
            &format!("refused vtype {vtype} created no key"),
        );
    }

    // A foreign value of an unsupported type still reads back raw, so the
    // transaction layer can see it and refuse to touch it.
    seed_foreign(
        &vals,
        "Foreign",
        RawValue {
            vtype: 0,
            bytes: vec![9, 8, 7],
        },
    );
    check(
        reg.read_value(HIVE, &vals, "Foreign").unwrap()
            == Some(RawValue {
                vtype: 0,
                bytes: vec![9, 8, 7],
            }),
        "foreign REG_NONE reads back raw",
    );

    // delete_value reports whether something was there.
    check(
        reg.delete_value(HIVE, &vals, "foreign").unwrap(),
        "delete existing value is true",
    );
    check(
        !reg.delete_value(HIVE, &vals, "Foreign").unwrap(),
        "delete again is false",
    );
    check(
        reg.read_value(HIVE, &vals, "Foreign").unwrap().is_none(),
        "deleted value is gone",
    );

    // create_key is idempotent and creates parents.
    let made = format!(r"{base}\Made\A\B");
    reg.create_key(HIVE, &made).unwrap();
    reg.create_key(HIVE, &made).unwrap();
    check(reg.key_exists(HIVE, &made).unwrap(), "create_key creates the key");

    // delete_key_if_empty: never a key with values or subkeys.
    check(
        !reg.delete_key_if_empty(HIVE, &format!(r"{base}\Made\A")).unwrap(),
        "key with a subkey stays",
    );
    check(!reg.delete_key_if_empty(HIVE, &vals).unwrap(), "key with values stays");
    check(reg.key_exists(HIVE, &vals).unwrap(), "key with values still exists");
    check(reg.delete_key_if_empty(HIVE, &made).unwrap(), "empty leaf is deleted");
    check(!reg.key_exists(HIVE, &made).unwrap(), "deleted leaf is gone");
    check(
        reg.delete_key_if_empty(HIVE, &format!(r"{base}\made\a")).unwrap(),
        "now-empty parent is deleted, any case",
    );
    check(
        reg.delete_key_if_empty(HIVE, &format!(r"{base}\Made")).unwrap(),
        "and its parent",
    );

    // Empty out the values key and remove the whole tree bottom-up.
    for (name, _) in samples() {
        check(reg.delete_value(HIVE, &vals, name).unwrap(), &format!("{name} deletes"));
    }
    check(reg.delete_key_if_empty(HIVE, &vals).unwrap(), "emptied key is deleted");
    check(
        reg.delete_key_if_empty(HIVE, &format!(r"{base}\Values")).unwrap(),
        "values parent is deleted",
    );
    check(reg.delete_key_if_empty(HIVE, base).unwrap(), "base is deleted");
    check(!reg.key_exists(HIVE, base).unwrap(), "nothing is left");
    checks
}

#[test]
fn the_fake_registry_meets_the_contract() {
    let fake = super::fake::FakeRegistry::new();
    fake.create_key(HIVE, "Software").unwrap(); // exists on every real Windows
    let checks = contract(&fake, r"Software\PeakTweaksTest-fake", &|path, name, v| {
        fake.set_external(HIVE, path, name, v)
    });
    assert_eq!(fake.key_paths(), vec![(HIVE, "software".to_owned())]);
    println!("fake registry: {checks} contract checks passed");
}

#[cfg(windows)]
mod real {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ};
    use winreg::{RegKey, RegValue};

    use super::super::windows::WinRegistry;
    use super::*;
    use crate::context::{ContextResolver, UserContext, UserResolution};
    use crate::engine::Engine;
    use crate::env::{License, StubProbe};
    use crate::identity;
    use crate::journal::{Journal, JournalEntry};
    use crate::secure_dir::TrustedDir;
    use crate::testutil::TestTweak;
    use crate::types::{ExecutionContext, Tier, Tweak};

    /// `HKCU\Software\PeakTweaksTest-<pid>-<name>`, deleted (with everything
    /// under it) on drop, so a failing assertion still cleans up.
    struct Scratch {
        path: String,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = format!(r"Software\PeakTweaksTest-{}-{name}", std::process::id());
            let _ = hkcu().delete_subkey_all(&path);
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = hkcu().delete_subkey_all(&self.path);
        }
    }

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    /// Bypasses `WinRegistry` entirely, as another program would.
    fn seed(path: &str, name: &str, v: RawValue) {
        let vtype: RegType = match v.vtype {
            0 => RegType::REG_NONE,
            1 => RegType::REG_SZ,
            2 => RegType::REG_EXPAND_SZ,
            3 => RegType::REG_BINARY,
            4 => RegType::REG_DWORD,
            7 => RegType::REG_MULTI_SZ,
            11 => RegType::REG_QWORD,
            other => panic!("seed: add vtype {other}"),
        };
        let (key, _) = hkcu().create_subkey(path).unwrap();
        key.set_raw_value(name, &RegValue { bytes: v.bytes, vtype }).unwrap();
    }

    /// Every key and value under `path`, read with winreg directly, with
    /// names lower-cased so the comparison follows registry rules.
    fn dump(path: &str) -> BTreeMap<String, Option<(u32, Vec<u8>)>> {
        fn walk(key: &RegKey, rel: &str, out: &mut BTreeMap<String, Option<(u32, Vec<u8>)>>) {
            out.insert(format!("[{rel}]"), None);
            for v in key.enum_values() {
                let (name, val) = v.unwrap();
                out.insert(
                    format!("{rel}:{}", name.to_lowercase()),
                    Some((val.vtype as u32, val.bytes)),
                );
            }
            for k in key.enum_keys() {
                let k = k.unwrap();
                let child = key.open_subkey_with_flags(&k, KEY_READ).unwrap();
                walk(&child, &format!(r"{rel}\{}", k.to_lowercase()), out);
            }
        }
        let mut out = BTreeMap::new();
        if let Ok(root) = hkcu().open_subkey_with_flags(path, KEY_READ) {
            walk(&root, "", &mut out);
        }
        out
    }

    fn real_engine(dir: &Path, tweaks: Vec<Box<dyn Tweak>>) -> Engine {
        let user = UserContext {
            sid: identity::current_process_sid().unwrap(),
            resolution: UserResolution::OwnToken,
            is_self: true,
        };
        let resolver = ContextResolver::new(user, identity::is_elevated(), Arc::new(WinRegistry::new()));
        let journal = Journal::open(&TrustedDir::insecure_for_tests(dir)).unwrap();
        Engine::new(
            resolver,
            journal,
            tweaks,
            Box::new(StubProbe::open_for_dev()),
            License::dev(Tier::Ultimate),
        )
    }

    fn user_tweak(id: &str, key: &str, writes: &[(&str, u32)]) -> Box<dyn Tweak> {
        let mut t = TestTweak::new(id, key, writes);
        t.context = ExecutionContext::User;
        Box::new(t)
    }

    /// Evidence, not a pass/fail check: how Windows reads back string values
    /// stored without a NUL terminator. CI run 36959654373 read an unterminated
    /// REG_EXPAND_SZ back differently from what was written, on some runs only.
    /// This reads each such value many times through `WinRegistry` and through
    /// `RegQueryValueExW` directly with the spare buffer pre-filled with 0x00
    /// and with 0xAB, and prints every distinct result with its count.
    #[test]
    fn unterminated_string_read_back_report() {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{RegQueryValueExW, RegSetValueExW, HKEY, REG_VALUE_TYPE};

        let s = Scratch::new("readback");
        let utf16 = |t: &str| -> Vec<u8> { t.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        let cases = [
            ("Sz", 1u32, utf16("abc")),
            ("ExpandSz", 2, utf16("%TEMP%")),
            ("ExpandSzLonger", 2, utf16("%SystemRoot%\\x")),
            ("MultiSz", 7, utf16("a\0b")),
        ];
        let reg = WinRegistry::new();
        for (name, vtype, bytes) in &cases {
            seed(
                &s.path,
                name,
                RawValue {
                    vtype: *vtype,
                    bytes: bytes.clone(),
                },
            );
        }
        let key = hkcu().open_subkey_with_flags(&s.path, KEY_READ).unwrap();
        for (name, vtype, bytes) in &cases {
            let mut via_backend: BTreeMap<String, usize> = BTreeMap::new();
            for _ in 0..300 {
                let v = reg.read_value(HIVE, &s.path, name).unwrap().unwrap();
                *via_backend.entry(format!("{}:{:02x?}", v.vtype, v.bytes)).or_default() += 1;
            }
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let mut direct: BTreeMap<String, usize> = BTreeMap::new();
            for fill in [0x00u8, 0xAB] {
                for _ in 0..100 {
                    let mut buf = vec![fill; 256];
                    let mut len = buf.len() as u32;
                    let mut ty = REG_VALUE_TYPE(0);
                    let rc = unsafe {
                        RegQueryValueExW(
                            HKEY(key.raw_handle() as *mut _),
                            PCWSTR(wide.as_ptr()),
                            None,
                            Some(&mut ty),
                            Some(buf.as_mut_ptr()),
                            Some(&mut len),
                        )
                    };
                    let shown = &buf[..(len as usize + 4).min(buf.len())];
                    *direct
                        .entry(format!(
                            "fill {fill:02x}: rc={} len={len} type={} bytes+4={shown:02x?}",
                            rc.0, ty.0
                        ))
                        .or_default() += 1;
                }
            }
            // Write with RegSetValueExW directly, the memory right after the
            // data holding 00 00 or ab ab, and see how many bytes were stored.
            let rw = hkcu().create_subkey(&s.path).unwrap().0;
            let mut writes: BTreeMap<String, usize> = BTreeMap::new();
            for follow in [0x00u8, 0xAB] {
                for _ in 0..50 {
                    let mut buf = bytes.clone();
                    buf.extend([follow; 4]);
                    let rc = unsafe {
                        RegSetValueExW(
                            HKEY(rw.raw_handle() as *mut _),
                            PCWSTR(wide.as_ptr()),
                            0,
                            REG_VALUE_TYPE(*vtype),
                            Some(&buf[..bytes.len()]),
                        )
                    };
                    let stored = rw.get_raw_value(name).map(|v| v.bytes.len());
                    *writes
                        .entry(format!("next bytes {follow:02x}: rc={} stored {stored:?} bytes", rc.0))
                        .or_default() += 1;
                }
            }
            println!("{name} (vtype {vtype}) wrote {} bytes {bytes:02x?}", bytes.len());
            for (k, n) in &writes {
                println!("  RegSetValueExW x{n}: {k}");
            }
            for (k, n) in &via_backend {
                println!("  WinRegistry x{n}: {k}");
            }
            for (k, n) in &direct {
                println!("  RegQueryValueExW x{n}: {k}");
            }
        }
    }

    #[test]
    fn the_real_registry_meets_the_contract() {
        let s = Scratch::new("contract");
        let checks = contract(&WinRegistry::new(), &s.path, &seed);
        assert!(dump(&s.path).is_empty(), "scratch key left behind: {:?}", dump(&s.path));
        println!("real registry (HKCU\\{}): {checks} contract checks passed", s.path);
    }

    /// Apply two changes through the engine on the real registry, then revert
    /// them, and require the scratch tree to be byte-for-byte what it was:
    /// overwritten values back, added values gone, keys we created removed,
    /// a key that already existed kept with its other values.
    #[test]
    fn engine_apply_then_revert_restores_the_real_registry_exactly() {
        let s = Scratch::new("engine");
        let existing = format!(r"{}\Existing", s.path);
        let created = format!(r"{}\Made\Here", s.path);
        seed(&existing, "Prev", RawValue::dword(7));
        seed(&existing, "Other", RawValue::sz("keep me"));
        let before = dump(&s.path);

        let dir = tempfile::tempdir().unwrap();
        let mut engine = real_engine(
            dir.path(),
            vec![
                user_tweak("t.existing", &existing, &[("Prev", 1), ("New", 2)]),
                user_tweak("t.created", &created, &[("A", 3)]),
            ],
        );
        let mut entries: Vec<JournalEntry> = engine.apply("t.existing").unwrap();
        entries.extend(engine.apply("t.created").unwrap());

        let reg = WinRegistry::new();
        let read = |p: &str, n: &str| reg.read_value(HIVE, p, n).unwrap().and_then(|v| v.as_dword());
        assert_eq!(read(&existing, "Prev"), Some(1));
        assert_eq!(read(&existing, "New"), Some(2));
        assert_eq!(read(&created, "A"), Some(3));
        let made_here = entries.iter().find(|e| e.value_name == "A").unwrap();
        assert_eq!(made_here.created_keys.len(), 2, "{:?}", made_here.created_keys);
        for e in &entries {
            let bytes = std::fs::read(dir.path().join(&e.backup_file)).unwrap();
            assert_eq!(&bytes[..2], &[0xFF, 0xFE], "{} is UTF-16LE with a BOM", e.backup_file);
        }

        let results = engine.revert_all();
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert_eq!(dump(&s.path), before, "revert restored the exact prior tree");
        println!(
            "real engine: {} writes applied and reverted under HKCU\\{}; tree identical ({} entries)",
            entries.len(),
            s.path,
            before.len()
        );
    }

    /// The `.reg` backups are the recovery path when PeakTweaks itself cannot
    /// run. Import them with Windows' own `reg.exe` and require the prior
    /// values back exactly, for every type the encoder handles, including a
    /// value that did not exist (the backup must delete it).
    #[test]
    fn reg_exe_import_of_the_backups_restores_the_prior_values() {
        let s = Scratch::new("import");
        let key = format!(r"{}\Existing", s.path);
        let mut names = Vec::new();
        for (name, v) in samples() {
            seed(&key, name, v);
            names.push(name);
        }
        seed(&key, "SzControlChars", RawValue::sz("line one\nline two\ttab"));
        names.push("SzControlChars");
        seed(&key, "Untouched", RawValue::sz("not in the tweak"));
        names.push("WasAbsent");
        let before = dump(&s.path);

        let writes: Vec<(&str, u32)> = names.iter().map(|n| (*n, 1)).collect();
        let dir = tempfile::tempdir().unwrap();
        let mut engine = real_engine(dir.path(), vec![user_tweak("t.import", &key, &writes)]);
        let mut entries = engine.apply("t.import").unwrap();
        assert_eq!(entries.len(), names.len());
        assert_ne!(dump(&s.path), before, "apply changed the tree");
        drop(engine);

        // Newest first, as a person restoring by hand would.
        entries.sort_by_key(|e| std::cmp::Reverse(e.seq));
        for e in &entries {
            let file = dir.path().join(&e.backup_file);
            let out = Command::new("reg.exe").arg("import").arg(&file).output().unwrap();
            assert!(
                out.status.success(),
                "reg import {} failed: {}{}",
                file.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        // reg.exe sometimes stores a NUL after string data that had none: it
        // hits the same RegSetValueExW behaviour WinRegistry::set_exact guards
        // against (CI runs 36952824230, 36963518402, 36965446605), and we cannot
        // change reg.exe. So that one difference is accepted, only for string
        // values written without a terminator; every other byte must match.
        // In-app Undo restores through WinRegistry and is exact (tests above).
        let after = dump(&s.path);
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>(),
            "same keys and values"
        );
        let mut normalised = Vec::new();
        for (name, was) in &before {
            let now = &after[name];
            if now == was {
                continue;
            }
            let terminated = match (was, now) {
                (Some((t, b)), Some((t2, b2))) if t == t2 && matches!(*t, 1 | 2 | 7) && !b.ends_with(&[0, 0]) => {
                    *b2 == [b.as_slice(), &[0, 0]].concat()
                }
                _ => false,
            };
            assert!(terminated, "{name} not restored exactly: was {was:?}, now {now:?}");
            normalised.push(name.clone());
        }
        let values = before.values().filter(|v| v.is_some()).count();
        println!(
            "reg.exe import: {} backups (HKEY_USERS\\<own sid> form) restored {values} values under HKCU\\{}; \
             {} byte-exact, NUL terminator added by reg.exe to: {normalised:?}",
            entries.len(),
            s.path,
            values - normalised.len(),
        );
    }
    /// The combined per-change file (agent brief bug 9): importing only it
    /// with reg.exe must undo the whole change. Uses terminated strings so the
    /// reg.exe NUL behaviour (test above) does not apply.
    #[test]
    fn reg_exe_import_of_the_session_file_alone_restores_the_change() {
        let s = Scratch::new("session");
        let key = format!(r"{}\Existing", s.path);
        let mut names = Vec::new();
        for (name, v) in samples() {
            let terminated = !matches!(v.vtype, 1 | 2 | 7) || v.bytes.ends_with(&[0, 0]);
            if terminated {
                seed(&key, name, v);
                names.push(name);
            }
        }
        names.push("WasAbsent");
        let before = dump(&s.path);

        let writes: Vec<(&str, u32)> = names.iter().map(|n| (*n, 1)).collect();
        let dir = tempfile::tempdir().unwrap();
        let mut engine = real_engine(dir.path(), vec![user_tweak("t.session", &key, &writes)]);
        let entries = engine.apply("t.session").unwrap();
        drop(engine);
        assert_ne!(dump(&s.path), before);

        let day = dir.path().join(Path::new(&entries[0].backup_file).parent().unwrap());
        let session: Vec<_> = std::fs::read_dir(&day)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("session_"))
            .collect();
        assert_eq!(session.len(), 1, "{session:?}");
        let out = Command::new("reg.exe").arg("import").arg(&session[0]).output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            dump(&s.path),
            before,
            "importing the session file alone restored the exact prior tree"
        );
        println!(
            "reg.exe import of {}: {} values restored exactly under HKCU\\{}",
            session[0].file_name().unwrap().to_string_lossy(),
            entries.len(),
            s.path
        );
    }

    /// The offline undo set (offline.rs) on hive *files*, as the recovery
    /// environment would meet them: build a SYSTEM, SOFTWARE and NTUSER.DAT
    /// with `reg save`, write the set for changes in all three, run
    /// `recover.cmd` against that folder, then load the files again and
    /// require the prior values. Also: the script refuses this running
    /// Windows, and a folder with no Windows in it.
    #[test]
    fn recover_cmd_restores_prior_values_in_offline_hive_files() {
        use crate::journal::JournalAction;
        use crate::offline;

        fn reg(args: &[&str]) -> std::process::Output {
            Command::new("reg.exe").args(args).output().unwrap()
        }
        fn ok(out: &std::process::Output) -> String {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.status.success(), "{text}");
            text
        }
        /// Unloads on drop, so a failed assertion does not leave a hive loaded.
        struct Loaded(String);
        impl Drop for Loaded {
            fn drop(&mut self) {
                let _ = Command::new("reg.exe").args(["unload", &self.0]).output();
            }
        }

        let s = Scratch::new("offline");
        let root = tempfile::tempdir().unwrap();
        let w = root.path();
        let config = w.join(r"Windows\System32\config");
        let profile = w.join(r"Users\pt");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&profile).unwrap();

        // The "applied" state, saved as three hive files.
        let sys = format!(r"{}\sys", s.path);
        let soft = format!(r"{}\soft", s.path);
        let user = format!(r"{}\user", s.path);
        seed(&format!(r"{sys}\Select"), "Current", RawValue::dword(1));
        seed(&format!(r"{sys}\ControlSet001\Control\PTTest"), "V", RawValue::dword(9));
        seed(&format!(r"{soft}\PTTest"), "S", RawValue::sz("applied"));
        seed(&format!(r"{soft}\PTTest"), "Added", RawValue::dword(1));
        seed(&format!(r"{user}\Control Panel\PTTest"), "U", RawValue::sz("applied"));
        for (key, file) in [
            (&sys, config.join("SYSTEM")),
            (&soft, config.join("SOFTWARE")),
            (&user, profile.join("NTUSER.DAT")),
        ] {
            ok(&reg(&["save", &format!(r"HKCU\{key}"), file.to_str().unwrap(), "/y"]));
        }

        // What the journal would hold for changes that made that state.
        const SID: &str = "S-1-5-21-1000-2000-3000-4242";
        let write = |seq: u64, tweak: &str, path: &str, name: &str, previous: Option<RawValue>| {
            let mut e = crate::journal::tests::entry(seq, seq, tweak, JournalAction::Apply);
            e.display_path = path.into();
            e.value_name = name.into();
            e.previous = previous;
            e
        };
        let outstanding = vec![
            (
                "t.user".to_owned(),
                vec![write(
                    5,
                    "t.user",
                    &format!(r"HKEY_USERS\{SID}\Control Panel\PTTest"),
                    "U",
                    Some(RawValue::sz("before")),
                )],
            ),
            (
                "t.machine".to_owned(),
                vec![
                    write(
                        1,
                        "t.machine",
                        r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\PTTest",
                        "V",
                        Some(RawValue::dword(3)),
                    ),
                    write(
                        2,
                        "t.machine",
                        r"HKEY_LOCAL_MACHINE\SOFTWARE\PTTest",
                        "S",
                        Some(RawValue::sz("before")),
                    ),
                    write(3, "t.machine", r"HKEY_LOCAL_MACHINE\SOFTWARE\PTTest", "Added", None),
                ],
            ),
        ];
        let facts = offline::Facts {
            control_set: Some(1),
            system_drive: "C:".into(),
            profiles: [(SID.to_owned(), Some(r"C:\Users\pt".to_owned()))].into(),
        };
        let plan = offline::plan(&outstanding, &facts);
        assert!(plan.not_covered.is_empty(), "{:?}", plan.not_covered);
        let data = w.join(r"ProgramData\PeakTweaks");
        offline::refresh(&data, &plan).unwrap();
        let script = data.join(offline::DIR).join("recover.cmd");

        let run = |arg: &Path| Command::new(&script).arg(arg).output().unwrap();
        let out = run(w);
        let text = ok(&out);

        let mounts = [
            ("HKLM\\PTCHECK_SYSTEM", config.join("SYSTEM")),
            ("HKLM\\PTCHECK_SOFTWARE", config.join("SOFTWARE")),
            ("HKLM\\PTCHECK_USER", profile.join("NTUSER.DAT")),
        ];
        let mut loaded = Vec::new();
        for (name, file) in &mounts {
            ok(&reg(&["load", name, file.to_str().unwrap()]));
            loaded.push(Loaded((*name).to_owned()));
        }
        let r = WinRegistry::new();
        let get = |path: &str, name: &str| r.read_value(super::Hive::LocalMachine, path, name).unwrap();
        assert_eq!(
            get(r"PTCHECK_SYSTEM\ControlSet001\Control\PTTest", "V"),
            Some(RawValue::dword(3))
        );
        assert_eq!(
            get(r"PTCHECK_SYSTEM\Select", "Current"),
            Some(RawValue::dword(1)),
            "the rest is untouched"
        );
        assert_eq!(
            get(r"PTCHECK_SOFTWARE\PTTest", "S").and_then(|v| v.as_sz()).as_deref(),
            Some("before")
        );
        assert_eq!(
            get(r"PTCHECK_SOFTWARE\PTTest", "Added"),
            None,
            "a value the change added is deleted"
        );
        assert_eq!(
            get(r"PTCHECK_USER\Control Panel\PTTest", "U")
                .and_then(|v| v.as_sz())
                .as_deref(),
            Some("before")
        );
        drop(loaded);

        // Every hive the script loaded was unloaded again.
        for mount in ["PT_OFFLINE_SYSTEM", "PT_OFFLINE_SOFTWARE", &format!("PT_OFFLINE_{SID}")] {
            assert!(
                !r.key_exists(super::Hive::LocalMachine, mount).unwrap(),
                "{mount} left loaded"
            );
        }

        // This Windows is running: its SYSTEM hive is in use and is refused.
        let live = run(Path::new("C:"));
        assert_eq!(live.status.code(), Some(3), "{}", String::from_utf8_lossy(&live.stdout));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(run(empty.path()).status.code(), Some(2));

        println!(
            "recover.cmd on hive files saved by reg.exe: {} files imported, 4 values restored, all hives \
             unloaded; refused the running Windows (exit 3) and a folder without Windows (exit 2). Output:\n{text}",
            plan.files.len()
        );
    }
}
