# The tool catalogue: everything Hone and ExitLag offer, and how PeakTweaks builds it

**Kegan's decision (2026-10-06, DECISIONS 15.22):** "any single feature that Hone has you can do", and "any single feature that ExitLag has, you can make". A tweak Hone ships therefore counts as supported, which settles the "forum post" rule of plan section 12 for those tweaks. Allowing ExitLag's features also allows the app to use the network for them (ping tests, traffic priority), which it did not do before (C24).

What does **not** change: the safety net (a restore point before the first change, a backup and a journal record before each one, Undo and Undo all), and the copy rule (no "faster", "FPS", "boost" in the app without a stored proof run).

**Who builds it:** the Claude session on Kegan's PC, because it can build and run the app on real Windows and check every registry path there. This session (cloud) did the research and reviews.

Every path and value below is **VERIFY** (from tool scripts, vendor pages and memory) until checked against Microsoft documentation or a real PC.

## Hone

Hone's own groups: FPS and latency, network and ping, quality of life, privacy, GPU, storage, background processes, Boost-Ups (cleaning), per-game settings. Since Hone 1.4, many toggles are merged into bigger groups. Names below are Hone's where public, otherwise descriptive.

| # | Hone feature | What it changes | How PeakTweaks builds it |
|---|---|---|---|
| H1 | Gaming optimizations (Game Mode, Game Bar and recording off, GPU scheduling, windowed-game optimizations) | `GameBar`, `GameConfigStore`, `GameDVR`, `GraphicsDrivers\HwSchMode`, `DirectX\UserGpuPreferences` | ValueTweak |
| H2 | Optimize Windows timer resolution | A program holds a 0.5 ms timer while games run; on Windows 11 also `kernel\GlobalTimerResolutionRequests=1` | Registry value + a background helper that holds the timer while a known game runs (process watcher) |
| H3 | Optimize CSRSS settings | `Image File Execution Options\csrss.exe\PerfOptions` `CpuPriorityClass=4`, `IoPriority=3` | IFEO priority, generalised from the Fortnite tweak |
| H4 | Optimize raw mouse input | `mouclass`/`kbdclass` `Parameters\MouseDataQueueSize`/`KeyboardDataQueueSize`; pointer precision | ValueTweak |
| H5 | Hone Gaming Mode (background processes while playing) | While a game runs: pause indexing and updates, notifications quiet, game priority up | Process watcher; session-only changes put back when the game closes |
| H6 | Optimize Message Signal Interrupts | `Enum\PCI\<device>\<instance>\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties\MSISupported=1` for the GPU and network card | Per-device keys (wildcard allowlist), restart needed, Advanced |
| H7 | Power plan (Hone's high-performance plan, core parking off, minimum processor state 100%, USB and PCIe power saving off) | `powercfg` | New undo type: our own copy of the plan, previous active plan remembered |
| H8 | Disable power throttling | `Power\PowerThrottling\PowerThrottlingOff=1` | ValueTweak |
| H9 | System responsiveness, MMCSS game priority, network throttling index | `Multimedia\SystemProfile` (+ `Tasks\Games`) | ValueTweak |
| H10 | Hibernation and fast startup off | `Session Manager\Power\HiberbootEnabled=0`; `powercfg /h off` | ValueTweak + power undo type for `/h` |
| H11 | Visual effects, animations, transparency | `Desktop`, `WindowMetrics`, `VisualEffects`, `Personalize` | ValueTweak |
| H12 | Startup apps | `Explorer\StartupApproved\Run` flags (same as Task Manager) | Per-entry list in Tools; registry, exact Undo |
| H13 | Background apps off | `BackgroundAccessApplications\GlobalUserDisabled=1` | ValueTweak |
| H14 | Privacy: telemetry, advertising ID, tips, activity history | Policy and user values; plus telemetry services and scheduled tasks | ValueTweak; services and scheduled tasks need their undo types |
| H15 | Notifications off while playing | Focus assist / `PushNotifications\ToastEnabled` | Session-only via the process watcher |
| H16 | Search indexing off | Service `WSearch` | Service undo type |
| H17 | SysMain (Superfetch) off on SSD PCs | Service `SysMain` | Service undo type |
| H18 | Xbox services off (for PCs without Game Pass) | `XblAuthManager`, `XblGameSave`, `XboxNetApiSvc`, `XboxGipSvc` | Service undo type; never offered when Game Pass is installed |
| H19 | Fullscreen optimizations off per game | `AppCompatFlags\Layers\<game exe>="~ DISABLEDXMAXIMIZEDWINDOWEDMODE"` | Per installed game (exe from `game_installs.rs`) |
| H20 | NVIDIA settings (low latency mode, power management "prefer maximum performance", shader cache size, texture filtering, threaded optimization, vertical sync) | NVIDIA driver profile (NvAPI DRS) | New undo type: previous setting values recorded |
| H21 | AMD settings (Anti-Lag and others) | Per-adapter registry under the display class key | Per-device keys |
| H22 | Disable Nagle (Low Latency Mode) | `Tcpip\Parameters\Interfaces\{guid}\TcpAckFrequency=1`, `TCPNoDelay=1` | Per-adapter keys (wildcard allowlist) |
| H23 | TCP settings | `netsh int tcp set global` (autotuning, RSS, ECN) | New undo type: previous `show global` values |
| H24 | Network adapter settings (interrupt moderation, energy-efficient Ethernet, flow control, power saving) | Adapter advanced properties under `Class\{4d36e972-...}\00xx`, then adapter restart | Per-device keys + restart side effect |
| H25 | DNS (Cloudflare / Google) | Adapter DNS servers | New undo type: previous DNS servers per adapter |
| H26 | QoS for game traffic | Policy-based QoS: DSCP 46 for the game's exe | Registry policy + policy refresh; see E3 |
| H27 | Explorer and quality-of-life (file extensions, classic context menu, web results off in Start, wallpaper quality 100) | `Explorer\Advanced`, `CLSID` override, `Search`, `Desktop\JPEGImportQuality` | ValueTweak |
| H28 | Boost-Ups: junk cleaner (temp, Windows Update cache, thumbnail and DirectX shader caches, crash dumps) | File deletion | Cleanup action: shows sizes, asks once, cannot be undone (says so), journalled as an action |
| H29 | Boost-Ups: drive optimisation | Windows' own `defrag /O` (TRIM on SSD) | Runs Windows' tool; nothing to undo |
| H30 | Per-game "pro settings" (Fortnite, Valorant, CS2, Apex, Minecraft) | The game's own settings file | File undo type (backup of the whole file); `ini.rs` exists (N55) |
| H31 | Game process priority | `Image File Execution Options\<game exe>\PerfOptions` | Generalise the Fortnite tweak to every installed known game |

## ExitLag

| # | ExitLag feature | What it is | How PeakTweaks builds it |
|---|---|---|---|
| E1 | Route optimisation and Multipath | Game traffic sent through ExitLag's own servers around the world | **Cannot be built in the app.** It is a paid server network, a separate business. Options for Kegan: leave it out, or later partner with or resell an existing routing service. |
| E2 | FPS Boost | Overlays off, visual effects, background processes, priorities, unneeded services | Same as H1, H5, H11, H16-H18 |
| E3 | Traffic Shaper | Game packets first; other programs' bandwidth limited while playing | Policy-based QoS: DSCP 46 for the game, throttle rates for chosen programs (launchers, cloud sync), Delivery Optimization bandwidth limit; session-only while a game runs |
| E4 | Network Analyzer | Ping, jitter and packet loss to the game's servers; local network quality | ICMP echo (`IcmpSendEcho`, no networking library) to the router and to published game-region addresses; Wi-Fi vs cable and Wi-Fi signal |
| E5 | Multi-Internet | Several connections at once, failover | Without servers, only failover: prefer the cable over Wi-Fi (interface metric), warn when a game runs on Wi-Fi |
| E6 | RAM Cleaner | Frees memory | One-shot: empty the standby list and trim working sets (Windows' memory-list API); nothing to undo |

## Not built, even though another app has it

These are in plan section 12 ("never do"). Kegan's approval covers Hone and ExitLag features; if either app has one of these, it still needs Kegan to change section 12 explicitly first.

- Turning off Memory Integrity or virtualization-based security; toggling Secure Boot, TPM or IOMMU.
- Anything inside a game process: memory, injection, Roblox fast flags (including Roblox FPS unlockers), game CPU affinity.
- BIOS settings; vulnerable kernel drivers.

Also left out: turning off Spectre/Meltdown mitigations, Defender, SmartScreen or Windows Update. These appear in EXM Tweaks, not in Hone or ExitLag as far as the public material shows.

## Order of work

1. Engine groundwork: per-PC keys (wildcard allowlist segment) and side effects after a write, each journalled.
2. Network: Nagle per adapter (H22), Network Analyzer (E4), Traffic Shaper and QoS (E3, H26), DNS (H25).
3. Power plan (H7, H10) and services (H16-H18, H14).
4. Per-game: process priority for every installed game (H31, H3), fullscreen optimizations (H19).
5. Process watcher: one-click Proof, Gaming Mode (H5, H15), timer resolution (H2).
6. NVIDIA (H20), adapter settings (H24), MSI (H6), cleanup (H28, H29, E6), per-game settings (H30).

## Sources

- [Hone 1.4 update: Boost-Ups and merged optimizations](https://hone.gg/blog/hone-1-4-update-boost-ups-and-merged-optimizations/), [Recommended optimizations](https://support.hone.gg/hc/en-gb/articles/4758238924959-Recommended-Optimizations-to-Use), [Using Hone](https://support.hone.gg/hc/en-gb/articles/4758186993183-Using-Hone), [Hone: optimize Windows 11 for gaming](https://hone.gg/blog/optimize-windows-11-for-gaming/), [Hone: fix high latency](https://hone.gg/blog/fix-high-latency-issues/)
- [What is ExitLag](https://www.exitlag.com/blog/what-is-exitlag/), [How ExitLag works](https://www.exitlag.com/blog/how-exitlag-works/), [ExitLag guide 2026](https://www.exitlag.com/blog/exitlag-guide/)
- [EXM utility script (GitHub gist)](https://gist.github.com/Svxy/2a1fb6314aa7ad353d5b5001c148485b)
