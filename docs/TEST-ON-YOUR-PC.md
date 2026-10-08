# Testing PeakTweaks on your own PC

This is the first time PeakTweaks runs on a real Windows PC. GitHub's test
machines are virtual Windows Servers: no System Restore, no GPU, no real
display. So several things have only ever been tested in pieces (docs/NOTES.md
N24, N3, N22, N43). This test is what settles them.

Allow about 30 minutes. Windows 10 22H2 or Windows 11, with an administrator account.

## What the tester build is

`peaktweaks-tester.exe` is the normal app with one difference: **every plan is
unlocked**, because licensing does not exist yet and every change in the catalogue
is a Pro change. Everything that keeps the PC safe is the real thing:

- PeakTweaks asks Windows for a **restore point before its first change**, and
  makes no change without one.
- Every change is **written down first, with a copy of the old value**, and can
  be undone on its own or with **Undo all** (Backups).
- It never touches the BIOS, drivers, game files or anti-cheat, and it collects
  no data.

The top bar says **Tester build** on every screen. **Tools** lists about 50
changes in sections (gaming, input, appearance, privacy, network, power,
services, graphics and more); the main view shows the Safe ones, and the
**Advanced** switch at the top shows the rest, plus MSI mode per device.
Below them: **While you play** (Gaming Mode and the game timer), **Startup
apps**, **Connection** (a check of your connection) and **One-time actions**
(empty the standby list, clear junk files, optimize the Windows drive).
Settings this PC already has are listed as **Already optimized**, not hidden.
Changes that need hardware this PC does not have (an NVIDIA or AMD card, Wi-Fi
and a cable) are folded under "... do not apply to this PC" in their section. The per-game changes
(Fortnite and Roblox process priority, game traffic priority) stay "Not
available" until they have been tried with each game's anti-cheat (NOTES N75).

## 1. Get the two programs

You need `peaktweaks-tester.exe`, and `peaktweaks-field-check.exe` for step 9.

**Option A (simplest).** Every push builds and tests everything on Windows
(the repository is public, so GitHub runs it for free).

1. Open the repository on GitHub, **Actions** tab, the newest green **ci** run
   for the branch `claude/peaktweaks-windows-setup-sw5aub-ax2mx0`.
2. Under **Artifacts**, download **peaktweaks-tester-exe** and
   **peaktweaks-field-check**, and unzip both.

**Option B (build it yourself).** It needs about 10 GB of downloads
the first time.

1. Install, with the default options:
   - Rust: https://rustup.rs
   - Node.js LTS (22 or newer): https://nodejs.org
   - Build Tools for Visual Studio: https://visualstudio.microsoft.com/downloads/.
     In the installer, tick **Desktop development with C++**.
2. Get the code. On GitHub, switch to the branch
   `claude/peaktweaks-windows-setup-sw5aub-ax2mx0`, then **Code > Download ZIP**, and unzip it.
   Or, with Git: `git clone -b claude/peaktweaks-windows-setup-sw5aub-ax2mx0 https://github.com/keganscott/PC-TWEAK-APP`.
3. Open PowerShell (not as administrator) in that folder and run:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\build-tester.ps1
   ```
   It checks the three tools first and says what is missing. The first build
   takes 10 to 20 minutes. Both programs land in the `tester-build` folder.

## 2. First start

Double-click `peaktweaks-tester.exe`.

- **"Windows protected your PC"** (SmartScreen) is expected: the program is not
  code-signed yet (NOTES N36). Click **More info**, then **Run anyway**.
- **"Do you want to allow this app to make changes?"** (UAC) is expected:
  PeakTweaks needs administrator rights for the settings it changes. Click **Yes**.
  *Note whether this prompt appeared* (NOTES N3).
- If your antivirus blocks or quarantines it, note the antivirus name and its
  message. The app writes a copy of PresentMon (Intel's frame-time tool)
  to `C:\ProgramData\PeakTweaks\tools`, which some antivirus programs flag.

If the app shows **"PeakTweaks could not start"**, take a screenshot. The same
reason is saved in `%LOCALAPPDATA%\PeakTweaks\startup-error.log`; send that file too.

## 3. Welcome and Home

1. The welcome appears on first start. Step through it.
2. Check the top bar says **Tester build**.
3. On **Home**, under **Your PC**, compare each tile with what your PC really has.
   Task Manager > Performance shows the processor, memory speed and GPU.
   Settings > System > Display > Advanced display shows the refresh rate.
   Note anything that is wrong or says "Could not tell".
4. Read **What the scan found**. For each item, does it match your PC? Note anything
   that looks wrong.

## 4. Make a restore point (the most important step)

This has never succeeded on a real PC yet (NOTES N24).

If Home already says **"Restore point #... is ready"**, Windows made one in the
last 24 hours (Windows Update often does) and PeakTweaks uses it, so there is no
button to click. In that case, test the creation with the field-check tool
instead: at the end of step 9, run `.\peaktweaks-field-check.exe --create-restore-point`
(it asks you to type `yes`), and note what it printed. It records "Allow a
restore point on demand" like the app does; afterwards, Backups > **Undo all** in
the app (or `.\peaktweaks-field-check.exe --revert-all`) puts that back.
Otherwise:

1. Click **Make a restore point** (on Home, on Tools above the list, or **Make
   one now** in the sidebar). It can take a minute. PeakTweaks already runs as
   administrator, so Windows does not ask again.
2. Note what happened: success ("Restore point #... is ready") or the error text.
3. Check in Windows: Start, type **Create a restore point**, open it, click
   **System Restore...**, **Next**. A point named "PeakTweaks: before changes"
   should be in the list. Click **Cancel**; do not restore.

PeakTweaks also records one change of its own here, **Allow a restore point on
demand** (Windows normally allows one restore point per 24 hours). It is listed
in Backups and is undone by Undo all.

## 5. Apply and undo one change

1. Write down how your mouse is set now. Settings > Bluetooth & devices > Mouse >
   Additional mouse settings > **Pointer Options**: is **Enhance pointer
   precision** ticked?
2. In PeakTweaks, open **Tools**, find **Pointer precision**, click **Apply**.
3. Re-open Pointer Options: **Enhance pointer precision** should now be unticked,
   and the mouse should already move without acceleration.
4. Back in PeakTweaks, click **Undo** on the same card.
5. Re-open Pointer Options: it should be back to what you wrote down in step 1.

If you prefer acceleration off, apply it again and keep it. It stays undoable.

## 5b. A few more changes

Each of these is recorded and undone the same way. Note anything that fails,
reads wrong afterwards, or is confusing.

1. **Tools**, any section: **Apply recommended** applies that section's
   recommended changes in one click. Check a couple of them in Windows, for
   example Settings > Gaming > **Game Mode** or Settings > Personalization >
   Colors > **Transparency effects**.
2. Turn on **Advanced**. Apply **PeakTweaks power plan**, then check Control
   Panel > Power Options: "PeakTweaks" should be the selected plan. Apply
   **Search indexing** (Services) and check `services.msc`: Windows Search
   should be Disabled and stopped.
3. **Connection**: click **Check now**. It pings your router and two public
   DNS servers 20 times each and says where packets were lost, if anywhere. Note
   what it says and whether that matches your connection.
4. **One-time actions**: open **Clear out junk files**. It shows the size of
   each area first and asks once before deleting; you can cancel there.
5. **While you play**: switch **Gaming Mode** on and off once.

## 6. Backups and Undo all

1. Open **Backups**. **Applied now** lists what is in effect from step 5b, and
   "Allow a restore point on demand" if step 4 made a restore point.
2. Click **Undo all** and confirm. **Applied now** should become empty.

## 7. Close and reopen

1. While the app is open, double-click `peaktweaks-tester.exe` again. The second
   window should say **"PeakTweaks is already open"**: only one copy runs at a
   time, so the record of changes stays correct. Close that second window.
2. Close the app and start it again. The welcome should not show again, and
   Backups should show the same record as before.

Close the app before step 9: the field-check tool also refuses to run its
engine part while the app is open.

## 8. Optional: Proof (only if you have a game installed)

Proof records a game before and after a change and compares the two. In
**Proof**, click **New comparison**, pick the game, and follow the steps on the
page. Note anything that fails or is confusing.

## 9. The field-check report

This collects the raw readings behind steps 3 and 4 in one file. Open PowerShell
**as administrator** in the folder with `peaktweaks-field-check.exe` and run:

```powershell
.\peaktweaks-field-check.exe
```

It takes about 30 seconds, changes nothing, and writes a folder
`peaktweaks-field-check-<time>` with `report.json`. docs/FIELD_CHECK.md has the
optional extra steps (frame capture with a game running).

## 10. What to send back

- Your notes from steps 2 to 8 (including 5b), and screenshots of anything wrong or confusing.
- `report.json` from step 9. It can identify the PC (hardware IDs, your Windows
  account's ID), so send it only to whoever works on PeakTweaks.
- `%LOCALAPPDATA%\PeakTweaks\startup-error.log`, if it exists.

## If something goes wrong

- **A change misbehaves:** Backups > **Undo all** puts back the exact values
  PeakTweaks found before each change.
- **Windows itself misbehaves:** Start, type **Create a restore point**, then
  **System Restore...**, and pick the "PeakTweaks: before changes" point.
- **Windows will not start:** Backups > **If Windows will not start** explains
  the offline recovery files in `C:\ProgramData\PeakTweaks\offline`. This path
  has not been tested in the Windows Recovery Environment yet (NOTES N48), so
  System Restore from the recovery screen is the first thing to try.

## Removing it

Run **Undo all** first, then delete the exe. `C:\ProgramData\PeakTweaks` holds
the change record and backups. Delete it only after Undo all shows nothing
applied, because it is what Undo works from.
