@echo off
rem Double-click to run the remaining PeakTweaks PC checks (scripts\test-on-pc.ps1).
rem Windows asks for admin once; everything after that runs in the one window.
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','\"%~dp0test-on-pc.ps1\"'"
