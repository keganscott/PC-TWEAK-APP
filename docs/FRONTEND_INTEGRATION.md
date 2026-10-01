# Bringing the existing frontend in

Kegan has a React + TypeScript + Tailwind frontend that ran against a mock
(plan section 2). It is not in the repository yet. This page says where to put
it and exactly what happens to it next, so the hand-over is quick.

## 1. Kegan: add the files

Commit them to this branch (`claude/peaktweaks-windows-setup-sw5aub`) at the
paths the plan lists, replacing the placeholder `src/main.ts` and `index.html`:

```
src/types.ts
src/services/mockIpc.ts
src/store/store.ts
src/components/ui/primitives.tsx
src/components/shell/{AppShell,TopBar,ExecutionBus}.tsx
src/components/views/{Scanner,QuickSafe,Sandbox,DriverAdvisor,Backups}View.tsx
src/App.tsx
src/main.tsx
src/index.css
tailwind.config.js
index.html
```

Plus the `package.json` dependencies it used (React, Tailwind, `lucide-react`).
Do not touch `src/ipc.ts` or `src/generated/`: those are the contract with the
engine and are regenerated from Rust.

Uploading through GitHub's web UI ("Add file" > "Upload files") onto the branch
is fine.

## 2. Claude: wire it to the real engine

In this order, one commit each, CI green after each:

1. **Build as delivered.** Install its dependencies; `tsc` and `vite build` pass
   unchanged before anything else moves.
2. **One set of types.** Replace hand-written duplicates in `src/types.ts` with
   the generated types in `src/generated/` (R11). Anything the UI needs that the
   engine does not provide is listed for Kegan, not invented.
3. **Real IPC.** Views call `engine.*` / `proof.*` from `src/ipc.ts`. The mock
   stays only for `vite dev` outside Tauri, is built from `src/generated/fixtures.ts`,
   and every value it shows carries a visible SAMPLE label (R21, plan 4.8).
4. **Store fixes (R20).** Suite apply reports per-item results; `boot()` and
   `revertAll()` get error states; target-game requests are tagged so a late
   reply never overwrites a newer one; selectors return stable references
   (plan section 7).
5. **Driver advice (R21).** Delete invented numbers; use only the facts the plan
   gives, labelled with their source, until the driver probe exists (N28).
6. **New screens the engine already supports.** Restore lock as an inline step
   (create restore point with progress), scan findings with "what is already
   right", Telemetry & Proof (sessions, captures, verdict headline shown as the
   engine words it), settings (rig-class override, plain/technical wording),
   journal and Undo all.
7. **Tests.** vitest for store actions (failure reporting, races); a Playwright
   smoke test against the SAMPLE mock (Chromium is available in CI images);
   accessibility checks from plan section 7 (status never colour-only, focus
   trap in modals, labelled toggles, 1366×768 at 125%/150% scaling).

## Already in place for it

- `src/ipc.ts`: typed client for every engine command, with `EngineFault` errors.
- `src/generated/`: TypeScript types generated from the Rust types, plus
  `fixtures.ts`, real engine output checked by `tsc` against those types.
- `npm run lint:copy`: fails the build if user-facing text promises a result
  (shared word list `scripts/claim-words.json`; plan section 0.4).
- The IPC audit (`src-tauri/src/command_audit.rs`) fails CI if any command takes
  environment, licence, tier or gate state, or lacks a typed client call.
