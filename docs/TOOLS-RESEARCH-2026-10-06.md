# Tools research and plan (2026-10-06)

Written for Kegan, after his first run of the tester build, and for whichever Claude session builds it next. Nothing here is built yet. Every registry path below is from published tool scripts and from memory, so each one is **VERIFY** until it is checked against Microsoft documentation or a real PC (plan section 12).

## 1. Why Tools shows only two changes

1. **The catalogue was never filled in.** The original upload came with a handful of tweaks. The plan (section 12) says no tweak ships "whose only support is a forum post; every tweak needs an evidence grade in the dictionary". The dictionary (`tweak-dictionary.md`, NOTES N2) was never uploaded. So I held the catalogue at the two original tweaks, plus the scanner's read-only checks, until Kegan approved new tweaks (2026-10-06, DECISIONS 15.19). That approval came yesterday; nothing has been added since.
2. **"Pointer precision: Already set outside PeakTweaks"** means mouse acceleration was already off on that PC, set by Windows, a game or a previous change. PeakTweaks found nothing to change, so it offers nothing to undo. The wording is unclear; it should read like **"Already off on this PC"** with a tick (see section 6).
3. **"Fortnite process priority"** is the only game-specific tweak from the original upload. It asks Windows to start Fortnite at high CPU priority. It belongs under Games, not as one of two items in Tools.

## 2. What the other apps actually do

| App | What it offers | Can PeakTweaks do the same? |
|---|---|---|
| **Hone** | About 60 one-click optimizations in four groups: FPS and latency (Windows and power settings), network and ping (TCP settings), quality of life (debloat, Explorer, mouse), and background processes (pausing telemetry and indexing while playing). Per-game "pro settings". The free plan has 10 optimizations. | Mostly yes: almost all of it is registry, power plan and service changes, which fit the existing undo system or need small extensions (section 3). |
| **ExitLag** | Mainly **network routing**: game traffic sent through ExitLag's own servers, over several paths at once (Multipath), plus Multi-Internet failover. Also an "FPS Boost" (overlays off, priority, unneeded services off). | **The routing, no.** It is a paid server network. Building one is a separate business and breaks the app's "no network" promise. The FPS Boost part, yes (it overlaps Hone). A read-only **connection check** is possible (section 5). |
| **EXM Tweaks** | A large script: power plan and CPU power settings, timers and boot options (`bcdedit`), memory settings, TCP (Nagle), network throttling, MMCSS game priority, GPU vendor registry values, Game Bar and Game DVR off, Xbox services off, telemetry and ~60 scheduled tasks off, debloat (removing Store apps, even uninstalling Edge), and **turning off security protections** (VBS, Memory Integrity, Spectre/Meltdown mitigations, CFG, ASLR, SmartScreen). | **Much of it, yes**, with backups. **Not** the security parts: some are on the plan's never-do list, and they can break anti-cheat (section 4). |

The common core is the same in all three. That core is what PeakTweaks should have first, each change one click, backed up and undoable.

## 3. Proposed catalogue

Evidence grade: **A** = documented by Microsoft or the vendor as a user setting. **B** = widely used, with a clear mechanism, and harmless when it does nothing. **C** = disputed or forum-only. Per the plan, C goes behind Proof (measured on that PC) or Advanced until it earns a better grade.

"Undo type" says what the existing engine needs. **Reg** fits `Transaction` today (journal record and `.reg` backup first). Anything else needs a new journal record kind first, like the planned refresh-rate fix (DECISIONS 15.19).

### 3a. Build first: registry only, fits today's safety system

| # | Change | What it sets (VERIFY) | Grade | Undo type | Notes |
|---|---|---|---|---|---|
| 1 | Background recording off (Game DVR) | `HKCU\System\GameConfigStore\GameDVR_Enabled=0`, `HKCU\Software\Microsoft\Windows\CurrentVersion\GameDVR\AppCaptureEnabled=0` | A | Reg | Approved Starter fix (15.19). Free. |
| 2 | Game on the dedicated graphics chip (laptops with two) | `HKCU\Software\Microsoft\DirectX\UserGpuPreferences\<game exe>="GpuPreference=2;"` | A | Reg | Approved Starter fix; the scanner already finds it (`gpu.choice`). Free. |
| 3 | Optimizations for windowed games | same key, `DirectXUserGlobalSettings` gets `SwapEffectUpgradeEnable=1;` | A | Reg | The Windows 11 setting Settings > Display > Graphics shows. Must keep the other parts of that string. |
| 4 | Variable refresh rate for windowed games | same string, `VRROptimizeEnable=1;` | A | Reg | Only when the display supports VRR. |
| 5 | Hardware-accelerated GPU scheduling | `HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\HwSchMode=2` | A | Reg | Restart needed. Needed for DLSS Frame Generation; disputed for others, so offer, do not push. |
| 6 | Game Mode on | `HKCU\Software\Microsoft\GameBar\AutoGameModeEnabled=1` | A | Reg | On by default; this repairs a PC where it was turned off. |
| 7 | Sticky/Filter/Toggle Keys shortcuts off | `HKCU\Control Panel\Accessibility\StickyKeys\Flags` (and Keyboard Response, ToggleKeys) | A | Reg | Stops the Shift x5 pop-up taking you out of a game. Quality of life. |
| 8 | Keyboard repeat delay shortest | `HKCU\Control Panel\Keyboard\KeyboardDelay=0`, `KeyboardSpeed=31` | A | Reg | Same as the Settings slider. |
| 9 | Pointer precision off | exists | A | Reg | Fix the "already set" wording. |
| 10 | Transparency effects off | `HKCU\...\Themes\Personalize\EnableTransparency=0` | A | Reg | For low-end PCs. |
| 11 | Animations off | `HKCU\Control Panel\Desktop\WindowMetrics\MinAnimate=0` plus the Performance Options flags | A | Reg | For low-end PCs. |
| 12 | Background apps off | `HKCU\...\BackgroundAccessApplications\GlobalUserDisabled=1` | A | Reg | Windows 10; Windows 11 is per app. |
| 13 | Startup apps you choose | `HKCU`/`HKLM ...\CurrentVersion\Run` entries, or the `StartupApproved` flags (same as Task Manager) | A | Reg | Show the list; the user ticks. Uses StartupApproved so Undo is exact. |
| 14 | Power throttling off | `HKLM\SYSTEM\CurrentControlSet\Control\Power\PowerThrottling\PowerThrottlingOff=1` | B | Reg | Laptops: battery cost, so it says so. |
| 15 | Nagle's algorithm off (TCP games) | per interface `Tcpip\Parameters\Interfaces\{guid}\TcpAckFrequency=1`, `TCPNoDelay=1` | B | Reg | Affects TCP only. Most shooters use UDP; Minecraft Java uses TCP. Offer per game. |
| 16 | Windows tips and suggestions off | `ContentDeliveryManager` values, `SystemPaneSuggestionsEnabled=0` | A | Reg | Quality of life. No performance claim. |
| 17 | Telemetry to the minimum the edition allows | `HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection\AllowTelemetry` | A | Reg | Privacy, not speed; the copy must say so. |
| 18 | Mouse and keyboard data queue sizes | `mouclass`/`kbdclass` `Parameters\*DataQueueSize` | C | Reg | Advanced, behind Proof. |
| 19 | MMCSS game task priority and SystemResponsiveness | `...\Multimedia\SystemProfile` | C | Reg | Only affects programs that register with MMCSS. Advanced. |
| 20 | Global timer resolution requests | `...\Session Manager\kernel\GlobalTimerResolutionRequests=1` | C | Reg | Windows 11 changed timer behaviour; behind Proof. |

### 3b. Build next: needs a new undo type

| # | Change | Mechanism | Grade | Undo type to build |
|---|---|---|---|---|
| 21 | High performance / Ultimate Performance power plan | `powercfg /duplicatescheme`, `/setactive` | A | **Power plan record**: remember the previous active plan GUID, delete our copy on Undo. |
| 22 | Core parking off, minimum processor state 100% | `powercfg /setacvalueindex` on our own plan copy | B | Same record, on our copy only, so the user's plans are never edited. |
| 23 | Services a gamer may not need (Xbox services if not on Game Pass, Print Spooler if no printer, Fax, Maps, Remote Registry, Bluetooth if none) | `Services\<name>\Start` | B | Reg covers the value; add a **service record** to also stop/start the service and check dependencies. Ask per service; never Windows Update, Defender or anti-cheat services. |
| 24 | Scheduled tasks (telemetry and CEIP) | `schtasks /change /disable` | B | **Task record** (previous enabled state). Privacy only. |
| 25 | Pause indexing and updates while a game runs | Services/APIs, temporary | B | Session-only change, reverted when the game closes. Hone's "background processes". |
| 26 | Network adapter: interrupt moderation, energy-efficient Ethernet, power saving off | adapter `Class\{4d36e972...}\00xx` values, then restart the adapter | B/C | Reg plus an **adapter restart**; connection drops for a few seconds, so say so. |
| 27 | NVIDIA profile: power management "prefer maximum performance", low latency mode, shader cache size | NVIDIA driver settings API (NvAPI DRS) | A (vendor setting) | **Driver-profile record**. Not driver hosting (plan section 12 allows it). AMD equivalent later. |
| 28 | In-game competitive settings (Fortnite `GameUserSettings.ini`, etc.) | Edit one `Key=Value` line | A (Epic's own advice for Fortnite) | **File record**: the parser exists (`ini.rs`, NOTES N55); needs backup-and-restore of the file. This is Hone's "pro settings". |
| 29 | Display refresh rate to the maximum | `ChangeDisplaySettingsEx` | A | **Display record** (planned, 15.19). The scanner already finds it (`display.refresh_rate`). |

### 3c. Will not build, and why

| Change seen in other apps | Why not |
|---|---|
| Memory Integrity / VBS off, HVCI off | Plan section 12 never-do. Also required by some anti-cheats (Vanguard, FACEIT) on Windows 11. |
| Spectre/Meltdown, CFG, SEHOP, ASLR mitigations off | Removes protection against real attacks for the whole PC. Same reasoning as Memory Integrity; anti-cheats may refuse to run. Only Kegan can overrule this, and I advise against it. |
| SmartScreen, Defender, Windows Update off | Security. Not a gaming setting. |
| Uninstalling Edge or Store apps | Not cleanly undoable (Edge also runs WebView2, which PeakTweaks itself uses). |
| `bcdedit` timer changes (`disabledynamictick`, `useplatformtick`, HPET) | Boot-configuration changes outside the registry, grade C, and some (HPET) are reported to make things worse on modern CPUs. Could come back later behind Proof with a boot-record undo type. |
| Game CPU affinity, Roblox fast flags, anything inside a game process | Plan section 12 never-do; anti-cheat risk. |
| ExitLag-style routing | Needs a paid server network; breaks "no network". |

## 4. Warnings

Kegan's direction: fewer warnings, because every change is backed up and a restore point exists. Proposal:
- No warning text on grade A and B changes. One line where there is a real cost (a restart, battery, a dropped connection for a few seconds).
- One confirmation only for Advanced (grade C) changes.
- The safety net stays as it is: the restore point before the first change, the backup before each one, Undo and Undo all.

## 5. Network ("like ExitLag")

What PeakTweaks can do without servers: a **connection check**. It would cover Wi-Fi vs cable, packet loss, jitter, and ping to the game's region (needs a ping to the game's servers). It would also cover adapter power saving and Nagle for TCP games (3a #15, 3b #26). That is honest and useful. It needs network access, which the app currently never uses (C24, `check-no-network-deps.sh`). So it is a decision for Kegan: allow a network check that sends only pings to game servers, or keep "no network".

## 6. Proof, made one click

Today: create a comparison, type the program name, choose Before or After, press Record six times, then Compare. Proposal:

1. **One button on a game card: "Measure my next session".** PeakTweaks watches for that game to start (its exe is known for installed games, `game_installs.rs`).
2. When the game is running, it records automatically: three 30-second samples spread through the session, no clicks, with a small "recording" note in the activity bar.
3. After the session, the card says **"Baseline ready. Apply changes now?"** One click applies the suggested set (or opens Tools).
4. Next session, it records "after" the same way and shows the result. It reuses the existing verdict, so "no measurable change" stays possible and is shown honestly.
5. For the most comparable runs, the guide suggests a repeatable scene per game (for example a replay or a practice map). The suggestion lives in `gameGuidance.ts`.

Needs: a process watcher (polling the process list every few seconds, no injection), automatic capture scheduling in `ProofService`, and the card UI. All local, no network.

## 7. Order of work (suggested)

1. Wording fix for "already set" (Tools), and move Fortnite priority under Games.
2. Catalogue 3a #1–#17, grade A/B, one click each, plus an **"Apply the safe set"** button (the Auto on Home).
3. One-click Proof (section 6).
4. New undo types and 3b, starting with the power plan and in-game settings.
5. Kegan's decisions: the network check (section 5); the security mitigations (3c, recommended no); pricing (plan section 11).

Each new tweak needs a failing test first, its `touches()` allowlist, a `VERIFY` until checked, and no efficacy words in its copy (`scripts/claim-words.json`).

## Sources

- Hone: [Hone Premium overview (Salad)](https://community.salad.com/optimize-your-gaming-pc-with-hone-premium/), [Hone optimization guide](https://hone.gg/blog/optimize-pc-for-gaming/), [Hone premium pricing](https://hone.gg/premium)
- ExitLag: [What is ExitLag](https://www.exitlag.com/blog/what-is-exitlag/), [ExitLag guide 2026](https://www.exitlag.com/blog/exitlag-guide/)
- EXM Tweaks: [EXM utility script (GitHub gist)](https://gist.github.com/Svxy/2a1fb6314aa7ad353d5b5001c148485b), [EXM free tweaks](https://exmtweaks.com/en-us/free-tweaks/33-pc-tips-tweaks)
