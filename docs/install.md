# Installation

## Download

Grab the latest release from [the releases page](https://github.com/FemPoof/fisch-macro/releases) and unzip anywhere on your machine.

## System requirements

- **Windows 10 / 11 64-bit** — DXGI Desktop Duplication is the capture path; that's a Windows API.
- **Python is bundled** in the release; no manual install needed.
- **Roblox** must run in **Windowed** or **Fullscreen Borderless** mode. True fullscreen blocks DXGI capture.

The macro works at any monitor resolution; it auto-calibrates to whatever bar region you pick. 1080p and 1440p are well-tested; ultra-wide and 4K should work but expect to recalibrate the bar region.

## First launch

Run `fisch_macro.exe`. The configuration GUI opens. **The macro itself does not start until you press the start hotkey or click Start.**

The first launch is harmless — you can browse the tabs, change settings, and explore without firing any keystrokes. No fishing happens until the macro is explicitly started.

## What ships out of the box

- Default rod tuning (kp=1.8, kd=0.75, panic=100, etc.) calibrated against current Fisch UI
- Detection cascade pre-configured: `bgr → bgr_edges → arrow → template → carryover`
- Hotkeys mapped to the standard inventory layout (rod=1, totem=7, potion=8)
- Auto-totem and auto-potion both off (toggle on after configuring intervals)

The only mandatory first-run task is calibrating the **fish bar region** — see [calibration.md](calibration.md).

## Updating

The macro checks GitHub Releases on launch. When a new version is available, a banner appears in the **Extra** tab → **Updates** card. Click "Download && Install" to update in place.

To pin to your current version (skip auto-updates), set `auto_check_updates: false` in the **Extra** tab.

## Uninstall

The macro doesn't install Windows-side; it's a portable folder. Delete the unzipped directory plus `%LOCALAPPDATA%\fisch-macro\` (where the config and logs live) to fully remove.
