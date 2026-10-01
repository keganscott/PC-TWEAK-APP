# PeakTweaks — Development Plan for Claude Code

Author: Kegan Griffiths (owner). Written 2026-09-29 after a full code review and market research pass.
Hand this file to Claude Code together with the existing code (the `src-tauri/` Rust tree, the React/TypeScript files, and the three reference docs `tweak-dictionary.md`, `competitive-audit.md`, `tweak-framework.md`).

> **Revision note (2026-09-29, Claude):** Section 15 at the end lists every place the implementation departs from, or sharpens, the text below, with the reason. Nothing in Section 1 (Locked decisions) was changed.

---

## 0. How to work on this (read first)

1. **The existing Rust has never been compiled on Windows.** Only pure helper logic was tested on Linux. Treat all of it as reviewed-but-unproven. Your first job is to make it compile and pass tests on a Windows runner (Phase 2.1, task P0).
2. **Work one phase at a time.** Each phase ends at a gate (Section 3 onward). Do not start the next phase until the gate is met and Kegan has said go.
3. **Never claim something is done that you did not run.** "Done" means: `cargo check`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` pass on Windows, and the frontend `tsc` and build pass. Report the exact commands and results.
4. **Never write efficacy claims into UI copy** ("we measured", "X% faster", "boosts FPS") unless the text links to a stored proof-run id (Phase 4). Third-party numbers may appear only labeled as third-party and unverified.
5. **Any API signature marked VERIFY below is from memory or a secondary source.** Check it against the crate source or Microsoft docs before relying on it. The `windows` crate is pinned at 0.58; signatures change between releases.
6. **Small commits, one concern each.** Keep the registry-touching code behind the `Transaction` type. No tweak may write to the registry, run a command, or touch a service except through it.
7. **Ask before deviating from a Locked Decision (Section 1).** If a decision looks wrong, say so with evidence, but do not silently change it.

---

## 1. Product and locked decisions

**Product:** PeakTweaks, a native Windows gaming optimization utility for budget gamers on older hardware, extended to mid and high-end rigs. Tauri 2 shell, Rust engine, React + TypeScript + Tailwind frontend. Binary: `peaktweaks.exe`. Backups in `<app data>/peaktweaks_backups/` today; moving to ProgramData (see R1).

**Locked decisions (from Kegan):**
- Firmware (BIOS/UEFI): detect and guide only. No BIOS writes, no bundled vulnerable drivers, ever.
- Anti-cheat gate: no toggles to disable Secure Boot, TPM or IOMMU. Tweaks are blocked per selected game by a structured `BlockedReason { code, trigger, message }`.
- Deep mitigations (Spectre/Meltdown, VBS) only in an "Offline / Single-Purpose Rig" category behind a confirmation modal.
- Two-tier tweak UI: Quick Safe suite plus Advanced Sandbox cards.
- The dashboard is locked until a restore point is verified or created. Before every change: `.reg` export plus a JSON journal.
- One portable elevated binary (`requireAdministrator`), no Windows service.
- Interactive user resolution: own token first, falling back to explorer.exe's token; user-context tweaks write `HKEY_USERS\<SID>` (HKCU when the SID is ours).
- IFEO PerfOptions for game process priority; "background isolation" (demote non-game processes), not game CPU affinity.
- 24h restore-point limit workaround: `SystemRestorePointCreationFrequency = 0` (journalled and restorable).
- Never host or redistribute GPU drivers. Link to vendor pages only.
- Tiers: Free Scanner and Pro/Ultimate as originally set. **Proposed change (needs Kegan's OK):** Free Starter mode, drop weekly plans, Pro $24.99/yr or $4.99/mo, Ultimate $49.99/yr or $8.99/mo. Ultimate = AI Rig Engineer + community benchmarks. See Section 9.
- A dedicated top-level "Telemetry & Proof" tab (PresentMon).

---

## 2. Current code inventory

Rust (`src-tauri/`):
- `Cargo.toml` — tauri 2, serde, serde_json, winreg 0.52, windows 0.58 (features: Foundation, Security, Security_Authorization, System_Diagnostics_ToolHelp, System_RemoteDesktop, System_Threading, UI_WindowsAndMessaging).
- `src/main.rs` — `is_elevated()`, setup, `invoke_handler`.
- `src/engine/{error,types,context,journal,mod}.rs` — error enum, `Tweak` trait and types, SID/hive resolution, journal + `Transaction`, engine and Tauri commands.
- `src/tweaks/{mod,priority_separation,mouse_accel}.rs` — two reference tweaks.
- **Missing entirely:** `build.rs`, `tauri.conf.json`, `capabilities/`, icons, Windows manifest, CI.
- Duplicate flat copies of the Rust files may exist next to the tree. Use the `src-tauri/src/...` copies only.

Frontend (delivered as flat files; place them as below):
`src/types.ts`, `src/services/mockIpc.ts`, `src/store/store.ts`, `src/components/ui/primitives.tsx`, `src/components/shell/{AppShell,TopBar,ExecutionBus}.tsx`, `src/components/views/{Scanner,QuickSafe,Sandbox,DriverAdvisor,Backups}View.tsx`, `src/App.tsx`, `src/main.tsx`, `src/index.css`, `tailwind.config.js`, `index.html`, `README.md`. It ran against a mock (`mockIpc.ts`) and passed `tsc` and `vite build` when built earlier. It needs `lucide-react`.

Reference docs: `tweak-dictionary.md` (tweak catalogue with evidence grades A/B/C/D/N, risk 1–4, anti-cheat posture, and an anti-catalog of harmful tweaks to detect and revert), `competitive-audit.md`, `tweak-framework.md`.

---

## 3. PHASE 2.1 — Harden the engine (do this first, about 1 week)

**Gate:** Windows CI green (check, test, clippy, fmt); all journal tests below pass; no IPC command can mutate without an engine-owned gate; app builds and launches elevated.

### P0. Build scaffold and CI
1. Fix `context.rs` `sid_from_token`: on windows 0.58 `LocalFree` takes `HLOCAL`, not `Option<HLOCAL>`. Replace `LocalFree(Some(HLOCAL(raw.0 as *mut c_void)))` with `LocalFree(HLOCAL(raw.0 as *mut c_void))`. Then compile and fix any other signature mismatches. VERIFY each against `windows-0.58.0` source.
2. Add `build.rs`, `tauri.conf.json`, `capabilities/default.json`, icons.
   - `tauri.conf.json`: identifier e.g. `com.peaktweaks.app`; one window; strict CSP (`default-src 'self'; script-src 'self'; connect-src ipc: http://ipc.localhost`; `style-src 'self' 'unsafe-inline'` only if the build needs it); no remote URLs; no `withGlobalTauri` unless required.
   - Capabilities: main window only, `core:default`, **no** fs, shell, http or opener plugins.
   - Restrict app commands to the explicit list via Tauri's app manifest command list if available in the pinned Tauri 2 version (VERIFY).
   - Manifest: request `requireAdministrator` through `tauri_build::WindowsAttributes::app_manifest`. The manifest must keep the Common Controls v6 dependency or the app misbehaves; known pitfall is a "duplicate resource" error if a manifest is embedded twice (Tauri issues 6732, 10154). Test that UAC prompts on launch.
3. `WEBVIEW2_USER_DATA_FOLDER`: under Administrator Protection the elevated process runs as a different, hidden user and WebView2 fails to start (Tauri issue 13926). Before the Tauri builder runs, set this env var to a folder the *interactive* (de-elevated) user can write, e.g. under that user's Local AppData resolved with the interactive token. Test in a Windows 11 VM with Administrator Protection enabled (Release Preview builds 26100.9267 / 26200.9267 or later; policy-controlled, off by default).
4. CI: `.github/workflows/ci.yml` on `windows-latest`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo check`, `cargo test`, then Node install, `tsc --noEmit`, `vite build`, frontend tests. Cache cargo and npm.
5. Introduce a `RegistryBackend` trait (read value, write value, delete value, open/create key, key exists) with a real `winreg` implementation and an in-memory fake. `ContextResolver` and `Transaction` use the trait, so journal logic runs in tests on any OS. The `windows`-crate-only code (SID, tokens) stays behind `#[cfg(windows)]` with a stub for tests.

### R1. Journal must not be user-writable (HIGH, privilege escalation)
Problem: journal and `.reg` backups live in `app_data_dir()`, which is Roaming AppData. Any process running as the user can append a journal line for a real tweak id with an arbitrary `root`, `key_path`, `value_name` and `previous`. `restore_journalled` replays it as admin. `guard_context` checks only the hive class, not the key.

Fix:
- Store journal and backups in `%ProgramData%\PeakTweaks\` (resolve with `SHGetKnownFolderPath(FOLDERID_ProgramData)`; VERIFY feature flags). Create it from the elevated process with a protected DACL: full control to SYSTEM and Administrators only, inheritance on, no Users write. Example SDDL `D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` applied with `ConvertStringSecurityDescriptorToSecurityDescriptorW` + `SetNamedSecurityInfoW` (VERIFY). On every start, verify the directory's owner and DACL and **refuse to start mutating** if a non-admin principal has write access.
- Per-tweak allowlist: add `fn touches(&self) -> &'static [RegTarget]` to the `Tweak` trait, where `RegTarget { root, key, values: &'static [&'static str] }`. `Transaction` receives it at `begin`. `set_raw`, `delete_value` **and** `restore_journalled` refuse any (root, key, value) outside it with `EngineError::ContextViolation`. Registry key compare is case-insensitive.
- Test: a journal line naming a key outside the tweak's allowlist is rejected on revert and nothing is written.
- This also fixes Administrator Protection (two identities, two profiles, one history).

### R2. Engine owns the environment (HIGH)
Problem: `set_environment(env: SystemEnv)` lets the webview set `system_protection_enabled`, `target_game`, `secure_boot`, etc.; `Engine::apply` trusts them.

Fix:
- Delete `set_environment`. Add `select_target_game(game_id: String)` (validated against the known game list) and `rescan()` (runs probes in Rust). `SystemEnv` is built only by the engine from probes (Phase 3) and is never deserialized from IPC.
- The restore-point gate reads a value produced by the Rust restore probe with a short TTL, re-checked inside `Engine::apply` immediately before writing.
- Add a test that no `#[tauri::command]` takes a `SystemEnv`.

### R3. Torn journal tail (HIGH)
Problem: a crash can leave a half-written last line without a newline. The next append is glued to it; `read_all_at` stops at the first unparsable line, so all later records disappear (state shows Foreign, revert returns `NoJournalEntry`).

Fix:
- On `Journal::open`: read the file, find the last complete valid line, and truncate the file to that point (or refuse and report). Also guarantee each append starts on a fresh line (write `\n` first if the file does not end with one).
- In `read_all_at`: on a bad line that is **not** the final line, skip it and record a `JournalWarning`; never stop silently. Expose warnings to the UI (Backups tab).
- Tests: truncated final line; garbage mid-file; empty file; file with trailing partial UTF-8; append after each case and confirm the new record is readable.

### R4. Revert must restore the state before the latest apply (HIGH)
Problem: `restore_journalled` replays every Apply the tweak ever had, newest first. Apply, revert, external change to 0x28, apply, revert ends at the oldest value, not 0x28. `revert_all` orders by catalogue position, not by apply time.

Fix (recommended design):
- Journal records get a `tx_id` (the first write's `seq`) and a `Commit { tx_id, action }` record appended after a transaction completes. State per tweak = the last committed transaction's action.
- Revert replays only the Apply writes belonging to transactions **after the last committed Revert** (including one uncommitted trailing Apply from a crash), newest first.
- `revert_all` orders tweaks by their last Apply `seq`, descending.
- Tests (using the fake backend): apply→revert→external change→apply→revert lands on the external value; two tweaks touching one value revert in reverse apply order; crash between writes (no Commit) still reverts correctly.

### R6. Transactions must roll back on failure (MEDIUM)
`Engine::apply`: if `tweak.apply(&mut tx)` returns `Err`, undo the writes this transaction already made (restore their `previous`), append a `Commit` for a Revert, and return the original error. Test with a fake backend that fails the second of three writes (`mouse_accel` writes three values).

### R7. Mouse tweak live-push bugs (MEDIUM)
- `push_live` runs even when the resolved user is not the process user (`is_self == false`), changing the wrong profile. Expose `tx.user_is_self()`; only call `SystemParametersInfoW` when true.
- `revert` always pushes hard-coded `[6, 10, 1]`, contradicting its own comment. After restore, read `MouseSpeed`, `MouseThreshold1`, `MouseThreshold2` and push those.
- Verify the `SPI_SETMOUSE` array meaning (threshold1, threshold2, acceleration) against Microsoft docs.

### R8. Unknown state (MEDIUM)
Add `TweakState::Unknown { detail }`. A failed `read_state` must not become `Default`. The UI shows it and blocks Apply.

### R9. Journal index (MEDIUM)
Keep an in-memory index (tweak id → last committed action, last apply seq) built at open and updated on append. `is_applied` and `list()` stop re-reading the file per tweak.

### R10. Async commands (MEDIUM)
Synchronous Tauri commands run on the main thread. Make every engine command `async`, run engine work in `spawn_blocking`, hold the engine in `Arc<Mutex<_>>`, and do not hold the lock across long waits (PowerShell). Emit progress via Tauri events (`engine://progress` with `{ stage, tweakId?, message }`).

### R11. One serde contract (MEDIUM)
- `#[serde(rename_all = "camelCase")]` on all IPC structs; `TweakState` internally tagged, e.g. `#[serde(tag = "status", rename_all = "camelCase")]`.
- Generate TypeScript from Rust (ts-rs or specta; pick one) into `src/generated/`; delete hand-written duplicates in `types.ts`. CI fails if generated files are stale.
- Align names: Rust `Diagnostic` vs TS `'readonly'`; TS `blockedReason: string` becomes structured `{ code, trigger, message }`; remove `serviceRunning` / `userAgentRunning` (the service architecture was dropped); `revert_all` returns a struct list, not tuples.
- Add fixture tests: Rust serializes fixtures to JSON, a frontend test parses them with the generated types.

### R12. Tier enforcement in Rust (MEDIUM)
`Engine::apply` must check `metadata.tier` against a license state held in Rust. UI-only gating is bypassable. Until Phase 7 licensing exists, the license state is a dev-only stub behind a cargo feature; production builds default to Free.

### R13. User resolution (MEDIUM)
- Anchor on the app's own session: `ProcessIdToSessionId(GetCurrentProcessId())`, and look for `explorer.exe` in **that** session, not `WTSGetActiveConsoleSessionId`.
- Correct the module docs: "UAC elevation keeps the same SID" is true only for classic UAC. Under Administrator Protection the elevated token is a hidden system-managed account with a different SID and HKCU (Microsoft Learn). The `InteractiveShell` path is therefore the important one.
- Test matrix (manual, on VMs): Windows 10 22H2; Windows 11 24H2/25H2; launch with different admin credentials; RDP session; Administrator Protection on.

### R15. Replace the placeholder reference tweak (MEDIUM)
`Win32PrioritySeparation` 0x26 equals the client default of 2 (no-op). Also its predicate message says "We measured no difference", which is false. Remove both. Make the reference Service tweak **IFEO PerfOptions `CpuPriorityClass`** for a selected game exe (`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\<exe>\PerfOptions`, DWORD: 1 Idle, 2 Normal, 3 High, 4 Realtime, 5 BelowNormal, 6 AboveNormal). Use 3 (High), never 4. Key creation must be journalled (see R18). Gate it behind the anti-cheat readiness display; do not enable for any title until tested per anti-cheat vendor.

### Low items (do in the same patch)
- R16: delete the stale comment in `context.rs` about an "original version" that leaked handles.
- R17: `fsync` `.reg` backups and the directory; refuse to journal unsupported value types (do not map unknown types to `REG_NONE`).
- R18: add `created_key: bool` to journal records; on revert delete keys the tweak created if they are empty.
- R19: drop `Deserialize` from `TweakMetadata`; use `u64` for `unix_ms`.
- R20 (frontend): `applySuite` must collect per-item results and report failures; add try/catch and an error state to `boot()` and `revertAll()`; tag `setTargetGame` requests so out-of-order responses are ignored.
- R21 (frontend): every mock value shown in the UI is labeled SAMPLE. Delete the invented driver numbers (`566.36`, "89 captures") and the Fortnite "requires core isolation" block. Replace driver advice with real facts: NVIDIA's final Game Ready driver for Maxwell/Pascal/Volta was October 2025; security-only updates continue through October 2028; security driver 582.28 shipped 2026-02-01.
- R22: keep winreg 0.52 / windows 0.58 / wmi 0.14 pinned until Phase 3 compiles; upgrade afterwards as its own commit (windows signatures change between releases).

### Tests required at the end of Phase 2.1
Journal: torn tail, mid-file garbage, seq monotonic after reopen, apply/revert cycles with external changes (proptest over random sequences of apply, revert, external write, crash-truncate, asserting the final registry equals the expected model), allowlist rejection, rollback on partial failure, `.reg` output (UTF-16LE + BOM, deletion directive `"Name"=-`, hex wrapping). Contract: fixtures round trip. Security: no command accepts `SystemEnv`.

---

## 4. PHASE 3 — Probes, restore engine, IPC (about 2–3 weeks)

**Gate:** restore point verified on Windows 10 22H2, Windows 11, and an Administrator Protection VM; all probes return tri-state values (Yes / No / Unknown) and never guess; the dashboard lock works end to end.

Original Phase 3 request, refined:

### 4.1 Cargo
Add `wmi` (pinned 0.14 initially; check which `windows` version it pulls in and whether two versions coexist; accept duplicates or align, do not fight it) and `windows` feature `Win32_System_Com` (and any others compilation demands).

### 4.2 `engine/wmi.rs` — COM apartment safety
Tauri and WebView2 initialize COM on the main thread. Do not create COM objects there. Run all WMI on **one dedicated worker thread** that initializes COM once (multithreaded apartment), owns every `WMIConnection`, receives probe requests over a channel and replies over oneshot channels. Handle `RPC_E_CHANGED_MODE` and `RPC_E_TOO_LATE` (security already initialized) explicitly. Give each query a timeout. VERIFY wmi 0.14's `COMLibrary` API (`new`, `without_security`).

### 4.3 `engine/security.rs`
Return `Probe<T> = Yes(T) | No | Unknown(reason)`.
- Secure Boot: registry `HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State\UEFISecureBootEnabled` (DWORD). Key absent means legacy BIOS or unsupported → `No` with reason.
- HVCI / Memory Integrity: WMI `root\Microsoft\Windows\DeviceGuard` class `Win32_DeviceGuard`, `SecurityServicesRunning` includes the HVCI value (VERIFY numeric values); cross-check registry `...\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity\Enabled`.
- TPM: WMI `root\CIMV2\Security\MicrosoftTpm` `Win32_Tpm` (`IsEnabled_InitialValue`, `IsActivated_InitialValue`, `SpecVersion`); requires elevation; absent class → `No`.
- IOMMU / DMA protection: **cannot be detected reliably from user mode.** Return `Unknown` unless a documented signal is found (VERIFY `Win32_DeviceGuard.AvailableSecurityProperties`). Never claim `Yes` without a signal.
- `AntiCheatReadiness { secure_boot, tpm, iommu, per_game: Vec<{game_id, requirement, status}> }`. Requirements for Fortnite tournaments per Epic's announcement: Secure Boot, TPM, IOMMU (from 2026-02-19). Display only; never toggle.

### 4.4 `engine/hardware.rs`
- `Win32_PhysicalMemory`: `Capacity`, `Speed` vs `ConfiguredClockSpeed`, `BankLabel`, `DeviceLocator`, `SMBIOSMemoryType`; derive **stick count** and a **channel estimate** (one stick = single channel; two sticks in different banks/channels = dual; otherwise `Unknown`).
- Total RAM, `Win32_Processor` (name, `NumberOfCores`, `NumberOfLogicalProcessors`), `Win32_VideoController` (name, driver version/date; `AdapterRAM` is capped at 4 GB in WMI, so read `HardwareInformation.qwMemorySize` from the display adapter registry key or use DXGI), boot disk media type (`MSFT_PhysicalDisk`, namespace `root\Microsoft\Windows\Storage`), display refresh (current vs maximum mode via `EnumDisplaySettings`).
- Rig class: from the weakest of memory, graphics, logical processors, boot disk (see Section 6). Pure function, unit-tested.

### 4.5 `engine/restore.rs`
- Read state: is System Protection enabled for the system drive; last restore point; current `SystemRestorePointCreationFrequency`.
- Enable protection when off (WMI `SystemRestore.Enable` in `root\default`, or PowerShell `Enable-ComputerRestore`; note group policy `DisableSR` at `HKLM\SOFTWARE\Policies\Microsoft\Windows NT\SystemRestore` blocks it; detect and report).
- Set `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore\SystemRestorePointCreationFrequency = 0` **through `Transaction`** so it is journalled and restorable. Windows otherwise rate-limits to one point per 24 hours.
- Create: `SRSetRestorePointW` (srclient.dll; load dynamically, BEGIN_SYSTEM_CHANGE with `MODIFY_SETTINGS`, then END_SYSTEM_CHANGE; VERIFY struct layout), fallback PowerShell `Checkpoint-Computer -RestorePointType MODIFY_SETTINGS`. PowerShell: absolute path `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`, `-NoProfile -NonInteractive`, fixed command strings with **no user-supplied interpolation**, timeout, capture exit code and stderr.
- **Verify** creation by listing restore points (WMI `root\default` `SystemRestore`, `SequenceNumber`) and comparing before/after; only then set the gate to verified. Journal a record for the restore point (id, time, method).
- Long operation: runs in `spawn_blocking`, emits progress events, never on the main thread.

### 4.6 Live `SystemEnv`
`SystemEnv` becomes the output of `rescan()`, assembled from the probes above, cached with a TTL and invalidated after any apply/revert. It is never accepted from IPC.

### 4.7 IPC commands (all async)
`audit_system` (runs all probes, returns findings + env summary), `create_restore_point`, `apply_tweak(id)`, `revert_tweak(id)`, `revert_all`, plus `list_tweaks`, `select_target_game`, `list_journal`, `engine_context`. Errors serialize as `EngineError` (camelCase, `kind` tag). Each returns typed results and emits progress events.

### 4.8 Frontend wiring
Replace `mockIpc.ts` with a real `ipc.ts` using generated types and `@tauri-apps/api`; keep the mock only for `vite dev` without Tauri and mark all mock data SAMPLE. Implement the restore-point lock as an inline step (see Section 7).

---

## 5. PHASE 4 — Telemetry & Proof tab (about 2 weeks)

**Gate:** run-to-run variance measured on real runs; the UI refuses to call a difference "an improvement" when it is not larger than the measured spread.

- Bundle PresentMon's console app (MIT license; include the license text and record the version and hash) or download a pinned, hash-verified release. VERIFY the current release and CLI flags.
- Capture with: `--process_name <exe>`, `--timed <seconds>`, `--delay <seconds>`, `--terminate_after_timed`, `--output_file <path>`. Parse the CSV columns `MsBetweenPresents`, `MsBetweenDisplayChange`, `MsCPUBusy`, `MsGPUBusy`, `DisplayLatency` (VERIFY names against the shipped version; they differ across versions). Requires admin or the Performance Log Users group; we are elevated.
- Metrics: average FPS, 1% and 0.1% lows (computed from frame times, not from averaged FPS), frame-time percentiles, and a stutter count.
- Method: A/B with N runs per side (default 3), same scene and duration, warm-up discarded. Report median and spread per side. Store every run (CSV + metadata: game build, rig class, tweak set, timestamp) under a `runId`. UI verdicts: "Better", "No measurable change" or "Worse", decided only when the difference exceeds the run-to-run spread.
- NVIDIA throttle reasons via NVML (`nvmlDeviceGetCurrentClocksThrottleReasons`; thermal slowdown, hardware slowdown, software power cap): load `nvml.dll` from the driver install, do not bundle it. AMD/Intel paths are unresearched; show "not available".
- Starter tier gets **one** proof run for its own fixes; Pro unlimited.
- Lint rule (test): UI copy may contain "measured" only when bound to a `runId`.

---

## 6. PHASE 5 — Scanner, rig classes, Auto mode, free Starter (about 3–4 weeks)

**Gate:** automated check that Starter builds contain no networking dependency and open no outbound connections; scanner findings match hand-verified results on at least three real machines.

### 6.1 Rig class (Low / Mid / High)
Weakest-of-four rule: memory, graphics, logical processors, boot disk. Suggested thresholds (tune with data): **Low** = ≤ 8 GB RAM, or integrated / ≤ 4 GB VRAM graphics, or ≤ 4 logical processors, or HDD boot. **High** = ≥ 12 GB VRAM and ≥ 16 logical processors and 144 Hz+ display. Everything else is **Mid** (the typical Steam PC: 16 GB RAM, 8 cores, 1080p). User can override. The class changes defaults and copy only; it never hides a safety gate.

### 6.2 Scanner checks (all unelevated, Diagnostic context; each finding = reading, severity, remedy, `guidedOnly`, optional `fixTweakId`)
1. Single memory stick / single channel (guide only; do not push kit purchases; DDR4 32 GB kits are about $150–180 and DDR5 from about $350 as of Sept 2026).
2. Memory below rated speed (`Speed` vs `ConfiguredClockSpeed`) → guide to XMP/EXPO.
3. Refresh rate below monitor maximum → one-click fix (user hive, reversible).
4. Game on the integrated GPU on laptops → set per-app GPU preference (`HKCU\Software\Microsoft\DirectX\UserGpuPreferences`, VERIFY).
5. Power mode not on performance → set it (Epic's own FPS steps recommend this).
6. GPU throttling (NVML) → advice only.
7. Game installed on a hard drive → advise SSD.
8. Driver branch: Pascal and older NVIDIA have no newer Game Ready driver; recommend security updates only.
9. Background load → list top offenders at idle; offer to demote known-safe launchers/updaters (below-normal priority / EcoQoS; VERIFY the process power-throttling API, do not touch game affinity).
10. Windows 10 end of support: consumer ESU extended to 2027-10-12 (single source; re-verify before shipping copy).
11. Memory Integrity: Microsoft begins auto-enabling on eligible Windows 11 PCs from October 2026; PCs where it was deliberately disabled keep that. Offer an A/B proof run; **never disable automatically.**
12. Foreign tweaks from other tools: compare against the anti-catalog in `tweak-dictionary.md`; offer undo behind a restore point.
Order findings by expected gain, not by category. Include "what is already right".

### 6.3 Auto mode
One primary button on Home: scan → classify → verify/create restore point → apply the safe set for the class → re-check → show a result card (what changed, restore point id, "Undo all", "Prove it" 30-second test). Uses only tweaks with evidence grade A/B in `tweak-dictionary.md` and safety tier Safe.

### 6.4 Starter mode (free forever)
- Audience: parents and children about 8–12 on modest PCs (Minecraft, Roblox, Fortnite from about 11).
- Contents: plain-language scan ("fixed by us / fixable by you / needs hardware", printable), five one-click fixes with undo chosen by evidence grade (candidates: power mode, refresh rate, per-app GPU choice on laptops, background demotion, background recording off), game cards for Minecraft, Roblox, Fortnite (in-game guidance and OS-level items only), honest upgrade advice, one proof run.
- Hard limits: no Advanced tab, no Offline Rig category, no AI, no account.
- **Collects nothing.** No account, analytics, ads, crash uploads. No HTTP client crate in the Starter build (Cargo feature `starter`; CI runs `cargo tree` and fails on any networking crate, and a test greps for sockets). License check is offline. Store listing may say "collects no data" only while this holds.
- Rationale: COPPA treats persistent identifiers such as device IDs as personal information; the amended rule (compliance date 2026-04-22) adds retention, security-program and separate third-party consent duties. Collecting nothing avoids most of it. This is not legal advice; Kegan should consult counsel before any data leaves the device.
- On a standard Windows account the UAC prompt asks for an admin credential, which acts as a parental gate.

---

## 7. Frontend plan (runs alongside Phases 3–5)

- **Navigation:** Home, Games, Tools, Proof, Backups. The Advanced Sandbox lives inside Tools behind an Advanced switch. Drivers become a card on Home/Games.
- **Home:** one sentence on the PC's state, one primary Auto button, last result.
- **Restore lock:** an inline step inside Auto ("Turn on protection", one button), not a wall on other tabs.
- **Result card** after any apply: changes, restore point, Undo all, Prove it.
- **Language:** two registers, plain (default; forced in Starter) and technical (registry paths visible).
- **Execution Bus:** closed by default in Auto and Starter; open for pros.
- **Errors and empty states:** boot failure screen with retry, skeleton loaders, SAMPLE labels on any mock.
- **Accessibility (target WCAG 2.2 AA):** status is never colour-only; modal traps focus; toggles have accessible names; keep reduced-motion and focus rings; test at 1366×768 and 125%/150% scaling.
- **Store:** keep `useSyncExternalStore`, but selectors must return stable references (a selector that builds a new object each call will loop).
- **Tests:** vitest for store actions (suite failure reporting, race handling), Playwright smoke test against the mock.

---

## 8. PHASE 6 — Per-game profiles and Pro (about 3–4 weeks)

**Gate:** every shipped profile has a verified stamp (game build, date, rig class, proof `runId`) and passes its anti-cheat gate test.

- Profile = signed JSON data file, not code. Fields: game id, exe names, store ids, anti-cheat vendor and requirements, setting edits (file, keys, values per rig class), launch options with undo, OS items (per-app GPU preference, IFEO priority, background demotion list), allowed and blocked tweak ids, proof recipe (scene, duration), verified stamp.
- Sign packs with Ed25519 (`ed25519-dalek`); the app verifies against an embedded public key before use; a signed updater delivers packs without an app release. A profile past its stamp, or for a newer game build, shows "unverified for this version" and offers only build-independent parts.
- Detect installed games from launcher manifests (Steam `libraryfolders.vdf`, Epic manifests; VERIFY paths); show profiles only for installed games.
- Every profile write goes through `Transaction` like any tweak.
- **Fortnite first.** Sourced base (Epic's own low-FPS guidance): Performance rendering mode, high-resolution textures off, V-Sync off, SSD, close background programs, NVIDIA Control Panel Low Latency Mode Ultra and power management "Prefer maximum performance". Third-party extras (`-d3d11` via launcher extra arguments; large FPS claims) are unverified; ship them only after a proof run and label them so. Fortnite tournament requirements (Secure Boot, TPM, IOMMU) are a readiness display only. Config file location: VERIFY (`GameUserSettings.ini` under the user's Local AppData Fortnite config folder).
- **Minecraft:** research first (Java memory allocation, render distance) against Mojang documentation before writing a profile.
- **Roblox:** OS-level items and a guide to Roblox's own graphics-quality setting **only**. No fast-flag editing, no injection, until Roblox's own policy has been read; a banned child account costs more than any FPS gain.
- **Valorant / BattlEye / other kernel anti-cheat:** scan and guidance only until each vendor's rules are tested.
- IFEO priority: use `PerfOptions\CpuPriorityClass` High only. IFEO I/O priority is capped at Normal; child processes inherit only Idle/Below Normal. Elastic's IFEO rule flags `Debugger` and `MonitorProcess`, not `PerfOptions`, but that says nothing about any game's own anti-cheat; test per vendor.

---

## 9. PHASE 7 — Ultimate, licensing, release (about 4 weeks)

**Gate:** model output limited to catalogue ids (adversarial tests pass); a signed release installs and updates on a clean machine.

- **Licensing:** offline-verifiable Ed25519 license tokens checked in Rust; tier enforced in `Engine::apply` (R12). Payment via a merchant of record (not yet chosen; Kegan to decide; research tax handling). Renewals: clear disclosure, express consent, easy cancel (FTC click-to-cancel was vacated in 2025 but ROSCA enforcement continues).
- **AI Rig Engineer (Ultimate only, adult account):** deterministic recommender picks candidates from the verified catalogue; the LLM only explains and ranks. Model output is parsed against a schema and every tweak id is validated in Rust against the catalogue and the current gates. Treat hardware strings, game names, process names, window titles as untrusted prompt input (prompt-injection risk). Never render model output as HTML. Never send data without explicit opt-in. Never in Starter.
- **Signing:** Azure Artifact Signing (about $9.99/month; individuals in the USA and Canada are eligible; verify identity requirements). EV certificates give no instant SmartScreen reputation; reputation builds over consecutive releases signed by the same identity, and a new file hash resets file reputation, so ship fewer, larger releases and tell first-time users where "More info → Run anyway" is.
- **Antivirus:** sign; no packers or obfuscation; publish SHA-256 hashes; submit each release to Microsoft Defender and major vendors.
- **Updater:** signed updates (VERIFY Tauri updater signature requirements).
- **Store:** MSIX and `requireAdministrator` likely conflict; do not rely on the Store for the elevated app.

---

## 10. Pricing proposal (needs Kegan's approval)

| Tier | Price | Contents |
|---|---|---|
| Starter | Free | Scan, five fixes with undo, three game cards, one proof run. No account, no data collected. |
| Pro | $24.99/yr or $4.99/mo | Full Auto, all game profiles and updates, unlimited proof runs, Advanced tab, drift check. |
| Ultimate | $49.99/yr or $8.99/mo | Pro plus AI Rig Engineer and opt-in community benchmarks. |
| Dropped | Weekly plans | Annualize to $155.48 (Pro) and $259.48 (Ultimate), 6.2× and 5.2× the yearly price; highest complaint and legal risk. |

Recurring price only where recurring cost exists: profile maintenance (Pro) and model calls (Ultimate). Fixed cost: signing about $119.88/yr. All prices are proposals to test; competitor pricing was not verified.

---

## 11. Decisions Kegan still owes

1. Approve or change the tier and price proposal above (drop weekly plans; free Starter).
2. Build a second, **unelevated** Starter build for the Microsoft Store? (Read-only scanner and per-user fixes only; discovery and no SmartScreen wall; costs a second build; departs from the single-elevated-binary decision.)
3. Open-source the tweak catalogue (MIT, like WinUtil) and sell the app, profiles and proof harness?
4. Merchant of record and licensing vendor.
5. Which anti-cheat titles to support after Fortnite.

---

## 12. Things never to do

- Write BIOS/UEFI settings, load or bundle vulnerable kernel drivers, or host GPU drivers.
- Toggle Secure Boot, TPM or IOMMU, or disable Memory Integrity automatically.
- Touch game process memory, inject into games, edit Roblox fast flags, or set game CPU affinity.
- Apply a change without a verified restore point, a `.reg` backup and a journal record written first.
- Accept environment, license, tier or gate state from the frontend.
- Ship a tweak whose only support is a forum post; every tweak needs an evidence grade in the dictionary and, before any efficacy copy, a proof run.
- Use fabricated or sample numbers in the UI without a SAMPLE label.

---

## 13. Reference: review findings

High: R1 journal user-writable (privilege escalation); R2 client-controlled environment; R3 torn journal tail loses history; R4 revert restores the wrong value; R5 build does not compile and scaffold is missing.
Medium: R6 no transaction rollback; R7 mouse live-push bugs; R8 read errors become Default; R9 journal re-read per tweak; R10 sync commands on main thread; R11 Rust/TypeScript contract mismatch; R12 tier gating in UI only; R13 SID and session assumptions; R14 WebView2 under Administrator Protection; R15 placeholder reference tweak with a false "measured" claim.
Low: R16 stale comment; R17 fsync and unsupported value types; R18 created keys survive revert; R19 needless `Deserialize` and u128; R20 frontend error handling and races; R21 invented mock data; R22 dependency versions (winreg 0.56, windows 0.62.2, wmi 0.18.4 were current when checked).

Full write-up with sources: the report document "PeakTweaks: Code Review, Market Research and Game Plan".

## 14. Estimates (guesses for one developer)

Phase 2.1 about 1 week; Phase 3 two to three weeks; Phase 4 two weeks; Phase 5 three to four weeks; Phase 6 three to four weeks; Phase 7 about a month. Roughly four months end to end. The gates matter more than the dates.

---

## 15. Revision log and status (Claude, 2026-09-29)

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
