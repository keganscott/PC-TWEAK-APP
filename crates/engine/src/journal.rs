//! Transactional journal, `.reg` backup export, and the mutation gateway.
//!
//! Three guarantees this module exists to provide:
//!
//! 1. **Nothing is written before its prior value is durable.** Each mutation
//!    captures the current value, appends a journal record, `fsync`s it, and
//!    only then touches the registry. A power loss mid-apply leaves a journal
//!    that describes more than was actually changed, which is recoverable.
//!    The reverse ordering would leave changes with no record, which is not.
//!
//! 2. **Absence is recorded as a value.** If a value did not exist before, the
//!    journal stores `None` and the `.reg` backup emits `"Name"=-`. Restoring a
//!    tweak that created a value therefore deletes it rather than writing a
//!    guessed default. This is the single most commonly botched part of a
//!    rollback engine.
//!
//! 3. **The journal is readable without this binary.** It is line-delimited
//!    JSON, one self-contained record per line, with values as comma-separated
//!    hex. A torn final line from an unclean shutdown is discarded on parse
//!    rather than poisoning the file. Someone in WinPE with Notepad and the
//!    `.reg` files can recover a machine that will not boot — which is exactly
//!    the scenario MSI-mode tweaks can produce.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winreg::enums::REG_NONE;

use super::context::ContextResolver;
use super::error::{EngineError, Result};
use super::types::{ExecutionContext, RawValue, RegRoot};

const JOURNAL_FILE: &str = "journal.jsonl";
const BACKUP_DIR: &str = "peaktweaks_backups";

// ---------------------------------------------------------------------------
// Journal records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalAction {
    Apply,
    Revert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Monotonic within a file. Rollback replays descending.
    pub seq: u64,
    pub unix_ms: u128,
    pub tweak_id: String,
    pub action: JournalAction,
    pub context: ExecutionContext,
    pub root: RegRoot,
    /// Path without the hive prefix, as the tweak declared it.
    pub key_path: String,
    /// Fully-qualified path including the resolved SID. What the `.reg` uses.
    pub display_path: String,
    pub value_name: String,
    /// `None` means the value did not exist. Restoring it means deleting.
    pub previous: Option<RawValue>,
    pub written: Option<RawValue>,
    /// Companion `.reg` file, relative to the backup root.
    pub backup_file: String,
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

pub struct Journal {
    root: PathBuf,
    journal_path: PathBuf,
    next_seq: u64,
}

impl Journal {
    /// `app_data` is the Tauri app data dir; backups land in
    /// `<app_data>/peaktweaks_backups/`.
    pub fn open(app_data: &Path) -> Result<Self> {
        let root = app_data.join(BACKUP_DIR);
        fs::create_dir_all(&root).map_err(|e| EngineError::storage(root.display().to_string(), e))?;

        let journal_path = root.join(JOURNAL_FILE);
        let next_seq = Self::read_all_at(&journal_path)?
            .last()
            .map(|e| e.seq + 1)
            .unwrap_or(1);

        Ok(Self { root, journal_path, next_seq })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> Result<Vec<JournalEntry>> {
        Self::read_all_at(&self.journal_path)
    }

    /// Entries for one tweak, oldest first.
    pub fn entries_for(&self, tweak_id: &str) -> Result<Vec<JournalEntry>> {
        Ok(self.entries()?.into_iter().filter(|e| e.tweak_id == tweak_id).collect())
    }

    /// True when the tweak has an apply that has not since been reverted.
    /// This is what separates `Applied` from `Foreign` in `read_state`.
    pub fn is_applied(&self, tweak_id: &str) -> Result<bool> {
        Ok(self
            .entries_for(tweak_id)?
            .last()
            .is_some_and(|e| matches!(e.action, JournalAction::Apply)))
    }

    fn read_all_at(path: &Path) -> Result<Vec<JournalEntry>> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(EngineError::storage(path.display().to_string(), e)),
        };

        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(l) => l,
                // A torn tail from an unclean shutdown. Everything before it is
                // still valid, so stop rather than fail.
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JournalEntry>(&line) {
                Ok(entry) => out.push(entry),
                Err(_) => break,
            }
        }
        Ok(out)
    }

    /// Append one record and flush it to disk before returning.
    fn append(&mut self, entry: &JournalEntry) -> Result<()> {
        let mut line = serde_json::to_string(entry).map_err(|e| EngineError::Storage {
            path: self.journal_path.display().to_string(),
            detail: format!("serialising journal entry: {e}"),
        })?;
        line.push('\n');

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal_path)
            .map_err(|e| EngineError::storage(self.journal_path.display().to_string(), e))?;

        // Single write_all keeps the record contiguous; sync_data makes it
        // durable before the registry is touched.
        f.write_all(line.as_bytes())
            .map_err(|e| EngineError::storage(self.journal_path.display().to_string(), e))?;
        f.sync_data()
            .map_err(|e| EngineError::storage(self.journal_path.display().to_string(), e))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// .reg export
// ---------------------------------------------------------------------------

/// Emit a `.reg` file restoring one value to `previous`.
///
/// Written as UTF-16LE with a BOM, which is what `regedit.exe` requires for the
/// "Windows Registry Editor Version 5.00" header. A UTF-8 `.reg` file imports
/// as mojibake and is a classic silent-corruption bug.
fn write_reg_backup(
    dir: &Path,
    file_stem: &str,
    display_path: &str,
    value_name: &str,
    previous: Option<&RawValue>,
) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|e| EngineError::storage(dir.display().to_string(), e))?;
    let path = dir.join(format!("{file_stem}.reg"));

    let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    text.push_str(&format!("[{display_path}]\r\n"));
    text.push_str(&format!("{}\r\n", reg_value_line(value_name, previous)));

    let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));

    fs::write(&path, &bytes).map_err(|e| EngineError::storage(path.display().to_string(), e))?;
    Ok(path)
}

/// One `.reg` value line. `None` produces a deletion directive.
fn reg_value_line(name: &str, value: Option<&RawValue>) -> String {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");

    let Some(v) = value else {
        // The value did not exist. Restoring means removing it.
        return format!("\"{escaped}\"=-");
    };

    match v.vtype {
        // REG_SZ
        1 => {
            let s = v.as_sz().unwrap_or_default();
            let s = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"=\"{s}\"")
        }
        // REG_DWORD
        4 => format!("\"{escaped}\"=dword:{:08x}", v.as_dword().unwrap_or(0)),
        // REG_BINARY
        3 => format!("\"{escaped}\"=hex:{}", hex_wrapped(&v.bytes, escaped.len() + 7)),
        // Everything else uses the typed hex form: hex(2) expand_sz,
        // hex(7) multi_sz, hex(b) qword, hex(0) none.
        other => format!(
            "\"{escaped}\"=hex({:x}):{}",
            other,
            hex_wrapped(&v.bytes, escaped.len() + 12)
        ),
    }
}

/// Comma-separated hex with `\` continuations. `.reg` lines wrap at 80 columns;
/// regedit tolerates longer but other parsers do not, so we conform.
fn hex_wrapped(bytes: &[u8], first_line_prefix: usize) -> String {
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

// ---------------------------------------------------------------------------
// Transaction — the only path to a mutation
// ---------------------------------------------------------------------------

/// Handed to `Tweak::apply` and `Tweak::revert`. Every write goes through here,
/// so every write is backed up and journalled. There is no escape hatch, which
/// is the point.
pub struct Transaction<'a> {
    tweak_id: &'static str,
    context: ExecutionContext,
    resolver: &'a ContextResolver,
    journal: &'a mut Journal,
    session_dir: PathBuf,
    action: JournalAction,
    written: Vec<JournalEntry>,
}

impl<'a> Transaction<'a> {
    pub fn begin(
        tweak_id: &'static str,
        context: ExecutionContext,
        resolver: &'a ContextResolver,
        journal: &'a mut Journal,
        action: JournalAction,
    ) -> Result<Self> {
        if !resolver.elevated() {
            return Err(EngineError::NotElevated);
        }
        let session_dir = journal.root().join(date_stamp());
        Ok(Self {
            tweak_id,
            context,
            resolver,
            journal,
            session_dir,
            action,
            written: Vec::new(),
        })
    }

    /// Records touched by this transaction, for the UI and for undo.
    pub fn written(&self) -> &[JournalEntry] {
        &self.written
    }

    pub fn set_dword(&mut self, root: RegRoot, key: &str, name: &str, value: u32) -> Result<()> {
        self.set_raw(root, key, name, RawValue::dword(value))
    }

    pub fn set_string(&mut self, root: RegRoot, key: &str, name: &str, value: &str) -> Result<()> {
        self.set_raw(root, key, name, RawValue::sz(value))
    }

    /// Core mutation. Read prior value, back it up, journal it, then write.
    pub fn set_raw(&mut self, root: RegRoot, key: &str, name: &str, value: RawValue) -> Result<()> {
        self.guard_context(root)?;

        let display_path = self.resolver.display_path(root, key);
        let previous = self.read_current(root, key, name)?;

        // No-op writes still cost a journal entry and a backup file, so skip
        // them. Reverting to a value that is already set is common — a user
        // toggling twice — and should not litter the backup directory.
        if previous.as_ref() == Some(&value) {
            return Ok(());
        }

        let seq = self.journal.next_seq;
        let stem = format!("{}_{}_{}", seq, sanitise(self.tweak_id), sanitise(name));
        let backup = write_reg_backup(&self.session_dir, &stem, &display_path, name, previous.as_ref())?;
        let backup_rel = backup
            .strip_prefix(self.journal.root())
            .unwrap_or(&backup)
            .to_string_lossy()
            .replace('\\', "/");

        let entry = JournalEntry {
            seq,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.to_string(),
            action: self.action.clone(),
            context: self.context,
            root,
            key_path: key.to_string(),
            display_path: display_path.clone(),
            value_name: name.to_string(),
            previous,
            written: Some(value.clone()),
            backup_file: backup_rel,
        };

        // Durable before the registry moves. Ordering is load-bearing.
        self.journal.append(&entry)?;
        self.journal.next_seq += 1;

        let regkey = self.resolver.open(root, key, true)?;
        let raw = winreg::RegValue {
            bytes: value.bytes.clone(),
            vtype: vtype_from_u32(value.vtype),
        };
        regkey
            .set_raw_value(name, &raw)
            .map_err(|e| EngineError::registry(display_path, Some(name), e))?;

        self.written.push(entry);
        Ok(())
    }

    /// Delete a value, recording its prior contents so it can come back.
    pub fn delete_value(&mut self, root: RegRoot, key: &str, name: &str) -> Result<()> {
        self.guard_context(root)?;

        let display_path = self.resolver.display_path(root, key);
        let Some(previous) = self.read_current(root, key, name)? else {
            return Ok(()); // already absent
        };

        let seq = self.journal.next_seq;
        let stem = format!("{}_{}_{}", seq, sanitise(self.tweak_id), sanitise(name));
        let backup = write_reg_backup(&self.session_dir, &stem, &display_path, name, Some(&previous))?;
        let backup_rel = backup
            .strip_prefix(self.journal.root())
            .unwrap_or(&backup)
            .to_string_lossy()
            .replace('\\', "/");

        let entry = JournalEntry {
            seq,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.to_string(),
            action: self.action.clone(),
            context: self.context,
            root,
            key_path: key.to_string(),
            display_path: display_path.clone(),
            value_name: name.to_string(),
            previous: Some(previous),
            written: None,
            backup_file: backup_rel,
        };

        self.journal.append(&entry)?;
        self.journal.next_seq += 1;

        let regkey = self.resolver.open(root, key, false)?;
        regkey
            .delete_value(name)
            .map_err(|e| EngineError::registry(display_path, Some(name), e))?;

        self.written.push(entry);
        Ok(())
    }

    /// Default revert path: replay this tweak's apply records in reverse,
    /// putting each value back to `previous` — deleting it when `previous` was
    /// `None`, which is the case that matters.
    pub fn restore_journalled(&mut self, tweak_id: &str) -> Result<()> {
        let mut applies: Vec<JournalEntry> = self
            .journal
            .entries_for(tweak_id)?
            .into_iter()
            .filter(|e| matches!(e.action, JournalAction::Apply))
            .collect();

        if applies.is_empty() {
            return Err(EngineError::NoJournalEntry {
                tweak_id: tweak_id.to_string(),
            });
        }

        applies.sort_by_key(|e| std::cmp::Reverse(e.seq));

        for entry in applies {
            match entry.previous.clone() {
                Some(prev) => self.set_raw(entry.root, &entry.key_path, &entry.value_name, prev)?,
                None => self.delete_value(entry.root, &entry.key_path, &entry.value_name)?,
            }
        }
        Ok(())
    }

    /// A `User`-context tweak must not reach HKLM, and vice versa. Without this
    /// the context split is a naming convention rather than a guarantee.
    fn guard_context(&self, root: RegRoot) -> Result<()> {
        if root.required_context() != self.context {
            return Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.to_string(),
                detail: format!(
                    "declared {:?} but attempted a write to {:?}",
                    self.context, root
                ),
            });
        }
        Ok(())
    }

    fn read_current(&self, root: RegRoot, key: &str, name: &str) -> Result<Option<RawValue>> {
        let Ok(regkey) = self.resolver.open_read(root, key) else {
            return Ok(None); // key absent means value absent
        };
        match regkey.get_raw_value(name) {
            Ok(v) => Ok(Some(RawValue {
                vtype: v.vtype as u32,
                bytes: v.bytes,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(EngineError::registry(
                self.resolver.display_path(root, key),
                Some(name),
                e,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn vtype_from_u32(v: u32) -> winreg::enums::RegType {
    use winreg::enums::RegType::*;
    match v {
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        11 => REG_QWORD,
        _ => REG_NONE,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `YYYY-MM-DD` without pulling in chrono. Days since epoch via civil-from-days.
fn date_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Registry value names allow characters that filenames do not.
fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_value_becomes_a_deletion_directive() {
        assert_eq!(reg_value_line("Win32PrioritySeparation", None), "\"Win32PrioritySeparation\"=-");
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
        let v = RawValue { vtype: 3, bytes: vec![0xAB; 64] };
        let line = reg_value_line("Curve", Some(&v));
        assert!(line.contains("\\\r\n"), "expected a continuation, got: {line}");
    }

    #[test]
    fn backslashes_in_names_are_escaped() {
        assert_eq!(reg_value_line("a\\b", None), "\"a\\\\b\"=-");
    }
}
