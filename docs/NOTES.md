# Deferred work and open items

Working log for anything **not finished, not verified, or decided by assumption**, with the reason and what closes it. Read this first when resuming. Update it at the end of every work chunk; move items to "Closed" with the evidence (CI run, commit) when done.

Legend: **BLOCKED** = needs something from Kegan or a real machine. **TODO** = doable, not yet done. **ASSUMED** = I picked a default; change it if wrong.

## Open

| # | Kind | Item | Why it is open | What closes it |
|---|---|---|---|---|
| N1 | BLOCKED | Frontend (R20, R21, plan section 7): `applySuite` failure reporting, `boot()`/`revertAll()` error states, race tagging, SAMPLE labels, real driver facts | The React/TS files (`types.ts`, `mockIpc.ts`, `store.ts`, components, `App.tsx`, ...) are not in the repository; `src/` is a placeholder page. | Kegan uploads the frontend files. Then: R20, R21, replace `mockIpc.ts` with the typed client in `src/ipc.ts`, delete hand-written duplicates of generated types, add vitest + Playwright smoke test. |
| N2 | BLOCKED | Reference docs `tweak-dictionary.md`, `competitive-audit.md`, `tweak-framework.md` | Not in the repository. The dictionary supplies evidence grades and the anti-catalog that Phases 5 and 6 need. | Kegan uploads them. |
| N3 | BLOCKED | UAC prompt appears on launch | GitHub runners are already elevated with UAC off, so CI cannot show the prompt. | Manual: run the CI artifact `peaktweaks-exe` on a normal Windows 10/11 account. |
| N4 | BLOCKED | R13 test matrix: Windows 10 22H2, Windows 11 24H2/25H2, alternate admin credentials, RDP, Administrator Protection on | Needs VMs / real machines. Code paths exist (`identity.rs`), only the pure parts are tested. | Manual runs; record the resolved `UserResolution` and SID for each. |
| N5 | BLOCKED | R14: WebView2 starting under Administrator Protection with `WEBVIEW2_USER_DATA_FOLDER` moved into the interactive profile | Needs a Windows 11 build with Administrator Protection enabled (26100.9267+ / 26200.9267+). | Manual run in that VM. |
| N6 | BLOCKED | `SPI_SETMOUSE` array order `[threshold1, threshold2, acceleration]` (VERIFY in `tweaks/mouse_accel.rs`) | `learn.microsoft.com` is blocked by the build sandbox's egress proxy. | Read the Microsoft docs (or test on a machine: set thresholds and observe). Then delete the VERIFY note. |
| N7 | BLOCKED | Fortnite executable name `FortniteClient-Win64-Shipping.exe` (VERIFY in `tweaks/ifeo_priority.rs`) | From memory; no install to check. | Confirm on a machine with Fortnite installed. The tweak stays blocked for every game until anti-cheat testing (`CLEARED_GAMES` is empty). |
| N8 | BLOCKED | Claim that `Win32PrioritySeparation` 0x26 equals the client default of 2 | Taken from the plan, not independently checked. The tweak is deleted, so nothing depends on it. | Only matters if it is ever reintroduced. Check against Microsoft docs first. |
| N9 | TODO | Test that a **non-admin user pre-creating** `%ProgramData%\PeakTweaks` is refused | CI runs as admin, so the owner check (`SID_SYSTEM` / `SID_ADMINISTRATORS`) is only exercised for "ordinary directory under ProgramData". | Create a low-privilege local user in CI (`New-LocalUser`), create the directory as that user via `Start-Process -Credential` or a scheduled task, then run the verifier. |
| N10 | TODO | Runtime proof that the Tauri app-manifest command list actually denies unlisted commands | Only the build-time side is verified (permissions generate, capability names resolve, audit test). | Launch the app, call an unlisted command from the webview, expect a denial. Needs the frontend (N1) or a devtools session. |
| N11 | TODO | `npx tauri dev --features dev-stubs` has never been run | No display / elevated Windows session available here. | Run once on a test machine; fix the README if the flag spelling is wrong. |
| N12 | TODO | Placeholder app icon | Generated a flat coloured square only so the build has an icon. | Real artwork, then `npx tauri icon <png>`. |
| N13 | TODO | CSP has no `'unsafe-inline'` for styles | Placeholder frontend needs none. The React/Tailwind app may (inline `style=` attributes). | Check the browser console for CSP violations once the real frontend runs; add the narrowest directive that works. |
| N14 | TODO | CI actions are on Node 20 (`checkout@v4`, `setup-node@v4`, `upload-artifact@v4`); GitHub forces Node 24 and warns | Cosmetic today. | Bump action majors when convenient. |
| N15 | ASSUMED | Production defaults: restore gate closed, license Free, so **nothing can be applied** in a normal build until Phase 3 (restore) and Phase 7 (licensing) land | Follows plan R12 and the restore-gate rule. | Phase 3 opens the gate for real; Phase 7 replaces the license stub. Use `--features dev-stubs` meanwhile. |
| N16 | ASSUMED | `Tweak::touches()` must keep old targets when a release changes what a tweak writes, or revert of old applies fails with `ContextViolation` | Consequence of the R1 allowlist. Documented on the trait method. | Keep in mind when editing any shipped tweak. Consider a `legacy_touches()` if it starts to bite. |
| N17 | ASSUMED | Known games list is only Fortnite, Minecraft, Roblox | Section 11 item 5 (which anti-cheat titles to support) is Kegan's decision. | Kegan decides; edit `KNOWN_GAMES` in `env.rs`. |
| N18 | ASSUMED | All five Section 11 decisions untouched (pricing, unelevated Store SKU, open-sourcing the catalogue, merchant of record, anti-cheat titles) | Not mine to guess. Nothing built so far depends on them; licensing is a stub. | Kegan answers before Phase 7 (pricing, merchant of record) and Phase 5/6 (Store SKU, catalogue, titles). |
| N19 | NOTE | The Phase 2.1 hardening landed as one large commit (`5505a46`) rather than one per concern | The rewrite touched every file at once and the pieces do not compile separately. | Nothing to do; later phases use small commits. |

## Closed

| # | Item | Evidence |
|---|---|---|
| C1 | `LocalFree` signature (R5) | Checked against `windows-0.58.0` source; Windows CI green. |
| C2 | P0 scaffold, CI, manifest, capability | CI run 36621445888. |
| C3 | R1-R4, R6-R9, R11-R19 (engine side) | CI run 36621445888; 97 engine tests on Windows. |
| C4 | Launch smoke test on a Windows runner | Same run: process survives 20 s, `C:\ProgramData\PeakTweaks` ACL is SYSTEM + Administrators only. |
