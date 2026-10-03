# Field check: the real-PC evidence, in one run

Most of what is still open in `docs/NOTES.md` cannot be settled on GitHub's
virtual Windows Server runners: they have no GPU, no display that draws frames,
no System Restore and no Windows 11 client features. `peaktweaks-field-check.exe`
collects all of it on a real PC in one run and writes one `report.json`.

It is a development tool. It is not shipped to users.

## Get it

1. Open the latest green run of the `ci` workflow on this branch (GitHub, Actions tab).
2. Download the artifact **peaktweaks-field-check**. It contains
   `peaktweaks-field-check.exe` (with the pinned PresentMon inside it) and the
   report the CI runner produced, as an example of the output.

Or build it on a Windows PC with Rust: `scripts\fetch-presentmon.ps1`, then
`cargo build --release -p peaktweaks-field-check`.

## Run it

Open PowerShell **as Administrator** in the folder with the exe.

**Step 1, read-only (about 30 seconds).** Changes nothing on the PC, except that it
creates the protected `C:\ProgramData\PeakTweaks` folder if it is not there yet,
exactly as the app does on first start.

```powershell
.\peaktweaks-field-check.exe
```

**Step 2, frames (about 1 minute).** Start a game, or play a video or animation in
a browser, keep it in the foreground, and name its exe:

```powershell
.\peaktweaks-field-check.exe --capture FortniteClient-Win64-Shipping.exe --seconds 30
# or, with a video playing in Edge:
.\peaktweaks-field-check.exe --capture msedge.exe --seconds 20
```

Every run writes a new folder `peaktweaks-field-check-<time>` with `report.json`.
A capture also leaves `capture.csv` and `presentmon-real.csv` (the first 600
frames). Commit `presentmon-real.csv` as
`crates/engine/tests/fixtures/presentmon-real.csv` and the parser test stops
saying NOT VERIFIED.

**Step 3, optional and it CHANGES THINGS: a real restore point.** This is the
app's real flow. It turns System Protection on for the system drive if it is
off, lifts Windows' one-point-per-24-hours limit (a journalled registry change),
creates a restore point and checks Windows recorded it. It asks you to type
`yes` first.

```powershell
.\peaktweaks-field-check.exe --create-restore-point
# afterwards, to put the 24-hour limit back the way it was:
.\peaktweaks-field-check.exe --revert-all
```

`--revert-all` undoes everything PeakTweaks has applied on that PC (the same
code as the app's "Undo all"). It does not delete the restore point and does not
turn System Protection back off (see N23).

## What each part of the report settles

| Report section | Notes item | What to look for |
|---|---|---|
| `audit` | N22, N41, N42, N43, N53, N54, N56, C14 | The full probe result and the scanner's findings for this PC. Compare memory type, speed, channels, disk type, refresh rate and rig class with what you know the PC has; where Fortnite, Roblox or Minecraft is installed, check `env.gameInstalls` names the right folder and drive, and on a laptop with two graphics chips check `env.gpuChoices` against Settings > System > Display > Graphics. The Phase 5 gate needs this on three PCs. |
| `powercfgList`, `powercfgActive` | N43 | Windows' own names for each power-plan GUID. They must match the four GUIDs in `crates/engine/src/power.rs`. |
| `wmiPhysicalMemory` | N22 | The raw `SMBIOSMemoryType`, `Speed`, `ConfiguredClockSpeed`, `BankLabel` and `DeviceLocator` behind the memory reading. |
| `wmiPhysicalDisks` | N22 | The raw `MediaType` behind the SSD/HDD reading. |
| `wmiVideoControllers` | N28 | Driver version and date, for the future driver advice. |
| `systemRestoreRegistry`, `systemRestorePolicy`, `shadowStorage` | N23, N25 | Candidate signals for "System Protection is on". Run step 1 once with protection on and once with it off and compare. |
| `gpuThrottle` | N33 | On an NVIDIA PC: real NVML readings. During a capture (step 2) this is sampled while the game runs. |
| `presentmonCapture` | N39, N34 | The real CSV header, frame counts and the statistics our code computes from it. |
| `createRestorePoint` | N24, N26 | The real outcome, and the restore status afterwards (the gate should be open). |
| `journal` | | Everything PeakTweaks has written on this PC. |

Every section records `ok`, how long it took, and either the result or the
error. A failed section never stops the others.

## Before you share a report

The report can identify the PC: hardware serial-like IDs (`PNPDeviceID`, disk
names), and the journal shows the Windows account's SID. Read it before
attaching it anywhere public. Sending it to whoever works on PeakTweaks is the
intended use.
