# PeakTweaks — working brief

Windows gaming optimizer: one elevated `peaktweaks.exe` (Tauri 2), Rust engine, React/TS/Tailwind UI. Owner: Kegan. Branch: `claude/peaktweaks-windows-setup-sw5aub` (PR keganscott/PC-TWEAK-APP#1).

**Read on demand, not up front:** `docs/NOTES.md` (open items, N-numbers) when choosing work · `docs/DECISIONS.md` (why things are the way they are; 15.x numbering) when changing a design · `docs/PEAKTWEAKS_DEV_PLAN.md` (Kegan's plan: §1 locked decisions, §6 Phase 5, §7 UI, §11 decisions he owes, §12 never-do) when a task touches it · closed items: `docs/archive/NOTES-closed.md`.

## Rules (from Kegan; non-negotiable)
- Never call something done without running it; quote the CI run / command output. Windows CI is the gate.
- No efficacy claims ("faster", "FPS", "boost", …) in copy unless bound to a stored proof run. Lints enforce it (`scripts/claim-words.json`).
- Keep `VERIFY` markers until checked against a primary source. Don't guess plan §11 decisions; ask or put behind a flag.
- Plan vs code conflict, or the plan looks wrong: stop and tell Kegan.
- Anything unfinished or assumed → a row in `docs/NOTES.md` (BLOCKED/TODO/ASSUMED) with why and what closes it.
- Never (plan §12): BIOS writes, vulnerable drivers, hosting GPU drivers; toggling Secure Boot/TPM/IOMMU or turning Memory Integrity off; game memory/injection/fast flags/game CPU affinity; any change without a verified restore point + `.reg` backup + journal record first; accepting env/license/tier/gate state from the UI; unlabeled sample data.
- Commits end with the Co-Authored-By/Claude-Session trailer. No new PR unless asked.

## Invariants the code relies on
- Every registry write goes through `Transaction` (journal record + `.reg` backup first), limited to the tweak's `touches()` allowlist; revert replays the journal, refuses another account's hive.
- The engine builds `SystemEnv`, licence and restore gate itself; no IPC command takes them (`src-tauri/src/command_audit.rs` enforces, also that every command is registered, permitted and wrapped in `src/ipc.ts`).
- Probes return `Probe<T>`: yes / no(reason) / unknown(reason). Never round unknown to yes or no.
- Rust types → TS via ts-rs into `src/generated/` (+ `fixtures.ts` of real engine output). CI fails if stale: run `cargo test -p peaktweaks-engine` and commit.
- UI: store slices are immutable (stable selectors); racing requests are tagged; every action records `Op` success/failure. SAMPLE mock exists only in `vite dev` / `--mode e2e`, never in a release bundle (CI checks).

## Code map
- `crates/engine/src/` — `engine.rs` (Engine API), `transaction.rs`, `journal.rs`, `registry/` (trait, fake, windows), `context.rs`/`identity.rs` (whose hive), `secure_dir.rs` (protected ProgramData), `restore.rs`/`restore_win.rs`, `sysprobe.rs` (probe cache, `SystemAudit`), `hardware.rs`, `security.rs`, `power.rs`, `scanner.rs` (findings), `settings.rs`, `tweaks/` (catalogue), `proof/` (PresentMon capture, metrics, verdict, store, NVML), `wmi.rs` (worker thread), `proc.rs` (timeout runner), `copy_lint.rs`.
- `src-tauri/src/` — `main.rs` (start; engine failure keeps window + `startup-error.log`), `commands.rs` (IPC via `EngineHandle`).
- `src/` — `ipc.ts` (typed client), `services/` (backend seam, SAMPLE mock), `store/`, `components/{shell,ui,views}`, `lib/` (error wording).
- `crates/field-check/` — real-PC evidence tool (`docs/FIELD_CHECK.md`). `e2e/` Playwright vs mock; `e2e-tauri/` WebDriver vs the real app.

## Commands
- Engine (Linux OK): `cargo test -p peaktweaks-engine`; `cargo clippy -p peaktweaks-engine --all-targets -- -D warnings`.
- Windows-only code from Linux: `cargo clippy -p peaktweaks --all-targets --target x86_64-pc-windows-msvc -- -D warnings` (Tauri can't build on Linux).
- UI: `npm run typecheck`, `npm test`, `npm run lint:copy`, `npm run e2e` (set `PW_CHROMIUM_PATH=/opt/pw-browsers/chromium-1194/chrome-linux/chrome` here).
- `cargo fmt --all` before committing. Run all relevant checks *before* pushing; a push cancels the running CI.

## CI and evidence
Jobs: `windows` (fmt, clippy, tests, live probes, Tauri build, launch smoke test, field check, real-app WebDriver e2e — evidence steps print at the end of the log), `engine-linux`, `frontend` (vitest, copy lint, release-bundle check, Playwright + axe). Read results with the GitHub MCP `get_job_logs` (`tail_lines` ~150 covers the evidence steps). Record evidence as a closed note with the run id.

## Status (2026-10-02)
Engine, IPC, scanner (8 read-only checks), settings, proof pipeline and the UI are built and green on CI, including the real app driven end to end. The registry write path is proven on the real registry: backend contract, engine apply/revert, and `reg.exe` import of the `.reg` backups (C20). The IPC permission boundary and forged-state refusal are shown in the real app (C21). Apply/Undo runs end to end in the real app on the real registry (C25); the app makes no outbound connections and the source has no network code (C24). Kegan's agent brief: DECISIONS.md 15.17; open questions N49-N51. Phase gates 3/4/5 are **not met**: they need real-PC evidence (field check), real game runs, three hand-checked machines. Blocked on Kegan: the field-check run, `tweak-dictionary.md`/`competitive-audit.md`/`tweak-framework.md` (N2; needed for Auto mode, Quick Safe, more tweaks), plan §11 decisions.
