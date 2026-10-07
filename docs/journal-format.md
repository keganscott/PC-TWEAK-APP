# Journal format

What PeakTweaks records about every registry change, and how undo is derived from it. Source of truth: `crates/engine/src/journal.rs`, `transaction.rs`, `reg_export.rs`.

## Where

`C:\ProgramData\PeakTweaks\` (owned by SYSTEM, writable only by SYSTEM and Administrators; `secure_dir.rs`):

- `journal.jsonl`: the journal, one JSON record per line.
- `backups\<YYYY-MM-DD>\<seq>_<tweak>_<value>.reg`: one `.reg` per value, written before that value changes.
- `backups\<YYYY-MM-DD>\session_<tx>_<tweak>.reg`: one `.reg` per applied change, restoring every value it changed, newest write first. Written before the change's commit record.
- `journal.jsonl.torn-<unix ms>`: what was cut off a torn final line (see Damage).

All `.reg` files are UTF-16LE with a BOM and the `Windows Registry Editor Version 5.00` header. User-hive paths are always written as `HKEY_USERS\<sid>\...`, so a file restores the right profile whoever imports it.

## Records

Every line is an object with a `record` tag and a `seq`. `seq` increases by one for every record and every transaction id, and is never reused, including across restarts.

### `write`: one registry change

| Field | Meaning |
|---|---|
| `seq` | This record's number. |
| `txId` | The transaction it belongs to: a `seq` reserved when the transaction began. |
| `unixMs` | When. |
| `tweakId` | The tweak that made it. |
| `action` | `apply` or `revert`: what the transaction was doing. |
| `context` | The tweak's execution context. |
| `root` | `local_machine`, `classes_root` or `interactive_user`. |
| `keyPath` | Key under `root`, as the tweak declared it. |
| `displayPath` | Full path, with `HKEY_USERS\<sid>` for the user hive. What the `.reg` files use. |
| `valueName` | The value. |
| `previous` | `{ vtype, bytes }` before the change, or `null`: the value did not exist, and restoring it means deleting it. |
| `written` | What was written, or `null` for a deletion. |
| `backupFile` | The per-value `.reg`, relative to the journal directory. |
| `createdKeys` | Keys this write had to create, shallowest first. Undo removes them again if they are empty. Omitted when none. |

Values are stored as their raw registry type number and bytes (comma-separated hex), so every type round-trips exactly.

### `commit`: a transaction ended

| Field | Meaning |
|---|---|
| `seq`, `unixMs`, `tweakId` | As above. |
| `txId` | The transaction it closes. |
| `action` | `apply`: the change finished. `revert`: an undo finished. `rollback`: an apply failed and its own writes were undone. |

### `restore_point`: a restore point PeakTweaks created and verified

`seq`, `unixMs`, `sequenceNumber` (Windows' own number for the point), `description`, `method` (`api` or `power_shell`), `protectionEnabledByUs` (`true` it was off and PeakTweaks turned it on, `false` it was already on, `null` could not be told; older journals hold only `true`/`false`).

### `action`: a one-time action from Tools ran

Written after the action ran. It changes no setting, so it has nothing to undo and is not part of any transaction; it is kept so the history says what was done.

| Field | Meaning |
|---|---|
| `seq`, `unixMs` | As above. |
| `action` | `purge_standby` (emptied the standby list), `cleanup` (deleted junk files) or `optimize_drive` (Windows' drive optimization). |
| `done` | What it did, or `null` when it failed. An object with the same `action` tag and: for `purge_standby`, `cachedBefore` and `cachedAfter` (bytes of files Windows kept in memory); for `cleanup`, `areas` (`user_temp`, `windows_temp`, `thumbnails`, `shader_caches`, `crash_dumps`), `removedBytes`, `removedFiles` and `leftFiles` (files left because a program had them open or Windows refused); for `optimize_drive`, `drive` (`C:`) and `seconds`. |
| `error` | Why it failed, or `null`. |

## Order of operations

For each value a transaction changes: read the current value, write its `.reg`, append the `write` record and `fsync` it, and only then change the registry. A power cut at any point leaves a journal that describes at least everything that changed.

A transaction ends with one `commit` record. An apply writes its `session_*.reg` just before that.

## What undo replays

Per tweak, the journal is read in order:

- An `apply` write is **outstanding** from the moment it is recorded, committed or not. So a crash in the middle of an apply still leaves those writes revertible.
- A `revert` commit with `txId = T` closes every outstanding write with `seq < T`: all applies made before that revert began.
- A `rollback` commit closes only its own transaction's writes.

A tweak is **applied** while it has outstanding writes. Revert restores the outstanding writes' `previous` values, newest first. When several applies stacked on the same value, the oldest one's `previous` wins. A revert whose values are already back in place writes nothing but still commits, which closes the apply.

Undo all reverts every tweak with outstanding writes, the most recently applied first.

Revert refuses a user-hive write recorded for a different account than the one PeakTweaks now resolves (`ContextViolation`), and any write outside the tweak's declared `touches()` targets.

## Damage

- A final line without a newline (a crash mid-append) is cut off when the journal is opened, and saved to `journal.jsonl.torn-<unix ms>`.
- A complete line that is not a valid record is skipped and reported as a `JournalWarning` (shown in Backups). Later records still count.
- Every append starts on a fresh line.

## Recovering by hand

If PeakTweaks cannot run but Windows starts in Safe Mode, import the change's `session_<tx>_<tweak>.reg` (double-click it, or `reg import <file>`), newest change first. Do not import those from the Windows Recovery Environment or WinPE: there `HKLM\SYSTEM` and `HKEY_USERS` are the recovery system's own, so the import changes it rather than the broken install.

For an installation that will not start at all, use `offline\` (`offline.rs`):

- `offline\NNN_<tweak>.reg`: one file per change that is still applied, numbered in the order Undo all uses (most recent first). Paths point at the installation's hive files loaded under temporary names: `HKEY_LOCAL_MACHINE\PT_OFFLINE_SYSTEM\ControlSet00<n>` for `SYSTEM\CurrentControlSet` (`<n>` read from `SYSTEM\Select\Current` when the change was made), `PT_OFFLINE_SYSTEM` / `PT_OFFLINE_SOFTWARE` for the rest of those hives, `PT_OFFLINE_<sid>` for a user's `NTUSER.DAT`.
- `offline\users.txt`: `<sid>|<profile folder without the drive>` for each user hive the files need.
- `offline\recover.cmd`: loads `Windows\System32\config\SYSTEM` and `SOFTWARE` and each user's `NTUSER.DAT` from its own drive (or the folder given as its argument), imports the files in name order, and unloads every hive it loaded, also after a failure. It refuses a running Windows (the hives are in use) and a drive with no Windows on it.
- `offline\README.txt`: the steps, and any change the script does not cover, with why.

The set is rewritten inside every apply and revert, before its commit record, so it always matches what Undo all would restore once that commit is written. Not covered: keys PeakTweaks created are left in place, empty (a `.reg` file can only delete a key with everything under it); changes in other hives (only `SYSTEM`, `SOFTWARE` and users' own settings are loaded); a profile folder on another drive than Windows, or with non-ASCII characters, `!` or `^` in its path (`cmd` reads `users.txt` in an unknown code page, and the script's delayed expansion eats `!` and `^`). After an apply interrupted by a crash, the set catches up when PeakTweaks next starts; if that refresh fails, Backups says so until the next apply or Undo rewrites it. After a recovery, PeakTweaks still lists the changes as applied; Undo there finishes the record.

One known difference: `reg.exe` adds a NUL terminator to a string value that was stored without one (NOTES-closed C22). In-app undo restores the exact bytes.
