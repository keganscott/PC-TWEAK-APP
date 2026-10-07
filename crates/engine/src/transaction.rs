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
use super::journal::{now_ms, CommitAction, CommitRecord, Journal, JournalAction, JournalEntry};
use super::offline;
use super::reg_export::{write_reg_backup, write_session_backup};
use super::registry::{components, pattern_eq, pattern_is_ancestor_or_equal};
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
        })
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
        let mut entries: Vec<JournalEntry> = self.journal.outstanding(&self.tweak_id).to_vec();
        if entries.is_empty() {
            return Err(EngineError::NoJournalEntry {
                tweak_id: self.tweak_id.clone(),
            });
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.seq));

        for e in &entries {
            self.check_same_account(e)?;
            self.check_allowed(e.root, &e.key_path, &e.value_name)?;
            for k in &e.created_keys {
                self.check_key_removable(e.root, k)?;
            }
        }

        for e in entries {
            match e.previous.clone() {
                Some(prev) => self.set_raw(e.root, &e.key_path, &e.value_name, prev)?,
                None => self.delete_value(e.root, &e.key_path, &e.value_name)?,
            }
            self.remove_created_keys(e.root, &e.created_keys)?;
        }
        Ok(())
    }

    /// Close the transaction successfully.
    pub fn commit(self) -> Result<Vec<JournalEntry>> {
        // One combined restore file per applied change, for recovery by hand
        // when PeakTweaks cannot run (Safe Mode; see journal.rs). Written
        // before the commit so a committed apply always has one.
        if self.action == JournalAction::Apply && !self.written.is_empty() {
            let values: Vec<(&str, &str, Option<&RawValue>)> = self
                .written
                .iter()
                .map(|e| (e.display_path.as_str(), e.value_name.as_str(), e.previous.as_ref()))
                .collect();
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
        Ok(self.written)
    }

    /// Undo this transaction's own writes, newest first, straight from their
    /// journal records, then cancel them in the journal. If any undo fails the
    /// cancel is not written: the writes stay outstanding so a revert can retry.
    pub fn rollback(self) -> Result<()> {
        let mut first_err = None;
        for e in self.written.iter().rev() {
            if let Err(err) = self.undo_entry(e) {
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
        let allowed = self.allowlist.iter().any(|t| {
            t.root == root && pattern_eq(&t.key, key) && t.values.iter().any(|v| v.eq_ignore_ascii_case(name))
        });
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
            && self
                .allowlist
                .iter()
                .any(|t| t.root == root && pattern_is_ancestor_or_equal(key, &t.key));
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
