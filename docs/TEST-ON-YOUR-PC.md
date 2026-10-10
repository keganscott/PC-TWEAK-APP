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

## 5c. New since your last test (2026-10-10)

Settings (bottom of the dialog) names the build, for example
"PeakTweaks 0.1.0, build 1a2b3c4". Put that in your notes. Each item below
points at its row in docs/NOTES.md; a "works" or "doesn't" from you closes it.

1. **Home, first run** (N109): before any restore point, Recommended offers
   **Make a restore point, then apply N**. One click should make the point and
   then the changes. While the app is still checking it says "Checking for a
   restore point".
2. **Tray icon** (N108): the icon by the clock. Right-click or click it:
   **Gaming Mode** (ticked when on) and **Open PeakTweaks**. Switch Gaming Mode
   from the tray, then look at Tools > While you play: it should match. Switch it
   in Tools and the tray's tick should follow. With a game running, hovering the
   icon names the game.
3. **Backups**: each change in effect says when it was applied. **Copy setup**
   (N106) puts a short text on the clipboard; pasting it on another PC lists what
   that PC would apply.
4. **After a restart** (N105): restart Windows with a change that needs one
   (its card says it takes effect after a restart) applied. Home should say whether everything is
   still in place.
5. **Games** (N103, N107): pick your main game; its card comes first. On a Steam
   game, **Play** asks Steam to start it without PeakTweaks' administrator rights.
   Games with publisher advice show it with the source named.
6. **Startup apps** (N100): Store apps (Xbox, Slack and so on) are listed and can
   be switched off and on.
7. **Window** (N102): move or resize the window, close, reopen: it comes back
   where it was.
8. **Reminders** (N104): Home mentions junk files after 30 days and an old
   graphics driver after 180; "Not now" hides one for 30 days.
9. **Connection** (N88): on Wi-Fi, Check now also shows the Wi-Fi signal as
   Windows rates it. Compare it with the bars by the clock.
10. **Last game** (N110): with PeakTweaks open, play a game for a few minutes,
   then close it. Tools > While you play shows **Last game**: how long it ran,
   the hottest graphics card reading, and whether the card slowed down for heat
   or power, and how busy the card and processor were and how full memory got
   (N112). Compare the temperature with the NVIDIA overlay if you use it, and
   the memory figure with Task Manager. Graphics card readings are NVIDIA only.
11. **Game history** (N111): after step 10, close PeakTweaks and open it again.
   Tools > While you play still shows that game as **Last game**, and its card
   on **Games** says when it was last played.
12. **Game search and ARC Raiders** (N113): on **Games**, type a few letters in
   **Or another game**; matching games list as you type. With ARC Raiders
   installed through Steam it is marked as on this PC. While you play it, open
   Task Manager > Details: the game should show as `PioneerGame.exe`. If it shows
   another name, send that name.
13. **Gaming Mode button** (N114): the pill at the top of every page and the
   card on Home. Click it on and off; it should glow while on, and Tools > While
   you play should match.
14. **Clean memory** (N115, N120): Tools > Quick tools. The gauge and figures
   should roughly match Task Manager > Performance > Memory. Click **Clean
   memory** and note the "let go" figure. Turn on **Clean memory during games**,
   play for a while, and the game's report in While you play says how often it
   cleaned.
15. **Presets** (N116): Tools > Presets. Open **Review** on each and check the
   list makes sense for your PC. Applying one is optional; Backups can undo it.
16. **New look** (N117, N118): Tools and Games. Say what you like and what you
   don't.
17. **Graphics driver** (N119): Tools > Quick tools > Graphics driver. Check
   the driver number it shows against NVIDIA's app or Device Manager. **Open
   NVIDIA's driver page** should open your browser. Installing a driver is
   optional and takes a few minutes; only do it with a restore point and a
   driver file from NVIDIA you want anyway.
18. **Record while I play** (N121): on **Games**, your main game's card has
   **Record my next games**. Click it, then play as usual: after 2 minutes, and
   every 3 minutes after that, Proof records 30 seconds while the game is the
   window in front. After 3 samples the card says the before side is ready. Make
   your changes in Tools, click **Record the after side**, play again, and Proof
   shows the result with a chart of each run. Note whether the timings suit how
   you play.

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

Proof records a game before and after a change and compares the two. The
easy way is step 18 above. By hand: in **Proof**, click **New comparison**,
pick the game, and follow the steps on the page. Note anything that fails or
is confusing.

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
- If something went wrong: open **Activity log** (top right), click **Copy**
  and paste it with your notes. It starts with the build number.

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
