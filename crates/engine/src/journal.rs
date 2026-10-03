//! Transactional journal.
//!
//! Guarantees this module exists to provide:
//!
//! 1. **Nothing is written before its prior value is durable.** Each mutation
//!    captures the current value, appends a journal record, `fsync`s it, and
//!    only then touches the registry. A power loss mid-apply leaves a journal
//!    that describes more than was actually changed, which is recoverable. The
//!    reverse ordering would leave changes with no record, which is not.
//!
//! 2. **Absence is recorded as a value.** If a value did not exist before, the
//!    journal stores `None` and the `.reg` backup emits `"Name"=-`. Restoring a
//!    tweak that created a value therefore deletes it rather than writing a
//!    guessed default.
//!
//! 3. **The journal is readable without this binary.** It is line-delimited
//!    JSON, one self-contained record per line, values as comma-separated hex
//!    (`docs/journal-format.md`). Each change also leaves `.reg` files: one per
//!    value and one per applied change (`session_<tx>_<tweak>.reg`, newest
//!    write first). The recovery path when PeakTweaks cannot run is Safe Mode
//!    on the installed Windows: import the session file there. Not WinPE: its
//!    `HKLM\SYSTEM` and `HKEY_USERS` are WinPE's own, so importing there
//!    changes WinPE, not the broken install. For an install that will not
//!    start, `offline\recover.cmd` loads its hive files and imports remapped
//!    copies (`offline.rs`; never yet run in WinRE, NOTES.md N48).
//!
//! 4. **A damaged journal degrades, it does not vanish.** A torn final line
//!    (crash mid-append) is cut off at open and kept in a side file. A bad line
//!    in the middle is skipped and reported as a `JournalWarning`. Every append
//!    starts on a fresh line.
//!
//! Record kinds: a `write` is one registry change; a `commit` closes a
//! transaction. Which writes are still "outstanding" (applied and not since
//! reverted) is derived from the commits, and is what revert replays.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::{EngineError, Result};
use super::fsutil;
use super::secure_dir::TrustedDir;
use super::types::{ExecutionContext, RawValue, RegRoot};

pub const JOURNAL_FILE: &str = "journal.jsonl";

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// What a transaction was doing when it wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum JournalAction {
    Apply,
    Revert,
}

/// How a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CommitAction {
    /// An apply finished. Its writes are outstanding until a revert commits.
    Apply,
    /// A revert finished. Every earlier outstanding apply is now undone.
    Revert,
    /// An apply failed and its own writes were undone. Cancels only that
    /// transaction's writes; earlier applies stay outstanding.
    Rollback,
}

/// One registry change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// Monotonic within a file.
    pub seq: u64,
    /// The transaction this write belongs to (a `seq` reserved at begin).
    pub tx_id: u64,
    pub unix_ms: u64,
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
    /// Companion `.reg` file, relative to the journal directory.
    pub backup_file: String,
    /// Keys this write had to create, shallowest first, as paths relative to
    /// `root`. Revert removes them again if they are empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_keys: Vec<String>,
}

/// Closes a transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub seq: u64,
    pub tx_id: u64,
    pub unix_ms: u64,
    pub tweak_id: String,
    pub action: CommitAction,
}

/// How a restore point was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMethod {
    /// `SRSetRestorePointW` from srclient.dll.
    Api,
    /// `Checkpoint-Computer` through PowerShell.
    PowerShell,
}

/// A restore point PeakTweaks created and verified, for the Backups tab and for
/// support: which point to roll back to, and how it was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RestorePointRecord {
    pub seq: u64,
    pub unix_ms: u64,
    /// Windows' own sequence number for the point (`Get-ComputerRestorePoint`).
    pub sequence_number: u32,
    pub description: String,
    pub method: RestoreMethod,
    /// True when we turned System Protection on to do it.
    pub protection_enabled_by_us: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    Write(JournalEntry),
    Commit(CommitRecord),
    RestorePoint(RestorePointRecord),
}

impl Record {
    pub fn seq(&self) -> u64 {
        match self {
            Self::Write(e) => e.seq,
            Self::Commit(c) => c.seq,
            Self::RestorePoint(r) => r.seq,
        }
    }
}

// ---------------------------------------------------------------------------
// Warnings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum JournalWarningKind {
    /// A complete line that is not a valid record. Skipped.
    UnparsableLine,
    /// The file ended mid-record. Cut off at open and saved to a side file.
    TornTail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct JournalWarning {
    /// 1-based line number in the journal file.
    pub line: usize,
    pub kind: JournalWarningKind,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Parsing (pure, no I/O)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub records: Vec<Record>,
    pub warnings: Vec<JournalWarning>,
    /// Bytes of the file worth keeping; anything after is a torn tail.
    pub keep_len: usize,
    /// The last kept record has no trailing newline; one must be added.
    pub needs_newline: bool,
    /// The dropped torn tail, if any.
    pub torn_tail: Option<Vec<u8>>,
}

pub(crate) fn parse_journal(bytes: &[u8]) -> Parsed {
    let mut out = Parsed::default();
    let mut pos = 0usize;
    let mut line_no = 0usize;

    while pos < bytes.len() {
        let (line, next, complete) = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(i) => (&bytes[pos..pos + i], pos + i + 1, true),
            None => (&bytes[pos..], bytes.len(), false),
        };
        line_no += 1;
        let trimmed = line.trim_ascii();

        if trimmed.is_empty() {
            if complete {
                out.keep_len = next;
            } else {
                // Trailing whitespace with no newline: drop it silently.
                out.keep_len = pos;
            }
            pos = next;
            continue;
        }

        match serde_json::from_slice::<Record>(trimmed) {
            Ok(rec) => {
                out.records.push(rec);
                out.keep_len = next;
                if !complete {
                    out.needs_newline = true;
                }
            }
            Err(e) if complete => {
                out.warnings.push(JournalWarning {
                    line: line_no,
                    kind: JournalWarningKind::UnparsableLine,
                    detail: format!("skipped: {e}"),
                });
                out.keep_len = next;
            }
            Err(e) => {
                out.warnings.push(JournalWarning {
                    line: line_no,
                    kind: JournalWarningKind::TornTail,
                    detail: format!("{} bytes cut off the end: {e}", line.len()),
                });
                out.torn_tail = Some(bytes[pos..].to_vec());
                out.keep_len = pos;
            }
        }
        pos = next;
    }
    out
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct TweakIndex {
    /// Apply writes not yet undone, oldest first.
    outstanding: Vec<JournalEntry>,
    last_committed: Option<CommitAction>,
}

impl TweakIndex {
    fn observe(&mut self, rec: &Record) {
        match rec {
            Record::Write(e) if e.action == JournalAction::Apply => self.outstanding.push(e.clone()),
            Record::Write(_) => {}
            Record::Commit(c) => match c.action {
                CommitAction::Apply => self.last_committed = Some(CommitAction::Apply),
                CommitAction::Revert => {
                    self.outstanding.retain(|e| e.seq > c.tx_id);
                    self.last_committed = Some(CommitAction::Revert);
                }
                CommitAction::Rollback => self.outstanding.retain(|e| e.tx_id != c.tx_id),
            },
            Record::RestorePoint(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

pub struct Journal {
    root: PathBuf,
    journal_path: PathBuf,
    next_seq: u64,
    records: Vec<Record>,
    index: HashMap<String, TweakIndex>,
    warnings: Vec<JournalWarning>,
}

impl Journal {
    /// Open (or create) the journal in a trusted directory, repairing a torn
    /// tail. Reads the file once; everything after is served from memory.
    pub fn open(dir: &TrustedDir) -> Result<Self> {
        Self::load(dir.path().to_path_buf())
    }

    /// Throw away the in-memory copy and read the file again. Used after a panic
    /// that may have interrupted an append, so the index is never trusted over
    /// what is on disk. The directory was vetted when the journal was opened.
    pub fn reload(&mut self) -> Result<()> {
        *self = Self::load(self.root.clone())?;
        Ok(())
    }

    fn load(root: PathBuf) -> Result<Self> {
        fsutil::create_dir_durable(&root)?;
        let journal_path = root.join(JOURNAL_FILE);

        let bytes = match fs::read(&journal_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(EngineError::storage(journal_path.display().to_string(), e)),
        };

        let parsed = parse_journal(&bytes);

        if let Some(tail) = &parsed.torn_tail {
            // Keep the evidence, then cut the file back to the last good record.
            let side = root.join(format!("{JOURNAL_FILE}.torn-{}", now_ms()));
            fsutil::write_durable(&side, tail)?;
            fsutil::truncate_durable(&journal_path, parsed.keep_len as u64)?;
        } else if (parsed.keep_len as usize) < bytes.len() {
            fsutil::truncate_durable(&journal_path, parsed.keep_len as u64)?;
        }
        if parsed.needs_newline {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&journal_path)
                .map_err(|e| EngineError::storage(journal_path.display().to_string(), e))?;
            f.write_all(b"\n")
                .and_then(|()| f.sync_data())
                .map_err(|e| EngineError::storage(journal_path.display().to_string(), e))?;
        }

        let next_seq = parsed.records.iter().map(Record::seq).max().map_or(1, |m| m + 1);
        let mut index: HashMap<String, TweakIndex> = HashMap::new();
        for rec in &parsed.records {
            if let Some(id) = record_tweak(rec) {
                index.entry(id.to_owned()).or_default().observe(rec);
            }
        }

        Ok(Self {
            root,
            journal_path,
            next_seq,
            records: parsed.records,
            index,
            warnings: parsed.warnings,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_root_for_test(&mut self, root: PathBuf) {
        self.journal_path = root.join(JOURNAL_FILE);
        self.root = root;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    /// Warnings found when the file was opened. Shown in the Backups tab.
    pub fn warnings(&self) -> &[JournalWarning] {
        &self.warnings
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Reserve the next sequence number.
    pub fn take_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    /// Writes for one tweak, oldest first.
    pub fn entries_for(&self, tweak_id: &str) -> Vec<JournalEntry> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Write(e) if e.tweak_id == tweak_id => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    /// Apply writes for this tweak that have not been undone, oldest first.
    /// This is exactly what a revert replays (newest first).
    pub fn outstanding(&self, tweak_id: &str) -> &[JournalEntry] {
        self.index.get(tweak_id).map_or(&[], |i| i.outstanding.as_slice())
    }

    /// True when the tweak has an apply that has not since been reverted. This
    /// is what separates `Applied` from `Foreign` in `read_state`.
    pub fn is_applied(&self, tweak_id: &str) -> bool {
        !self.outstanding(tweak_id).is_empty()
    }

    pub fn last_committed(&self, tweak_id: &str) -> Option<CommitAction> {
        self.index.get(tweak_id).and_then(|i| i.last_committed)
    }

    /// Tweaks with outstanding applies, most recently applied first. Reverting
    /// in this order undoes overlapping tweaks in the reverse of how they were
    /// stacked.
    pub fn applied_tweaks_newest_first(&self) -> Vec<String> {
        let mut v: Vec<(u64, String)> = self
            .index
            .iter()
            .filter_map(|(id, i)| i.outstanding.iter().map(|e| e.seq).max().map(|s| (s, id.clone())))
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.0));
        v.into_iter().map(|(_, id)| id).collect()
    }

    pub fn append_write(&mut self, entry: JournalEntry) -> Result<()> {
        self.append(Record::Write(entry))
    }

    pub fn append_commit(&mut self, commit: CommitRecord) -> Result<()> {
        self.append(Record::Commit(commit))
    }

    pub fn append_restore_point(&mut self, rec: RestorePointRecord) -> Result<()> {
        self.append(Record::RestorePoint(rec))
    }

    /// Append one record on a fresh line and flush it to disk before returning.
    /// The in-memory state only changes once the record is durable.
    fn append(&mut self, rec: Record) -> Result<()> {
        let storage = |e: std::io::Error| EngineError::storage(self.journal_path.display().to_string(), e);

        let mut line = serde_json::to_vec(&rec).map_err(|e| EngineError::Storage {
            path: self.journal_path.display().to_string(),
            detail: format!("serialising journal record: {e}"),
        })?;
        line.push(b'\n');

        let existed = self.journal_path.exists();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.journal_path)
            .map_err(storage)?;

        // A previous append that failed midway can leave a partial line. Start
        // on a fresh line so the new record is never glued to it.
        let len = f.metadata().map_err(storage)?.len();
        let mut buf = Vec::with_capacity(line.len() + 1);
        if len > 0 {
            let mut last = [0u8; 1];
            f.seek(SeekFrom::End(-1)).map_err(storage)?;
            f.read_exact(&mut last).map_err(storage)?;
            if last[0] != b'\n' {
                buf.push(b'\n');
            }
        }
        buf.extend_from_slice(&line);

        // One write_all keeps the record contiguous; sync_data makes it durable
        // before the registry is touched.
        f.write_all(&buf).map_err(storage)?;
        f.sync_data().map_err(storage)?;
        drop(f);
        if !existed {
            fsutil::sync_dir(&self.root)?;
        }

        if let Some(id) = record_tweak(&rec) {
            self.index.entry(id.to_owned()).or_default().observe(&rec);
        }
        self.records.push(rec);
        Ok(())
    }
}

/// The tweak a record belongs to, if any. Restore-point records belong to none.
fn record_tweak(rec: &Record) -> Option<&str> {
    match rec {
        Record::Write(e) => Some(&e.tweak_id),
        Record::Commit(c) => Some(&c.tweak_id),
        Record::RestorePoint(_) => None,
    }
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::ExecutionContext;

    pub(crate) fn entry(seq: u64, tx_id: u64, tweak: &str, action: JournalAction) -> JournalEntry {
        JournalEntry {
            seq,
            tx_id,
            unix_ms: 0,
            tweak_id: tweak.into(),
            action,
            context: ExecutionContext::Service,
            root: RegRoot::LocalMachine,
            key_path: "K".into(),
            display_path: "HKEY_LOCAL_MACHINE\\K".into(),
            value_name: "V".into(),
            previous: Some(RawValue::dword(1)),
            written: Some(RawValue::dword(2)),
            backup_file: "b.reg".into(),
            created_keys: vec![],
        }
    }

    pub(crate) fn commit(seq: u64, tx_id: u64, tweak: &str, action: CommitAction) -> CommitRecord {
        CommitRecord {
            seq,
            tx_id,
            unix_ms: 0,
            tweak_id: tweak.into(),
            action,
        }
    }

    fn line(rec: &Record) -> Vec<u8> {
        let mut v = serde_json::to_vec(rec).unwrap();
        v.push(b'\n');
        v
    }

    fn open_in(dir: &tempfile::TempDir) -> Journal {
        Journal::open(&TrustedDir::insecure_for_tests(dir.path())).unwrap()
    }

    fn write_raw(dir: &tempfile::TempDir, bytes: &[u8]) {
        fs::write(dir.path().join(JOURNAL_FILE), bytes).unwrap();
    }

    #[test]
    fn records_round_trip_as_single_lines() {
        let rec = Record::Write(entry(1, 1, "t", JournalAction::Apply));
        let bytes = line(&rec);
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
        let p = parse_journal(&bytes);
        assert_eq!(p.records, vec![rec]);
        assert!(p.warnings.is_empty());
        assert_eq!(p.keep_len, bytes.len());
    }

    #[test]
    fn empty_file_opens_with_seq_one() {
        let d = tempfile::tempdir().unwrap();
        write_raw(&d, b"");
        let mut j = open_in(&d);
        assert_eq!(j.take_seq(), 1);
        assert!(j.warnings().is_empty());
    }

    #[test]
    fn torn_final_line_is_cut_off_saved_and_next_append_is_readable() {
        let d = tempfile::tempdir().unwrap();
        let good = Record::Write(entry(1, 1, "t", JournalAction::Apply));
        let mut bytes = line(&good);
        let second = line(&Record::Write(entry(2, 1, "t", JournalAction::Apply)));
        bytes.extend_from_slice(&second[..second.len() / 2]); // torn, no newline
        write_raw(&d, &bytes);

        let mut j = open_in(&d);
        assert_eq!(j.records().len(), 1);
        assert_eq!(j.warnings().len(), 1);
        assert_eq!(j.warnings()[0].kind, JournalWarningKind::TornTail);
        // Evidence kept.
        let torn: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".torn-"))
            .collect();
        assert_eq!(torn.len(), 1);
        // File cut back to the good record.
        assert_eq!(fs::read(d.path().join(JOURNAL_FILE)).unwrap(), line(&good));

        let seq = j.take_seq();
        assert_eq!(seq, 2);
        j.append_write(entry(seq, 1, "t", JournalAction::Apply)).unwrap();
        let reopened = open_in(&d);
        assert_eq!(reopened.records().len(), 2);
        assert!(reopened.warnings().is_empty());
    }

    #[test]
    fn complete_final_record_without_newline_is_kept_and_terminated() {
        let d = tempfile::tempdir().unwrap();
        let mut bytes = line(&Record::Write(entry(1, 1, "t", JournalAction::Apply)));
        bytes.pop(); // drop the newline only
        write_raw(&d, &bytes);

        let mut j = open_in(&d);
        assert_eq!(j.records().len(), 1);
        assert!(j.warnings().is_empty());
        let seq = j.take_seq();
        j.append_write(entry(seq, 1, "t", JournalAction::Apply)).unwrap();
        assert_eq!(open_in(&d).records().len(), 2);
    }

    #[test]
    fn garbage_mid_file_is_skipped_with_a_warning_and_later_records_survive() {
        let d = tempfile::tempdir().unwrap();
        let mut bytes = line(&Record::Write(entry(1, 1, "t", JournalAction::Apply)));
        bytes.extend_from_slice(b"{this is not json\n");
        bytes.extend_from_slice(&line(&Record::Commit(commit(2, 1, "t", CommitAction::Apply))));
        write_raw(&d, &bytes);

        let j = open_in(&d);
        assert_eq!(j.records().len(), 2, "records after the garbage line must survive");
        assert_eq!(j.warnings().len(), 1);
        assert_eq!(j.warnings()[0].kind, JournalWarningKind::UnparsableLine);
        assert_eq!(j.warnings()[0].line, 2);
        assert!(j.is_applied("t"));
        assert_eq!(j.last_committed("t"), Some(CommitAction::Apply));
    }

    #[test]
    fn trailing_partial_utf8_is_treated_as_a_torn_tail() {
        let d = tempfile::tempdir().unwrap();
        let mut bytes = line(&Record::Write(entry(1, 1, "t", JournalAction::Apply)));
        bytes.extend_from_slice(&[b'{', b'"', 0xE2, 0x82]); // half of a 3-byte char
        write_raw(&d, &bytes);

        let mut j = open_in(&d);
        assert_eq!(j.records().len(), 1);
        assert_eq!(j.warnings()[0].kind, JournalWarningKind::TornTail);
        let seq = j.take_seq();
        j.append_commit(commit(seq, 1, "t", CommitAction::Apply)).unwrap();
        assert_eq!(open_in(&d).records().len(), 2);
    }

    #[test]
    fn append_after_a_failed_partial_append_starts_a_new_line() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        let s = j.take_seq();
        j.append_write(entry(s, s, "t", JournalAction::Apply)).unwrap();
        // Simulate a partial write left by a failed append.
        let path = d.path().join(JOURNAL_FILE);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"record\":\"wri").unwrap();
        drop(f);

        let s = j.take_seq();
        j.append_write(entry(s, s, "t", JournalAction::Apply)).unwrap();
        let reopened = open_in(&d);
        assert_eq!(reopened.records().len(), 2);
        assert_eq!(reopened.warnings().len(), 1, "the partial line is reported, not fatal");
    }

    #[test]
    fn seq_is_monotonic_across_reopen() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        let mut last = 0;
        for _ in 0..3 {
            let s = j.take_seq();
            assert!(s > last);
            last = s;
            j.append_write(entry(s, s, "t", JournalAction::Apply)).unwrap();
        }
        let mut j2 = open_in(&d);
        assert!(j2.take_seq() > last);
    }

    #[test]
    fn outstanding_tracks_commits() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        // tx 1: apply, committed
        j.append_write(entry(2, 1, "t", JournalAction::Apply)).unwrap();
        j.append_commit(commit(3, 1, "t", CommitAction::Apply)).unwrap();
        assert!(j.is_applied("t"));
        // tx 4: revert, committed
        j.append_write(entry(5, 4, "t", JournalAction::Revert)).unwrap();
        j.append_commit(commit(6, 4, "t", CommitAction::Revert)).unwrap();
        assert!(!j.is_applied("t"));
        assert_eq!(j.last_committed("t"), Some(CommitAction::Revert));
        // tx 7: apply that gets rolled back must not disturb earlier state
        j.append_write(entry(8, 7, "t", JournalAction::Apply)).unwrap();
        j.append_commit(commit(9, 7, "t", CommitAction::Rollback)).unwrap();
        assert!(!j.is_applied("t"));
    }

    #[test]
    fn rollback_cancels_only_its_own_transaction() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        j.append_write(entry(2, 1, "t", JournalAction::Apply)).unwrap();
        j.append_commit(commit(3, 1, "t", CommitAction::Apply)).unwrap();
        j.append_write(entry(5, 4, "t", JournalAction::Apply)).unwrap();
        j.append_commit(commit(6, 4, "t", CommitAction::Rollback)).unwrap();
        assert_eq!(j.outstanding("t").len(), 1);
        assert_eq!(j.outstanding("t")[0].seq, 2);
    }

    #[test]
    fn uncommitted_trailing_apply_counts_as_outstanding() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        j.append_write(entry(2, 1, "t", JournalAction::Apply)).unwrap();
        let re = open_in(&d);
        assert!(re.is_applied("t"));
        assert_eq!(re.last_committed("t"), None);
    }

    #[test]
    fn applied_tweaks_are_ordered_by_last_apply_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let mut j = open_in(&d);
        j.append_write(entry(2, 1, "a", JournalAction::Apply)).unwrap();
        j.append_commit(commit(3, 1, "a", CommitAction::Apply)).unwrap();
        j.append_write(entry(5, 4, "b", JournalAction::Apply)).unwrap();
        j.append_commit(commit(6, 4, "b", CommitAction::Apply)).unwrap();
        assert_eq!(j.applied_tweaks_newest_first(), vec!["b", "a"]);
        // Re-applying "a" makes it the newest.
        j.append_write(entry(8, 7, "a", JournalAction::Apply)).unwrap();
        j.append_commit(commit(9, 7, "a", CommitAction::Apply)).unwrap();
        assert_eq!(j.applied_tweaks_newest_first(), vec!["a", "b"]);
    }
}
