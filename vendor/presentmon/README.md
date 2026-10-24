# PresentMon (vendored, not committed)

`PresentMon-x64.exe` is Intel's PresentMon console application, MIT licensed
(`LICENSE.txt` here). The binary itself is **not** committed. `PINNED.json` names
the exact release, its SHA-256, and the signer, and `scripts/fetch-presentmon.ps1`
downloads it and refuses anything that does not match, including a missing or
invalid Authenticode signature.

Release builds embed the file in `peaktweaks.exe` (Cargo feature
`bundle-presentmon`) so that the app stays one portable executable and the
free tier needs no network access. At run time the embedded copy is written to
`%ProgramData%\PeakTweaks\tools\` and its hash is checked again before every
capture.

To bump the version: change `PINNED.json`, run the script, run the tests.

`README-ConsoleApplication.md` is a copy of Intel's documentation for this
release (commit `00db2ca` of GameTechDev/PresentMon, version 2.6.0). A test
checks that every command-line flag PeakTweaks passes appears in it.
