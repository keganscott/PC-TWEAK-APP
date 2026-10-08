//! The mutation gateway.
//!
//! `Transaction` is handed to `Tweak::apply` and `Tweak::revert`. Every write
//! goes through it, so every write is allowlist-checked, backed up and
//! journalled. There is no escape hatch, which is the point.
//!
//! A transaction ends in exactly one of three ways:
//!   * `commit`: appends a `Commit` record; the engine calls this on success.
//!   * `rollback`: an apply failed midway; its own writes are undone and a
//!     `Commit(Rollback)` cancels them in the journal.
//!   * dropped without either (a crash, or a failed revert): nothing more is
//!     written. The journal then still lists the writes as outstanding, so a
//!     later revert finishes the job. Revert is idempotent, so that is safe.

use std::path::PathBuf;

use super::context::ContextResolver;
use super::error::{EngineError, Result};
use super::journal::{
    now_ms, ChangeEntry, CommitAction, CommitRecord, EffectRecord, Journal, JournalAction, JournalEntry, NoteRecord,
    RegBackup,
};
use super::offline;
use super::reg_export::{write_reg_backup, write_session_backup};
use super::registry::{
    components, is_ancestor_or_equal, path_eq, pattern_eq, pattern_is_ancestor_or_equal, value_name_matches, WILDCARD,
};
use super::system::{SideEffect, SysItem, SysState};
use super::types::{ExecutionContext, RawValue, RegRoot, RegTarget, Tweak};

#[must_use = "a transaction must be committed or rolled back"]
pub struct Transaction<'a> {
    tweak_id: String,
    context: ExecutionContext,
    allowlist: Vec<RegTarget>,
    resolver: &'a ContextResolver,
    journal: &'a mut Journal,
    session_dir: PathBuf,
    action: JournalAction,
    tx_id: u64,
    written: Vec<JournalEntry>,
    /// Declared non-registry items (`Tweak::system_targets`).
    sys_allowlist: Vec<SysItem>,
    /// Non-registry changes made by this transaction so far.
    changes: Vec<ChangeEntry>,
    /// Run after the commit, in order, each once.
    effects: Vec<SideEffect>,
    /// Declared side effects (`Tweak::effect_targets`).
    effect_allowlist: Vec<SideEffect>,
}

impl<'a> Transaction<'a> {
    pub fn begin(
        tweak: &dyn Tweak,
        resolver: &'a ContextResolver,
        journal: &'a mut Journal,
        action: JournalAction,
    ) -> Result<Self> {
        if !resolver.elevated() {
            return Err(EngineError::NotElevated);
        }
        let session_dir = journal.root().join("backups").join(crate::timeutil::date_stamp());
        let tx_id = journal.take_seq();
        Ok(Self {
            tweak_id: tweak.id().to_owned(),
            context: tweak.execution_context(),
            allowlist: tweak.touches(),
            resolver,
            journal,
            session_dir,
            action,
            tx_id,
            written: Vec::new(),
            sys_allowlist: tweak.system_targets(),
            changes: Vec::new(),
            effects: Vec::new(),
            effect_allowlist: tweak.effect_targets(),
        })
    }

    /// Non-registry changes made by this transaction so far.
    pub fn changes(&self) -> &[ChangeEntry] {
        &self.changes
    }

    /// Change a non-registry item (`system.rs`): check it is declared, read and
    /// journal what it is now, then change it. Files go through `write_file`.
    pub fn set_system(&mut self, item: SysItem, state: SysState) -> Result<()> {
        if matches!(item, SysItem::File { .. }) || matches!(state, SysState::File { .. }) {
            return Err(EngineError::Internal {
                detail: "files are changed with write_file".into(),
            });
        }
        self.check_sys_allowed(&item)?;
        let previous = self.resolver.system().read(&item)?;
        if previous == state {
            return Ok(());
        }
        self.check_put_back_possible(&item, &previous)?;
        self.record_change(item.clone(), previous, state.clone())?;
        self.resolver.system().write(&item, &state)
    }

    /// A power setting Windows hides reads as `Absent`, and `Absent` cannot be
    /// written back. So one may only be changed on a plan copy this tweak made
    /// (in this transaction or an earlier apply still outstanding), which Undo
    /// deletes as a whole.
    fn check_put_back_possible(&self, item: &SysItem, previous: &SysState) -> Result<()> {
        let SysItem::PowerSetting { scheme, .. } = item else {
            return Ok(());
        };
        if *previous != SysState::Absent {
            return Ok(());
        }
        let ours = |c: &ChangeEntry| {
            c.previous == SysState::Absent
                && matches!(&c.item, SysItem::PowerScheme { guid } if guid.eq_ignore_ascii_case(scheme))
        };
        if self.changes.iter().any(ours) || self.journal.outstanding_changes(&self.tweak_id).iter().any(ours) {
            return Ok(());
        }
        Err(EngineError::ContextViolation {
            tweak_id: self.tweak_id.clone(),
            detail: format!(
                "the {} has no value of its own to put back, and the plan is not a copy PeakTweaks made",
                item.describe()
            ),
        })
    }

    /// Replace a whole file. A copy of what it held (or that it did not exist)
    /// is kept with the backups and journalled first; revert puts it back.
    pub fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        let item = SysItem::File { path: path.to_owned() };
        self.check_sys_allowed(&item)?;
        let before = self.resolver.system().read_file(path)?;
        if before.as_deref() == Some(bytes) {
            return Ok(());
        }
        let previous = match &before {
            Some(b) => self.keep_file_copy(b, "before")?,
            None => SysState::Absent,
        };
        let written = self.keep_file_copy(bytes, "after")?;
        self.record_change(item, previous, written)?;
        self.resolver.system().write_file(path, Some(bytes))
    }

    /// Run `effect` once this transaction commits (after apply or revert).
    /// Journalled with its outcome; a failure does not undo the change.
    pub fn after_commit(&mut self, effect: SideEffect) -> Result<()> {
        if !self.effect_allowlist.iter().any(|p| effect.matches(p)) {
            return Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.clone(),
                detail: format!("\"{}\" is not in this tweak's declared side effects", effect.describe()),
            });
        }
        if !self.effects.contains(&effect) {
            self.effects.push(effect);
        }
        Ok(())
    }

    pub fn tweak_id(&self) -> &str {
        &self.tweak_id
    }

    /// Records written by this transaction so far, for the UI and for undo.
    pub fn written(&self) -> &[JournalEntry] {
        &self.written
    }

    /// True when the interactive user is the user this process runs as, so
    /// per-session Win32 calls (which act on the calling user) hit the right
    /// profile. Live-push calls such as `SystemParametersInfoW` must check it.
    pub fn user_is_self(&self) -> bool {
        self.resolver.user().is_self
    }

    /// Read access for a tweak that needs to look at the result of its own
    /// writes (for example to push a live setting).
    pub fn resolver(&self) -> &ContextResolver {
        self.resolver
    }

    pub fn set_dword(&mut self, root: RegRoot, key: &str, name: &str, value: u32) -> Result<()> {
        self.set_raw(root, key, name, RawValue::dword(value))
    }

    pub fn set_string(&mut self, root: RegRoot, key: &str, name: &str, value: &str) -> Result<()> {
        self.set_raw(root, key, name, RawValue::sz(value))
    }

    /// Core mutation. Check the allowlist, read the prior value, back it up,
    /// journal it, then write.
    pub fn set_raw(&mut self, root: RegRoot, key: &str, name: &str, value: RawValue) -> Result<()> {
        self.check_allowed(root, key, name)?;
        let display_path = self.resolver.display_path(root, key);
        if !value.is_supported_type() {
            return Err(EngineError::UnsupportedValueType {
                path: display_path,
                value: name.to_owned(),
                vtype: value.vtype,
            });
        }

        let previous = self.read_current(root, key, name)?;

        // No-op writes still cost a journal entry and a backup file, so skip
        // them. Reverting to a value that is already set is common and should
        // not litter the backup directory.
        if previous.as_ref() == Some(&value) {
            return Ok(());
        }

        let created_keys = self.missing_ancestors(root, key)?;
        self.record(root, key, name, previous, Some(value.clone()), created_keys)?;

        let (hive, full) = self.resolver.route(root, key);
        self.resolver.backend().write_value(hive, &full, name, &value)?;
        Ok(())
    }

    /// Delete a value, recording its prior contents so it can come back.
    pub fn delete_value(&mut self, root: RegRoot, key: &str, name: &str) -> Result<()> {
        self.check_allowed(root, key, name)?;
        let Some(previous) = self.read_current(root, key, name)? else {
            return Ok(()); // already absent
        };
        self.record(root, key, name, Some(previous), None, Vec::new())?;

        let (hive, full) = self.resolver.route(root, key);
        self.resolver.backend().delete_value(hive, &full, name)?;
        Ok(())
    }

    /// Default revert path: put back every value this tweak has outstanding
    /// applies for, newest first, to what it was before the first of them.
    /// Deletes values that did not exist, and removes keys we created if they
    /// are now empty.
    ///
    /// Everything is validated against the tweak's allowlist before the first
    /// write, so a tampered or stale journal line cannot cause a partial
    /// restore followed by a refusal.
    pub fn restore_journalled(&mut self) -> Result<()> {
        let entries: Vec<JournalEntry> = self.journal.outstanding(&self.tweak_id).to_vec();
        let changes: Vec<ChangeEntry> = self.journal.outstanding_changes(&self.tweak_id).to_vec();
        if entries.is_empty() && changes.is_empty() {
            return Err(EngineError::NoJournalEntry {
                tweak_id: self.tweak_id.clone(),
            });
        }

        for e in &entries {
            self.check_same_account(e)?;
            self.check_allowed(e.root, &e.key_path, &e.value_name)?;
            for k in &e.created_keys {
                self.check_key_removable(e.root, k)?;
            }
        }
        for c in &changes {
            self.check_sys_allowed(&c.item)?;
        }

        // Registry writes and other changes, newest first across both.
        enum Step {
            Reg(JournalEntry),
            Sys(ChangeEntry),
        }
        let mut steps: Vec<(u64, Step)> = entries
            .into_iter()
            .map(|e| (e.seq, Step::Reg(e)))
            .chain(changes.into_iter().map(|c| (c.seq, Step::Sys(c))))
            .collect();
        steps.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));

        for (_, step) in steps {
            match step {
                Step::Reg(e) => {
                    if self.per_pc_key_is_gone(&e)? {
                        self.note(format!(
                            "{} no longer exists (the device or adapter was removed), so {} was not put back",
                            e.display_path, e.value_name
                        ))?;
                        continue;
                    }
                    match e.previous.clone() {
                        Some(prev) => self.set_raw(e.root, &e.key_path, &e.value_name, prev)?,
                        None => self.delete_value(e.root, &e.key_path, &e.value_name)?,
                    }
                    self.remove_created_keys(e.root, &e.created_keys)?;
                }
                Step::Sys(c) => {
                    if let Some(name) = self.adapter_is_gone(&c.item) {
                        self.note(format!(
                            "network adapter {name} no longer exists (it was removed), so its {} was not put back",
                            c.item.describe()
                        ))?;
                        continue;
                    }
                    self.change_back(&c.item, &c.previous)?
                }
            }
        }
        Ok(())
    }

    /// Put a non-registry item back to `target`, journalled like any change.
    fn change_back(&mut self, item: &SysItem, target: &SysState) -> Result<()> {
        let current = match item {
            SysItem::File { path } => {
                let bytes = self.resolver.system().read_file(path)?;
                let same = match (&bytes, target) {
                    (None, SysState::Absent) => true,
                    (Some(b), SysState::File { sha256, .. }) => hex_sha256(b) == *sha256,
                    _ => false,
                };
                if same {
                    return Ok(());
                }
                match bytes {
                    Some(b) => self.keep_file_copy(&b, "before")?,
                    None => SysState::Absent,
                }
            }
            _ => {
                let now = self.resolver.system().read(item)?;
                if now == *target {
                    return Ok(());
                }
                now
            }
        };
        self.record_change(item.clone(), current, target.clone())?;
        self.put_back(item, target)
    }

    /// Make `item` be `state` without journalling (rollback, and the last step
    /// of `change_back`).
    fn put_back(&self, item: &SysItem, state: &SysState) -> Result<()> {
        let system = self.resolver.system();
        match (item, state) {
            (SysItem::File { path }, SysState::Absent) => system.write_file(path, None),
            (SysItem::File { path }, SysState::File { backup, sha256 }) => {
                let src = self.journal.root().join(backup);
                let bytes = std::fs::read(&src).map_err(|e| EngineError::storage(src.display().to_string(), e))?;
                if hex_sha256(&bytes) != *sha256 {
                    return Err(EngineError::Storage {
                        path: src.display().to_string(),
                        detail: "the saved copy of this file does not match its recorded checksum".into(),
                    });
                }
                system.write_file(path, Some(&bytes))
            }
            (SysItem::File { .. }, _) | (_, SysState::File { .. }) => Err(EngineError::Internal {
                detail: format!(
                    "a journal record for the {} has the wrong kind of state",
                    item.describe()
                ),
            }),
            (item, state) => system.write(item, state),
        }
    }

    /// Close the transaction successfully.
    pub fn commit(mut self) -> Result<Vec<JournalEntry>> {
        // One combined restore file per applied change, for recovery by hand
        // when PeakTweaks cannot run (Safe Mode; see journal.rs). Written
        // before the commit so a committed apply always has one.
        let values: Vec<(&str, &str, Option<&RawValue>)> = self
            .written
            .iter()
            .map(|e| (e.display_path.as_str(), e.value_name.as_str(), e.previous.as_ref()))
            .chain(
                self.changes
                    .iter()
                    .flat_map(|c| &c.reg_backups)
                    .map(|b| (b.display_path.as_str(), b.value_name.as_str(), b.previous.as_ref())),
            )
            .collect();
        if self.action == JournalAction::Apply && !values.is_empty() {
            let stem = format!("session_{}_{}", self.tx_id, sanitise(&self.tweak_id));
            write_session_backup(&self.session_dir, &stem, &values)?;
        }
        // The offline undo set (offline.rs), as it must be once this commit is
        // written. Also before the commit, so a committed change always has it.
        self.refresh_offline()?;
        let action = match self.action {
            JournalAction::Apply => CommitAction::Apply,
            JournalAction::Revert => CommitAction::Revert,
        };
        let seq = self.journal.take_seq();
        self.journal.append_commit(CommitRecord {
            seq,
            tx_id: self.tx_id,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.clone(),
            action,
        })?;
        // The change has committed; side effects only help it take hold. Each
        // outcome is journalled, but neither a failed effect nor a failed
        // journal line for it may turn a committed change into a reported
        // failure (the caller would then retry a change that is already made).
        for effect in std::mem::take(&mut self.effects) {
            let error = self.resolver.system().run(&effect).err().map(|e| e.to_string());
            let seq = self.journal.take_seq();
            let _ = self.journal.append_effect(EffectRecord {
                seq,
                tx_id: self.tx_id,
                unix_ms: now_ms(),
                tweak_id: self.tweak_id.clone(),
                effect,
                error,
            });
        }
        Ok(self.written)
    }

    /// Undo this transaction's own writes, newest first, straight from their
    /// journal records, then cancel them in the journal. If any undo fails the
    /// cancel is not written: the writes stay outstanding so a revert can retry.
    pub fn rollback(self) -> Result<()> {
        let mut first_err = None;
        // Newest first across registry writes and other changes.
        let mut seqs: Vec<(u64, bool, usize)> = self
            .written
            .iter()
            .enumerate()
            .map(|(i, e)| (e.seq, true, i))
            .chain(self.changes.iter().enumerate().map(|(i, c)| (c.seq, false, i)))
            .collect();
        seqs.sort_by_key(|(seq, _, _)| std::cmp::Reverse(*seq));
        for (_, is_reg, i) in seqs {
            let undone = if is_reg {
                self.undo_entry(&self.written[i])
            } else {
                let c = &self.changes[i];
                self.put_back(&c.item, &c.previous)
            };
            if let Err(err) = undone {
                first_err.get_or_insert(err);
            }
        }
        if let Some(err) = first_err {
            return Err(err);
        }
        let seq = self.journal.take_seq();
        self.journal.append_commit(CommitRecord {
            seq,
            tx_id: self.tx_id,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.clone(),
            action: CommitAction::Rollback,
        })
    }

    // ---- internals --------------------------------------------------------

    /// Rewrite `offline\` to what "Undo all" would restore after this commit:
    /// every tweak's outstanding applies, except this tweak's when this is a
    /// revert (its commit closes them all). A rollback needs no refresh: it
    /// only runs before its apply's commit, which is what would have added it.
    fn refresh_offline(&self) -> Result<()> {
        let closing = (self.action == JournalAction::Revert).then_some(self.tweak_id.as_str());
        refresh_offline_set(self.resolver, self.journal, closing)
    }

    /// A per-user change is undone in the hive it was made in. The current
    /// interactive user may be someone else now (a different sign-in, alternate
    /// admin credentials), and writing the old values into their hive would
    /// corrupt it while leaving the original untouched. So it is refused, before
    /// anything is written, and the message says whose account it was.
    fn check_same_account(&self, e: &JournalEntry) -> Result<()> {
        if e.root != RegRoot::InteractiveUser {
            return Ok(());
        }
        let now = self.resolver.display_path(e.root, "");
        let same = e
            .display_path
            .get(..now.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(&now));
        if same {
            return Ok(());
        }
        let made_for = e
            .display_path
            .strip_prefix("HKEY_USERS\\")
            .and_then(|rest| rest.split('\\').next())
            .unwrap_or("an unknown account");
        Err(EngineError::ContextViolation {
            tweak_id: self.tweak_id.clone(),
            detail: format!(
                "this change was made for the Windows account {made_for}, but the current user is {}; \
                 sign in as that account to undo it",
                self.resolver.user().sid
            ),
        })
    }

    fn undo_entry(&self, e: &JournalEntry) -> Result<()> {
        let (hive, full) = self.resolver.route(e.root, &e.key_path);
        let backend = self.resolver.backend();
        match &e.previous {
            Some(prev) => backend.write_value(hive, &full, &e.value_name, prev)?,
            None => {
                backend.delete_value(hive, &full, &e.value_name)?;
            }
        }
        self.remove_created_keys(e.root, &e.created_keys)
    }

    /// Back up, journal (durably), and remember one change.
    fn record(
        &mut self,
        root: RegRoot,
        key: &str,
        name: &str,
        previous: Option<RawValue>,
        written: Option<RawValue>,
        created_keys: Vec<String>,
    ) -> Result<JournalEntry> {
        let display_path = self.resolver.display_path(root, key);
        let seq = self.journal.take_seq();
        let stem = format!("{}_{}_{}", seq, sanitise(&self.tweak_id), sanitise(name));
        let backup = write_reg_backup(&self.session_dir, &stem, &display_path, name, previous.as_ref())?;
        let backup_file = backup
            .strip_prefix(self.journal.root())
            .unwrap_or(&backup)
            .to_string_lossy()
            .replace('\\', "/");

        let entry = JournalEntry {
            seq,
            tx_id: self.tx_id,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.clone(),
            action: self.action,
            context: self.context,
            root,
            key_path: key.to_string(),
            display_path,
            value_name: name.to_string(),
            previous,
            written,
            backup_file,
            created_keys,
        };

        // Durable before the registry moves. Ordering is load-bearing.
        self.journal.append_write(entry.clone())?;
        // Remembered before the registry write is attempted: if that write
        // fails, rollback still undoes it (a no-op if nothing changed).
        self.written.push(entry.clone());
        Ok(entry)
    }

    /// Journal (durably) and remember one non-registry change, before it is made.
    fn record_change(&mut self, item: SysItem, previous: SysState, written: SysState) -> Result<()> {
        // `.reg` backups of the registry values behind the item come first,
        // like a registry write's (plan section 12).
        let seq = self.journal.take_seq();
        let mut reg_backups = Vec::new();
        for (key, name) in item.registry_backing() {
            let display_path = self.resolver.display_path(RegRoot::LocalMachine, &key);
            let prev = self.resolver.read_raw(RegRoot::LocalMachine, &key, name)?;
            let stem = format!("{seq}_{}_{}", sanitise(&self.tweak_id), sanitise(name));
            let file = write_reg_backup(&self.session_dir, &stem, &display_path, name, prev.as_ref())?;
            let backup_file = file
                .strip_prefix(self.journal.root())
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            reg_backups.push(RegBackup {
                display_path,
                value_name: name.to_owned(),
                previous: prev,
                backup_file,
            });
        }
        let entry = ChangeEntry {
            reg_backups,
            seq,
            tx_id: self.tx_id,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.clone(),
            action: self.action,
            item,
            previous,
            written,
        };
        self.journal.append_change(entry.clone())?;
        self.changes.push(entry);
        Ok(())
    }

    /// The interface GUID of a per-adapter change (DNS servers, interface
    /// metric) whose adapter Windows no longer lists: there is nothing to put
    /// back, and the write would fail, so Undo could never finish. When the
    /// adapters cannot be listed, the adapter is taken to be there.
    fn adapter_is_gone(&self, item: &SysItem) -> Option<String> {
        let (SysItem::DnsServers { interface } | SysItem::InterfaceMetric { interface, .. }) = item else {
            return None;
        };
        let adapters = self.resolver.system().network_adapters().ok()?;
        (!adapters.iter().any(|a| a.guid.eq_ignore_ascii_case(interface))).then(|| interface.clone())
    }

    /// True when `e` was allowed only through a `*` target and the key the `*`
    /// stood for (an adapter, a device instance) no longer exists. Writing it
    /// back would create a key for hardware that is gone, and under `Enum`
    /// Windows refuses that, so Undo could never finish.
    fn per_pc_key_is_gone(&self, e: &JournalEntry) -> Result<bool> {
        let literal = self
            .allowlist
            .iter()
            .any(|t| t.root == e.root && path_eq(&t.key, &e.key_path));
        if literal {
            return Ok(false);
        }
        let parts = components(&e.key_path);
        for t in self.allowlist.iter().filter(|t| t.root == e.root) {
            let pattern = components(&t.key);
            if let Some(i) = pattern.iter().position(|c| *c == WILDCARD) {
                if pattern_eq(&t.key, &e.key_path) && !self.resolver.key_exists(e.root, &parts[..=i].join("\\"))? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Add a note to the history, durably.
    fn note(&mut self, text: String) -> Result<()> {
        let rec = NoteRecord {
            seq: self.journal.take_seq(),
            tx_id: self.tx_id,
            unix_ms: now_ms(),
            tweak_id: self.tweak_id.clone(),
            text,
        };
        self.journal.append_note(rec)
    }

    /// The target key matches `key`: with one `*` segment, except under
    /// `HKEY_CLASSES_ROOT`, where `*` is a real key (every file type).
    fn target_matches(t: &RegTarget, root: RegRoot, key: &str) -> bool {
        t.root == root
            && if root == RegRoot::ClassesRoot {
                path_eq(&t.key, key)
            } else {
                pattern_eq(&t.key, key)
            }
    }

    /// Keep a copy of a file's content with the backups; the state names it
    /// (relative to the journal directory) and its checksum.
    fn keep_file_copy(&mut self, bytes: &[u8], which: &str) -> Result<SysState> {
        let seq = self.journal.take_seq();
        let name = format!("{seq}_{}_{which}.file", sanitise(&self.tweak_id));
        let path = self.session_dir.join(name);
        crate::fsutil::create_dir_durable(&self.session_dir)?;
        crate::fsutil::write_durable(&path, bytes)?;
        let backup = path
            .strip_prefix(self.journal.root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        Ok(SysState::File {
            backup,
            sha256: hex_sha256(bytes),
        })
    }

    /// The tweak's declared non-registry items.
    fn check_sys_allowed(&self, item: &SysItem) -> Result<()> {
        if self.sys_allowlist.iter().any(|p| item.matches(p)) {
            Ok(())
        } else {
            Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.clone(),
                detail: format!("the {} is not in this tweak's declared targets", item.describe()),
            })
        }
    }

    /// Keys on the way to `key` that do not exist yet, shallowest first.
    fn missing_ancestors(&self, root: RegRoot, key: &str) -> Result<Vec<String>> {
        let parts = components(key);
        for n in 1..=parts.len() {
            let prefix = parts[..n].join("\\");
            if !self.resolver.key_exists(root, &prefix)? {
                return Ok((n..=parts.len()).map(|m| parts[..m].join("\\")).collect());
            }
        }
        Ok(Vec::new())
    }

    fn remove_created_keys(&self, root: RegRoot, created: &[String]) -> Result<()> {
        for k in created.iter().rev() {
            let (hive, full) = self.resolver.route(root, k);
            self.resolver.backend().delete_key_if_empty(hive, &full)?;
        }
        Ok(())
    }

    /// A user-context tweak must not reach HKLM, and vice versa.
    fn guard_context(&self, root: RegRoot) -> Result<()> {
        if root.required_context() != self.context {
            return Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.clone(),
                detail: format!("declared {:?} but attempted a write to {:?}", self.context, root),
            });
        }
        Ok(())
    }

    /// The tweak's declared blast radius. Case-insensitive, as the registry is.
    fn check_allowed(&self, root: RegRoot, key: &str, name: &str) -> Result<()> {
        self.guard_context(root)?;
        let allowed = self
            .allowlist
            .iter()
            .any(|t| Self::target_matches(t, root, key) && t.values.iter().any(|v| value_name_matches(v, name)));
        if allowed {
            Ok(())
        } else {
            Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.clone(),
                detail: format!(
                    "{}\\{name} is not in this tweak's declared registry targets",
                    self.resolver.display_path(root, key)
                ),
            })
        }
    }

    /// Only keys on the path to something the tweak may write can be removed,
    /// and never a hive root.
    fn check_key_removable(&self, root: RegRoot, key: &str) -> Result<()> {
        self.guard_context(root)?;
        let ok = !components(key).is_empty()
            && self.allowlist.iter().any(|t| {
                t.root == root
                    && if root == RegRoot::ClassesRoot {
                        is_ancestor_or_equal(key, &t.key)
                    } else {
                        pattern_is_ancestor_or_equal(key, &t.key)
                    }
            });
        if ok {
            Ok(())
        } else {
            Err(EngineError::ContextViolation {
                tweak_id: self.tweak_id.clone(),
                detail: format!(
                    "{} is not on the path to any of this tweak's registry targets",
                    self.resolver.display_path(root, key)
                ),
            })
        }
    }

    fn read_current(&self, root: RegRoot, key: &str, name: &str) -> Result<Option<RawValue>> {
        match self.resolver.read_raw(root, key, name)? {
            Some(v) if !v.is_supported_type() => Err(EngineError::UnsupportedValueType {
                path: self.resolver.display_path(root, key),
                value: name.to_owned(),
                vtype: v.vtype,
            }),
            other => Ok(other),
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Rewrite `offline\` (offline.rs) to every tweak's outstanding applies,
/// leaving out `closing`, a tweak whose revert is about to commit. Also run at
/// start-up, which catches up after an apply that a crash cut short.
pub(crate) fn refresh_offline_set(resolver: &ContextResolver, journal: &Journal, closing: Option<&str>) -> Result<()> {
    let outstanding: Vec<(String, Vec<JournalEntry>)> = journal
        .applied_tweaks_newest_first()
        .into_iter()
        .filter(|id| Some(id.as_str()) != closing)
        .map(|id| {
            let writes = journal.outstanding(&id).to_vec();
            (id, writes)
        })
        .collect();
    let mut facts = offline::Facts {
        control_set: resolver
            .read_dword(RegRoot::LocalMachine, r"SYSTEM\Select", "Current")
            .ok()
            .flatten(),
        system_drive: std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()),
        profiles: Default::default(),
    };
    for e in outstanding.iter().flat_map(|(_, w)| w) {
        if let Some(sid) = e
            .display_path
            .strip_prefix("HKEY_USERS\\")
            .and_then(|r| r.split('\\').next())
        {
            if !facts.profiles.contains_key(sid) {
                let key = format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}");
                let image = resolver
                    .read_string(RegRoot::LocalMachine, &key, "ProfileImagePath")
                    .ok()
                    .flatten();
                facts.profiles.insert(sid.to_owned(), image);
            }
        }
    }
    offline::refresh(journal.root(), &offline::plan(&outstanding, &facts))
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Registry value names allow characters that filenames do not. Also capped so
/// a long name cannot push the path past MAX_PATH.
fn sanitise(s: &str) -> String {
    s.chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_replaces_path_characters_and_caps_length() {
        assert_eq!(sanitise(r"a\b/c:d"), "a_b_c_d");
        assert_eq!(sanitise(&"x".repeat(100)).len(), 48);
    }
}
