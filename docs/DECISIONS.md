# Decisions and status log

Moved out of `docs/PEAKTWEAKS_DEV_PLAN.md` section 15 so the plan stays readable. Numbering is kept (15.1 to 15.13) because commits, notes and code comments refer to it. Newest last. For what is still open, read `docs/NOTES.md`; for how to work, `CLAUDE.md`.

### 15.1 Changes to the plan, and why

Each item is an amendment to the text above. Kegan can veto any of them; none touches Section 1.

1. **Workspace layout (P0).** The engine is its own crate, `crates/engine` (no Tauri dependency), and `src-tauri` is a thin shell over it. Reason: the journal, transaction and fake-registry tests then run on plain Linux and Windows without WebView2/GTK, and `cargo check --target x86_64-pc-windows-msvc` from Linux catches Windows compile errors before CI. The plan's `src-tauri/src/engine/...` paths map to `crates/engine/src/...`.
2. **R4 design, two refinements.** (a) `tx_id` is a sequence number reserved when the transaction *begins*, not "the first write's seq", so a transaction that writes nothing (everything already at target) still has an id and can be committed. (b) The plan's R6 says a failed apply appends "a Commit for a Revert". That would erase the outstanding writes of an *earlier* successful apply of the same tweak. A third commit action, `Rollback`, cancels only its own transaction's writes.
3. **R18: `created_keys: Vec<String>` instead of `created_key: bool`.** The IFEO tweak creates two levels (`<exe>` and `PerfOptions`); a bool cannot say which to remove.
4. **R1: `Tweak::touches()` returns `Vec<RegTarget>` (owned), not `&'static [RegTarget]`.** The IFEO key depends on the game's exe name. For the same reason `Tweak::id()` returns `&str` and `TweakMetadata` text fields are `Cow<'static, str>`.
5. **R1: `restore_journalled()` takes no argument.** It always restores the transaction's own tweak, so one tweak cannot replay another's records. Every outstanding record is validated against the allowlist *before* the first write, so a bad line cannot cause a half-restore followed by an error.
6. **R1: strict startup.** If `%ProgramData%\PeakTweaks` fails verification (owner not SYSTEM/Administrators, a non-admin principal with write access, reparse point, NULL DACL, or the same for `journal.jsonl`) the engine refuses to start; it does not start read-only, and it never repairs a bad directory (planted files could be journal lines). `TrustedDir` is a witness type: `Journal::open` cannot be called without one.
7. **R11: ts-rs chosen** (stable 12.x; specta is still a release candidate). The "fixtures round-trip" test is done without a JS test runner: a Rust test writes `src/generated/fixtures.ts`, real serialized values wrapped in `satisfies <GeneratedType>`, and `tsc` fails on any drift. CI also fails if `src/generated` is stale. The `Progress` event type lives in the engine crate so all bindings come from one place. Journal records are now camelCase on disk as well as over IPC (nothing had shipped).
8. **MSRV.** `Cargo.toml` said `rust-version = "1.77"`; Tauri 2.12's dependencies require 1.90. Set to 1.90.
9. **R15.** `Win32PrioritySeparation` tweak and its "We measured no difference" message are deleted. Replaced by `priority.ifeo.<game>` (IFEO `CpuPriorityClass` = 3). It is blocked for every game (`CLEARED_GAMES` is empty) because the plan says not to enable it for any title until tested per anti-cheat vendor.
10. **R12 / production defaults.** Until Phase 3 (restore points) and Phase 7 (licensing) exist, a normal build has a closed restore gate and the Free tier, so **nothing can be applied in a production build**. That is deliberate. To drive the engine by hand use `--features dev-stubs` (never enabled in CI or release). `BlockedCode::TierRequired` was added.
11. **Known games list** contains only Fortnite, Minecraft and Roblox (the titles named in the plan's game cards). Which other anti-cheat titles to support is still Kegan's decision (Section 11, item 5).
12. **R14** sets `WEBVIEW2_USER_DATA_FOLDER` only when the process identity differs from the interactive user (the Administrator Protection / alternate-credentials case), and never overrides a value already set.
13. **R17 detail.** `.reg` export no longer writes wrong data for malformed values: a malformed REG_DWORD or a REG_SZ that does not round-trip exactly is exported as typed hex, not `0` or an empty string.
14. **`revert_all` order** is by most recent outstanding write, descending, taken from the in-memory journal index.
15. **Revert is never gated** by tier, predicates or the restore gate. Getting back to how things were must always be possible.

### 15.2 Things in the original code the plan did not list

- `sid_from_token` read a `TOKEN_USER` from a `Vec<u8>` (not 8-byte aligned) and leaked the SID string if UTF-16 conversion failed. Fixed.
- `Journal::is_applied` and `entries_for` re-read and re-parsed the file per call (R9), and `next_seq` used `.last()` rather than the maximum.
- The mouse tweak treated a missing `MouseSpeed` as "0" (off), so a fresh profile could read as Applied. A missing value now counts as Windows defaults (acceleration on).
- `main.rs` swallowed a poisoned engine lock message into `UserContextUnresolved`; it is now `EngineError::Internal`.

### 15.3 Status against the Phase 2.1 gate

Evidence is GitHub Actions run 36621445888 (commit `73159c5`, `windows-latest`, Rust 1.98.1 pinned): fmt, `clippy --all-targets -D warnings`, check, test (97 engine tests + 5 IPC audit tests), generated-bindings freshness, `npm ci`, `tsc --noEmit`, `vite build`, `tauri build --no-bundle`, manifest scan (`requireAdministrator` x1, Common Controls x1), launch smoke test. The `ubuntu-latest` job runs the same engine tests plus the freshness check.

| Item | State |
|---|---|
| P0 compile fix (`LocalFree`) | Done. Checked against `windows-0.58.0` source and on Windows CI. |
| P0 scaffold, capability, manifest, CI | Done and green. Command list is enforced at build time (app manifest) and by a test. |
| P0 `RegistryBackend` + fake | Done. |
| R1 ProgramData DACL, verify on start, allowlist | Done. 4 ACL tests run on real NTFS in CI (create, ordinary dir refused, Users-write detected, junction refused). The exe was launched in CI and its directory ACL is SYSTEM + Administrators only. **Not tested:** a *non-admin* user pre-creating the directory (owner check), because CI runs as admin. |
| R2, R12 engine-owned env, gate, tier | Done. `set_environment` deleted; audit test stops it coming back. |
| R3, R4, R6, R8, R9, R17, R18, R19 | Done. Includes a property test (300 cases on Linux, 60 on Windows) and a mutation check that breaks R4 and fails 4 tests. |
| R7 mouse tweak | Done except the `SPI_SETMOUSE` array order, still marked VERIFY: Microsoft docs are unreachable from the build sandbox. |
| R10 async commands, progress events | Done. Compiles and passes the audit; **not exercised from a running webview.** |
| R11 one serde contract | Done via ts-rs + `fixtures.ts` under `tsc`. Frontend `types.ts` duplicates do not exist in the repo yet, so none were deleted. |
| R13 session-anchored user resolution | Code done. The manual matrix (Windows 10 22H2, 11 24H2/25H2, RDP, alternate credentials, Administrator Protection) is **not done**. |
| R14 WebView2 data folder | Code done, pure parts tested. **Not tested under Administrator Protection.** |
| R15 reference tweak | Done. IFEO `CpuPriorityClass`=3, blocked for every game. |
| R16, R22 | Done (stale comment removed; `winreg` 0.52 / `windows` 0.58 pinned). |
| R20, R21 frontend | **Not started.** The React frontend files are not in the repository. |
| UAC prompt on launch | **Not tested.** GitHub runners are already elevated with UAC off. |

Open VERIFY markers left in place: `SPI_SETMOUSE` order (`mouse_accel.rs`), Fortnite executable name (`ifeo_priority.rs`), and the plan's claim that `Win32PrioritySeparation` 0x26 equals the client default (the tweak is deleted; the claim was not independently checked).

### 15.4 Phase 3 design decisions (Claude)

1. **Everything OS-specific sits behind a trait**, so the logic runs in unit tests on any OS: `WmiSource` (queries), `OsFacts` (system drive, display modes, DXGI adapters), `RestoreOps` (enable protection, create point, list points), plus the existing `RegistryBackend`. Real implementations are Windows-only; fakes are used in tests. This matches the plan's tri-state rule: every probe returns `Probe<T>` (`yes` / `no` with reason / `unknown` with reason), and a probe that cannot run yields Unknown, not an error and not a guess.
2. **WMI runs on one dedicated MTA thread** (`WmiWorker`) with a per-query timeout and a "stuck worker" guard. A live Windows test runs a query from a thread that is itself in a single-threaded apartment, which is the situation on Tauri's main thread.
3. **GPU memory comes from DXGI**, not `Win32_VideoController.AdapterRAM` (capped at 4 GB) and not the registry. Plan 4.4 allowed either.
4. **`SystemEnv` now carries the structured reports** (`hardware`, `security`, `restore`) instead of loose booleans; predicates ask `env.logical_processors()` and get `None` when unknown. It is still built only by the engine and never deserialized.
5. **The restore gate is derived from Windows' own restore-point list** (freshness window in `restore.rs`), re-read immediately before every apply, cached for 10 s. Creating a point is a three-part flow: (a) slow Windows calls (enable protection, create) run **without** the engine lock, (b) two short journalled steps take the lock, (c) success is claimed only after a *new* sequence number appears in Windows' list. A "success" that produces no new point is reported as a failure.
6. **Bootstrap exemption** for `SystemRestorePointCreationFrequency = 0` (NOTES.md N26): the plan's "through Transaction" and "no change before a verified restore point" cannot both hold for this one setting.
7. **`Engine::new` no longer probes.** Constructing the engine happens on Tauri's main thread and probing takes seconds. The first command that needs an environment probes, in `spawn_blocking`.
8. **New journal record kind** `restore_point` (Backups tab material). It belongs to no tweak, so it is not part of the revert index.
9. **System Restore does not exist on Windows Server**, which is what GitHub's Windows runners are. So the Phase 3 gate item "restore point verified on Windows 10 22H2, Windows 11, and an Administrator Protection VM" **cannot be automated**; CI checks the logic against fakes and that the real code fails cleanly. Manual protocol: NOTES.md N24.
10. **New IPC commands**: `audit_system`, `create_restore_point`; `rescan` now bypasses caches. `apply_tweak` still cannot reach the bootstrap tweak.

### 15.5 Phase 3 status (evidence: CI run 36627022976)

| Item | State |
|---|---|
| 4.1 dependencies | Done. `wmi` 0.14.5 pulls `windows` 0.59 next to our 0.58 (and Tauri's own); duplicates accepted as the plan allows. |
| 4.2 WMI worker | Done and exercised on Windows (see NOTES.md C5). |
| 4.3 security probes | Done. Secure Boot, TPM, Memory Integrity confirmed to return sensible answers on a Server VM; IOMMU always Unknown (N21); HVCI service code unverified (N20). |
| 4.4 hardware probes + rig class | Done. Channel layout, disk media type codes and SMBIOS codes not yet checked on client hardware (N22). |
| 4.5 restore engine | Logic done and tested (21 tests against fakes). Real creation **not yet achieved on any machine** (N24), protection on/off not detectable (N23). |
| 4.6 live `SystemEnv` | Done. |
| 4.7 IPC commands | Done: `audit_system`, `create_restore_point`, `rescan` (uncached). |
| 4.8 frontend wiring | Typed client `src/ipc.ts` done; UI work blocked on the frontend files (N1, N30). |
| **Gate** | **Not met.** Needs the manual restore-point runs (N24) and real-hardware checks (N22). |

### 15.6 Phase 4 design decisions (Claude)

1. **PresentMon is embedded in `peaktweaks.exe`**, not downloaded. The Starter tier gets a proof run yet must contain no networking, and the app must stay one portable binary, so a run-time download is out. `vendor/presentmon/PINNED.json` pins version, SHA-256, size and signer (Intel Corporation); `scripts/fetch-presentmon.ps1` (used by CI and developers) refuses anything else, including an invalid Authenticode signature; at run time the embedded copy is written to the protected `tools` folder and hashed again before every capture. MIT license text ships in `vendor/presentmon/`.
2. **Verified against upstream rather than memory:** the flags and CSV columns the plan marked VERIFY exist in PresentMon 2.6.0 (see NOTES.md C9).
3. **A "proof" is a session**: several "before" runs and several "after" runs of the same scene, all stored with their CSV. A difference is called Better or Worse only when it strictly exceeds the run-to-run spread (max minus min of the repeats) and at least 1% of the baseline; each side needs at least 2 runs; a change that hurts either average or 1% low is never called an improvement. The wording of the result is produced in one function (`proof/verdict.rs`) and a test fails if verdict wording appears anywhere else.
4. **Runs record what they measured**: tweaks applied at capture time, PresentMon version, rig class, and (when an NVIDIA GPU is present) the GPU throttle reasons seen while capturing, sampled at the start, about once a second, and at the end. A run held back by heat or power adds a warning to the comparison.
5. **Efficacy claims are linted in Rust**: every string literal in the tweak catalogue is scanned for claim words (boost, faster, FPS, smoother, measured, ...). The same check for frontend copy belongs with the frontend (NOTES.md N38).
6. **Two bugs found and fixed on the way**: killing a timed-out child process no longer waits for the output pipes a grandchild may still hold (this also affected the Phase 3 PowerShell runner), and the capture command line is built only from validated values (game names that could be read as options or paths are refused).


### 15.7 Phase 5 design decisions (Claude) — first slice only

1. **The scanner is a pure function of `SystemEnv`** (`crates/engine/src/scanner.rs`), returned inside `SystemAudit.scan`. It reads nothing itself, so it needs no new IPC command and no new frontend-supplied input, and it is unit-tested on Linux from hand-built machines.
2. **Every finding is `guidedOnly` with `fixTweakId: null`** because none of the one-click fixes in plan 6.2 exist as tweaks yet. A finding never names a fix that does not exist.
3. **Three states, not two**: a finding is Attention, Fine ("what is already right", plan 6.2) or Unknown with the reason. A probe that could not tell is never shown as fine or as a problem.
4. **Built (6 checks):** single memory channel, memory below rated speed, refresh rate below the display's maximum, Windows on a hard drive, Windows 10 or older, Memory Integrity state (reading only; the scanner never suggests turning it off). Not built, with reasons: NOTES.md N42.
5. **Ordering** is "Attention, Unknown, Fine" and then rule order. The plan says "order by expected gain"; gain needs proof runs, so this is a stated judgement (NOTES.md N41), not a measurement.
6. **Copy is linted**: the scanner's string literals are scanned for the same claim words as the tweak catalogue.
7. **Gate: not met.** "Scanner findings match hand-verified results on at least three real machines" is manual. The other half of the gate (no networking dependency in the Starter build) is enforced by `scripts/check-no-network-deps.sh` in CI (NOTES.md C11).
8. **Power plan added (check 5, read-only)**: `SystemEnv.powerPlan` and scanner finding `power.plan`. Guided only: a plan change is not a registry write the journal can undo, so no fix tweak is offered until Kegan decides how to journal it (NOTES.md N43).
9. **Settings** (`settings.rs`, IPC `get_settings` / `set_settings`): a rig-class override (plan 6.1) and plain/technical wording (section 7), stored as `settings.json` in the protected directory, written by temp file + rename. They are preferences only: the engine uses the override for the rig class it records with proof runs and reports as `effectiveRigClass`; no predicate, gate or licence reads them. A corrupt file gives defaults. The command takes only the `Settings` struct, so the IPC audit still holds.

### 15.8 Adversarial code review of the branch (Claude, `code-review high`)

Ten findings; all checked against the code. Fixed with tests: revert of a per-user change under a different account is refused before any write (`check_same_account`); the live mouse push no longer passes `SPIF_UPDATEINIFILE` (it wrote registry values outside the journal); an unreadable Device Guard list is Unknown, not "Memory Integrity off"; a failed revert still clears cached probe results; `apply` re-reads state probes; `detect_user` no longer hides an unreadable shell (NOTES.md N44); the two copies of the timeout runner are one (`proc.rs`); calendar helpers live together in `timeutil.rs`; the release profile unwinds instead of aborting. Recorded, not changed: NOTES.md N23 (the protection flag is always false on real machines) and N45.

### 15.9 Field-check tool (Claude)

Every remaining gate (Phase 3: restore points and hardware readings; Phase 4: variance on real runs; Phase 5: scans that match three real machines) needs evidence from a real PC, and the manual steps were spread across a dozen notes. `crates/field-check` builds `peaktweaks-field-check.exe`: it starts the real engine exactly as the app does (no dev stubs) and writes one `report.json` with the live audit and scan, `powercfg` names for the power-plan GUIDs, raw WMI rows for memory, disks and GPUs, System Restore registry values and shadow storage, NVML throttle samples and, on request, a real PresentMon capture (which also produces the parser fixture), a real restore point, and revert-all. It is read-only unless `--create-restore-point` or `--revert-all` is given and confirmed by typing yes. CI builds it, runs it read-only on the runner and publishes it as the `peaktweaks-field-check` artifact. Instructions and what each section settles: `docs/FIELD_CHECK.md`. It is a development tool and is not part of the shipped app.

### 15.10 One definition of "copy that promises a result" (Claude)

Plan 0.4 forbids efficacy claims in UI copy. The Rust lints had their own word lists; the UI had none. Now `scripts/claim-words.json` is the single list, read by the Rust copy lints (`crates/engine/src/copy_lint.rs`, used for the tweak catalogue and the scanner) and by `scripts/check-ui-copy.mjs`, which parses every `.ts`/`.tsx` file under `src/` with the TypeScript compiler and checks exactly what a user can see: string literals, template text, JSX text and shown attributes such as `aria-label`, but not comments, imports, types, object keys or `className`. Words that start with a letter match at word starts ("lag" catches "laggy", not "flag"). Text bound to a stored proof run may opt out with `// copy-lint-allow: <reason>`; an opt-out without a reason does not count. Both lints have tests, and CI runs the UI lint. Kegan chose to upload the existing frontend (not have a new one built); `docs/FRONTEND_INTEGRATION.md` lists where it goes and the wiring steps.

### 15.11 Frontend (Claude)

Built to section 7 because the original files never arrived (NOTES.md N1). React 19, TypeScript (strict, `noUncheckedIndexedAccess`), Tailwind 4, lucide-react, Vite 8.

- **One seam to the engine** (`src/services/backend.ts`): inside Tauri it is the typed IPC client (`src/ipc.ts`, unchanged); in a browser it is the SAMPLE mock. The mock is built from the engine's own serialized fixtures and keeps the engine's rules (no apply without a restore point, blocked stays blocked, revert needs a journal entry, errors arrive as `EngineFault`). It exists only in `vite dev` and the end-to-end test build (`--mode e2e`); a release build contains none of it and refuses to run outside the app window, and CI checks the release bundle for sample text.
- **Store** (`src/store/store.ts`): one external store read through `useSyncExternalStore`; immutable slices so selectors return stable references; every overlapping request tagged so a late reply is dropped; every action records success or the engine's error (R20). No UI code decides what the engine allows.
- **Screens**: Home (one-sentence state, restore lock as an inline step with live progress, result card with links to Backups and Proof, scan findings with "what is already right"), Games (target game, firmware security features read-only, per-game requirements with their source), Tools (tweaks by category, Advanced switch for non-safe tiers, trade-off must be acknowledged, registry targets only in technical wording), Proof (sessions, recording within the engine's limits, run tables, the engine's headline verbatim), Backups (applied changes, restore points, change record, Undo all behind a confirmation), Settings (wording, rig-class override), and the Execution Bus (closed by default).
- **Accessibility**: status always icon + word; dialogs trap focus, close on Escape and return focus; labelled controls; visible focus rings; reduced motion respected; tested at 1366x768 and at its 150%-scaling size with axe (WCAG 2.2 AA tags).
- **Copy**: all of it passes `npm run lint:copy`. Column labels for stored run numbers ("Average FPS") use the documented opt-out with a reason.
- **Dev CSP**: `devCsp` allows inline styles and the Vite websocket for `npm run tauri dev` only; the release CSP is unchanged.

### 15.12 Review of the frontend (Claude, `code-review high`)

Ten findings, all checked against the code and all fixed, each with a regression test (three of them mutation-checked: the test fails with the fix removed):
1. After a restore point was made, the "Step 1" card came back with its button enabled until the slower audit re-read agreed; a second click could make a duplicate point. Success now shows on the engine's confirmation.
2. Undo on Backups ignored its own result: no busy state, and a failure was invisible. Each row now shows both.
3. A target-game pick wrote the tweak list without taking the list's request tag, so an older list still in flight could overwrite it.
4. Two overlapping boots (React StrictMode in development) subscribed to progress twice, doubling every activity-log entry.
5. A failed re-read after a change was dropped. It is now recorded and shown as "some of what is shown may be out of date", with Check again.
6. Tools treated "audit not loaded yet" or "audit failed" as "no restore point" and locked every change. Only a definite "no" locks it now; the engine still checks at apply time.
7. A new run left the previous verdict on screen beside runs it did not cover. A capture now clears that comparison.
8. Capture state was global, so one comparison's error or recording state showed on another. It is per comparison now.
9. An audit that started before a target-game pick could land after it and undo the pick on screen.
10. The restore step's progress line could show the previous attempt's last message. It now reads only the current attempt's messages, and without copying the whole log on every event.

### 15.13 The real app, end to end in CI (Claude)

The Windows job now drives the real `peaktweaks.exe` UI with WebDriver after the shipped exe is uploaded: same code and engine, rebuilt with one change, a WebView2 debugging port in the window's browser arguments (`e2e-tauri/tauri.e2e.conf.json`), because the WebView2 runtime ignores the environment variable msedgedriver normally uses (evidence in NOTES.md C18). `e2e-tauri/real-app.mjs` checks that the engine starts, no SAMPLE data appears, the scan arrives, the restore lock matches the machine, Tools lists the real catalogue, Games/Proof/Backups open and the activity log carries the engine's messages, and prints what each screen shows. Found on the way and fixed: when the engine failed to start, the windowed release build used to vanish with no message; it now stays open, shows the reason on its start-up screen and writes it to `%LOCALAPPDATA%\PeakTweaks\startup-error.log`, and the launch smoke test fails if that log is written.

### 15.14 The registry write path, tested on the real registry (Claude)

Every engine test ran on the in-memory fake, and `WinRegistry`, the only code that changes the user's registry, had no test on Windows. `crates/engine/src/registry/contract_tests.rs` now holds one `RegistryBackend` contract that runs on the fake everywhere and on the real registry on Windows CI (step "Real registry write path on this runner"), plus two Windows-only tests: the engine applies and reverts on a scratch key under HKCU and must leave the tree identical, and the `.reg` backups are imported with `reg.exe` and must restore the prior values. Measured result (NOTES-closed C20): exact, except that `reg.exe` adds a NUL terminator to a REG_SZ or REG_EXPAND_SZ value that was stored without one. That cannot be fixed in the backup format, is harmless to programs reading the value as a string, and does not affect in-app Undo, which restores from the journal; the test accepts exactly that difference and nothing else. The fake now refuses unsupported value types as the real registry does.

### 15.15 The IPC boundary, tested in the real app (Claude)

`e2e-tauri/real-app.mjs` now calls commands from inside the running page, as injected script could, and requires the ACL to refuse everything outside the capability and the engine to ignore forged licence, tier, gate and environment fields (evidence NOTES-closed C21). To have a registered command to see refused, the WebDriver test build's capability is the shipped one minus `allow-revert-all` (`e2e-tauri/tauri.e2e.conf.json`); so the test build now differs from the shipped exe in two ways, the debugging port and that one permission, and `command_audit.rs` fails if the copy drifts in any other way.

### 15.16 Registry string writes go through our own `RegSetValueExW` call (Claude)

`RegSetValueExW` looks past the end of the data it is given for string types and stores an extra NUL when the next two bytes happen to be zero (NOTES-closed C22). `WinRegistry::write_value` therefore no longer uses `winreg`'s `set_raw_value`; it passes the data from its own buffer with two non-zero guard bytes after it, which makes the stored bytes exactly the requested ones. This is measured Windows behaviour, not documented API, so the contract test keeps rewriting unterminated strings 50 times on every Windows CI run and fails if any write is not exact.

### 15.17 The agent brief (independent review of commit 85d3bb2), item by item (Claude)

Kegan shared `PEAKTWEAKS_AGENT_BRIEF.md` on 2026-10-02: a review of `85d3bb2`, his original 10-file upload (commit 2 of 59). Most of it had already been fixed by the plan's R1-R19 hardening; each item was re-checked against the current code.

| Brief item | Status | Evidence |
|---|---|---|
| Phase 1: layout, catalogue(), build.rs, tauri.conf, icons, capabilities, admin manifest, Cargo.lock, .gitignore, CI | Done earlier | `99d8204` onward; capability lists every command (18, not the brief's 7: the app grew); CSP strict (C19); CI green |
| Bug 1 torn journal line | Done earlier | `journal.rs` tests `torn_final_line_is_cut_off_saved_and_next_append_is_readable`, `seq_stays_monotonic_across_restarts...` |
| Bug 2 mouse writes outside the journal | Done earlier | no `SPIF_UPDATEINIFILE`; revert pushes the restored values; skipped when `!is_self` (`mouse_accel.rs`) |
| Bug 3 revert replays every apply | Done earlier | test `revert_after_an_external_change_lands_on_the_value_before_the_latest_apply` (the brief's exact scenario) |
| Bug 4 no-op revert leaves "applied" | Was already right; test added | `a_revert_with_nothing_to_write_still_ends_the_apply` (passes on the old code: commit records close the apply) |
| Bug 5 failed apply not rolled back | Done earlier | `a_failed_apply_rolls_back_its_own_writes_and_returns_the_original_error` |
| Bug 6 gate trusts the frontend | Done earlier | `set_environment` deleted (audit test), engine-only gate, forged state refused in the real app (C21) |
| Bug 7 Revert All order / orphans | Order done earlier (journal-driven, newest first). Orphans: **open question** (N49) | an unknown tweak id is reported as a failed revert, not skipped; `.reg` files remain |
| Bug 8 unknown state / blocked hides applied | Unknown done earlier; blocked+applied **fixed now** | `an_applied_tweak_that_becomes_blocked_still_shows_applied_and_the_reason`; UI test "an applied change that is now blocked keeps its Undo" |
| Bug 9 false WinPE claim | **Fixed now**: comment rewritten, `session_<tx>_<tweak>.reg` per applied change, `docs/journal-format.md`, offline script logged (N48) | `each_applied_change_gets_one_session_reg_in_restore_order`; Windows: `reg_exe_import_of_the_session_file_alone_restores_the_change` |
| Mouse missing value / off-not-by-us | Missing value was right; off-not-by-us **fixed now** (`Foreign`) | `mouse_state` tests in `tests.rs` (`off_but_not_by_us_is_foreign` failed before the fix) |
| Internal error, journal cache, created keys, async commands | Done earlier | `EngineError::Internal`; in-memory journal index; `created_keys`; `pub async fn` commands |
| Phase 3 restore points, probes, React shell | Built; real restore-point creation unproven on a client PC (N24) | field check covers it |
| Phase 3 priority_separation | Done earlier: the tweak was deleted | N8 |
| Phase 3 remove `Tier::Free` | **Not done: conflicts with the plan** (N50) | plan 6.4 Starter "free forever" |

Not followed: the brief's paths (`src-tauri/src/engine/...`); the engine is its own crate (`crates/engine`) so it builds and tests on Linux.

Follow-up on 15.17 (Kegan said continue): brief Phase 3 items done without needing his decisions. Every `BlockedCode` has its own guidance (`src/lib/blocked.ts`, typed so a new code fails the build). Home names the recent restore point that unlocked changes and when Windows made it. Apply and Undo run end to end in the real app on the real registry (C25). A "comments are claims" audit of the safety modules (transaction, journal, engine, context, identity, restore, secure_dir, WinRegistry, tweaks, sysprobe): every always/never/only/cannot comment checked against the code; the WinPE claim (fixed in be6f425) was the only false one. Not done: tweak impact on cards (N51), `Tier::Free` (N50), orphaned ids (N49).


### 15.18 Offline undo for an install that will not start (Claude)

Agent brief bug 9 left recovery from outside Windows as future work (N48). Choices, and why:

- **Files kept ready, not a script that rewrites at recovery time.** The Windows Recovery Environment has `cmd` and `reg.exe` but no PowerShell, and `cmd` cannot safely rewrite registry paths inside UTF-16 files. So the engine writes `offline\` with paths already remapped to the hive files the script loads (`offline.rs`).
- **The set is what Undo all would restore, rewritten inside every apply and revert, before the commit record**, like the session file. A committed change therefore always has its offline file, and a reverted one never does (re-importing an undone change's old values could overwrite settings changed since). The cost: a failure to write the set fails that apply or revert, which stays outstanding and can be retried, the same as a journal append failure. A rollback does not refresh it: rollbacks run only before their apply's commit.
- **All of a change or none of it.** A change with any value in a hive the script does not load (or a profile on another drive, or with non-ASCII characters, `!` or `^`, which `cmd` cannot pass on) gets no file and is listed in `README.txt`, rather than half an undo.
- **Keys PeakTweaks created are left in place.** A `.reg` file can only delete a key with everything under it.
- **The set lives in the protected data folder**, so a standard user cannot plant a file there that the script would later import with full rights.
- **Order:** files are named `NNN_<tweak>.reg` in Undo-all order and imported by name; inside a file the newest write comes first, so stacked writes end on the oldest prior value, as in-app Undo does.

Tested on Windows CI against hive files made by `reg save` (`recover_cmd_restores_prior_values_in_offline_hive_files`). Never run in the recovery environment itself (N48).

### 15.19 Kegan's blanket approval (2026-10-06)

Kegan, in chat: "if you need anything, it's approved by me. I approve." Taken as approval of the open requests in `docs/AUDIT-2026-10-04.md` section 8, read narrowly:

1. **New tweaks may be added** (the agent brief's Phase 3 hold is lifted). First: the two Starter fixes that fit `Transaction` today (game on the high-performance graphics chip; background recording off).
2. **N50: the free Starter tier stays**, as plan 6.4 says; `Tier::Free` is kept and the Starter fixes are Free.
3. **Restore frequency** stays as the locked decision says (journalled, kept until Undo); no change.
4. **Refresh-rate fix**: to be built later with its own journal record kind, designed here first; not part of the first Starter fixes.

Not covered by an approval, because they need Kegan's own action or knowledge: GitHub Actions billing (N58), the field-check run, the N2 reference documents, and plan section 11 (pricing, merchant of record, Store build, open-sourcing, next anti-cheat titles).

### 15.20 The interface: Kegan's palette, round 3 (2026-10-06)

Kegan chose the round-3 dashboard (`docs/design/mockups-2026-10-06/v3.png`: "That UI looks way way better. Please build out this UI and implement it into our app") and left the details to me ("Make any changes or improvements that you think there should be"). What was built and why:

1. **Colours** (`src/index.css`): black base, near-black cards, hairline borders, no glows. Violet `#7E3BED` is for actions and "worth a look"; lime `#C6FF34` means ready or done (black text on it); white is the type. Red stays, for errors only, and an amber stays for the SAMPLE label (dev and e2e builds only). Every text colour meets 4.5:1 on the surface it sits on; on violet only pure white text is used, because lighter tints fall below 4.5:1. Status is still never colour alone (icon and word in `StatusBadge`).
2. **Logo**: mark A, "Summit" (a violet peak with a lime summit), my pick on the logo sheet; Kegan approved the UI without naming a mark, so it is assumed (N61). It is the sidebar mark, the start-up and failure screens, and the Windows app icon (`src-tauri/icons`, generated with `tauri icon` from `icons/source.png`).
3. **Fonts**: Plus Jakarta Sans (interface) and Exo 2 (wordmark), bundled from npm `@fontsource` packages and served from the app itself, so the no-network rule (C24) and the CSP `font-src 'self'` hold. Both are SIL Open Font License; the licence texts ship in the bundle under `licenses/`.
4. **Home** follows plan section 7 with the mockup's layout: one sentence on the PC's state (the greeting line and the line under it), a violet next-step card where Auto will go, and the last result. Until Auto exists (it needs the N2 reference documents), that card is the restore lock as an inline step, then "Choose the changes for this PC" with Open Tools. Restore point and changes-in-effect cards, hardware tiles with real readings only (memory speed against its rating, refresh rate against the display's maximum; flagged by the scanner's own findings), the scan grouped by who can act, and the games found.
5. **Navigation** is unchanged (Home, Games, Tools, Proof, Backups) in two groups; Settings moved from the top bar to the sidebar. Counts next to a view come from engine data and are drawn by CSS from `data-count`, so a button's name stays the view's name (the tests and the Windows WebDriver test find views by name); screen readers get the count as the button's description.
6. **Left out of the mockup**: the user's first name (the engine does not know it), the plan card "Starter, Free" (the UI never states tier on its own; it shows only what the engine reports) and the custom title bar (Windows' own title bar stays).

### 15.21 A tester build for Kegan's own PC (2026-10-06)

Kegan asked for the app to be ready for him to test on his personal PC. Two things stood in the way. GitHub Actions cannot build (N58). And a release build applies nothing, because both catalogue changes are Pro and the licence is the Free stub until Phase 7 (N15).

The Cargo feature `tester` (`License::tester`) unlocks every plan and changes nothing else: real probes, the real restore gate, the journal, the allowlists. It is not `dev-stubs`, which opens the restore gate and must never reach a real PC; the two refuse to compile together. The tier is still decided in Rust at build time, never by the UI (plan section 12). The engine reports the build in `ContextInfo.testerBuild`, so the label comes from the engine, not the page.

Delivery: CI builds it last and uploads `peaktweaks-tester-exe`, and `scripts/build-tester.ps1` builds it on a Windows PC. The steps and what to send back are in `docs/TEST-ON-YOUR-PC.md`. The feature goes when real licensing lands.


### 15.22 Every Hone and ExitLag feature, network use for the ExitLag ones, fewer warnings (2026-10-06)

Kegan, in chat: "Any single feature that Hone has you can do", and "any single feature that ExitLag has, you can make". The list being built is `docs/CATALOGUE.md` (Hone H1-H31, ExitLag E1-E6).

1. **Evidence.** A feature Hone ships counts as supported evidence for plan section 12's rule "no tweak whose only support is a forum post". Registry paths and values are still checked against Microsoft's documentation or a real PC before a VERIFY marker comes off.
2. **Network.** The app may use the network for the ExitLag-style features: ping tests (Network Analyzer, E4) and traffic priority (QoS / Traffic Shaper, E3 and H26), started by the user. Nothing else goes online, and nothing about the user is sent anywhere. This replaces the old "never uses the network" description (C24, N40, `scripts/check-no-network-deps.sh`), which must be updated to say so honestly when E4 lands.
3. **Fewer warnings.** No warning on well-supported changes; one short line only where there is a real cost (restart needed, battery, connection drops for a few seconds); one confirmation only for Advanced changes. The safety net is unchanged: a verified restore point before the first change, a `.reg` backup and journal record before each one, Undo and Undo all.
4. **Never-do still stands.** Plan section 12's list is unchanged. If a Hone or ExitLag feature needs one of its items (Memory Integrity/VBS off, Secure Boot/TPM/IOMMU, game memory or injection, Roblox fast flags or FPS unlockers, game CPU affinity, BIOS, vulnerable drivers), it is not built; Kegan is asked first.
5. **Not buildable:** E1 (ExitLag's routing over its own server network). It stays listed in the catalogue as such.

### 15.23 While you play: session changes and the game watcher (Claude, 2026-10-08)

Catalogue step 5 (NOTES N80).

1. **Gaming Mode's changes are tweaks, not a side channel.** Each one is an internal tweak (`tweaks::session`) made through `Transaction`, so it keeps the safety net: restore point first, `.reg` backup and journal record before the write, listed in Backups while in effect, Undo and Undo all reach it. They are not listed in Tools because the watcher, not the user, makes them. A session that a crash or power cut left open is put back at the next start with no game running.
2. **Off by default.** Gaming Mode and the game timer are switches in Tools, both off until the user turns them on, so nothing changes because a game started unless asked.
3. **The timer request is not journalled.** It is held by the app's process and Windows drops it when the process ends, so there is nothing on disk to back up or undo.
4. **Names only.** The watcher reads running program names; it never opens, reads or changes a game process (plan section 12). Programs whose name other software shares (Minecraft Java's `javaw.exe`) are not watched.
5. **Not in Gaming Mode:** pausing Windows Update (Kegan's brief: never touch Windows Update) and raising the game's priority (the per-game tool H31).

