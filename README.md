# PeakTweaks

A native Windows gaming optimisation utility: Tauri 2 shell, Rust engine, React/TypeScript frontend, one elevated `peaktweaks.exe`. The full plan is in [`docs/PEAKTWEAKS_DEV_PLAN.md`](docs/PEAKTWEAKS_DEV_PLAN.md); section 15 there tracks where the implementation differs from it.

## Layout

| Path | What |
|---|---|
| `crates/field-check` | `peaktweaks-field-check.exe`: collects real-PC evidence for the open items into one report (`docs/FIELD_CHECK.md`). Development tool, not shipped. |
| `crates/engine` | The engine: `RegistryBackend`, journal, `Transaction`, tweaks, `Engine`. No Tauri dependency, so it builds and tests on any OS. |
| `src-tauri` | Thin Tauri shell: async IPC commands, manifest (`requireAdministrator`), capability. |
| `src/generated` | TypeScript generated from the Rust types (ts-rs) plus `fixtures.ts`. Do not edit; `cargo test -p peaktweaks-engine` regenerates it and CI fails if it is stale. |
| `src` (rest) | Frontend. Currently a placeholder. |

## Commands

```sh
# Engine tests (any OS): journal, transaction, revert semantics, property test.
cargo test -p peaktweaks-engine

# Compile-check Windows-only code from Linux/macOS (no linker needed).
rustup target add x86_64-pc-windows-msvc
cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings

# Frontend
npm ci
npm run dev          # UI in a browser on the SAMPLE mock (labelled), http://localhost:1420
npm run typecheck    # app and Node-side TypeScript
npm test             # vitest: store, mock, error wording, components
npm run lint:copy    # no user-facing text may promise a result
npm run build        # release bundle (contains no SAMPLE data)
npm run e2e          # Playwright smoke test + axe scan at 1366x768 and 150% scaling

# Full app (Windows). The exe asks for administrator rights on launch.
npx tauri build --no-bundle
```

CI (`.github/workflows/ci.yml`) is the gate. Windows: fmt, clippy `-D warnings`, check, test, tsc, vite build, Tauri build, manifest check, stale-bindings check, live probes, field check. Linux: engine tests, no-networking-crates check. Frontend: types, vitest, copy lint, release-bundle check, Playwright + axe.

## Running the engine by hand

A normal build cannot apply anything: the restore-point gate is closed and the license is Free until Phases 3 and 7 exist. To exercise the engine yourself on a test machine:

```sh
npx tauri dev --features dev-stubs   # from an elevated terminal (not yet run by anyone)
```

`dev-stubs` is never enabled in CI or release builds.

## Rules that are enforced by tests

- No IPC command takes environment, license, tier or gate state (`src-tauri/src/command_audit.rs`).
- Tweaks cannot reach the registry except through `Transaction` (`tweak_sources_do_not_bypass_the_transaction`).
- Revert replays only what is outstanding, newest first, and only within the tweak's declared registry targets.
