# fisch-macro

Windows automation for the Roblox game *Fisch*. Tracks the fishing
mini-game bar and drives mouse input to keep the target line centered.

> ⚠️ **Disclaimer:** Automation tools may violate Roblox's Terms of
> Service. Use at your own risk. The authors are not responsible for
> any account action taken against users of this software.

## Download

Grab the latest release from the [**Releases**][releases] tab.
Download `fisch-macro.exe` and run it — no installer needed.

[releases]: https://github.com/FemPoof/fisch-macro/releases/latest

**System requirements:** Windows 10 or 11, 64-bit. No admin rights
needed. No .NET, no Python install, no Visual C++ runtime — everything
is bundled.

## First-time setup

1. Launch `fisch-macro.exe`.
2. Open the **Fish** tab → *Capture Region* card.
3. Click **Pick Region (drag on screen)**.
4. Drag a tight box around the in-game fishing bar.
5. Press **Enter** to accept.

That's it. Press **F3** to start/stop the macro.

## Default hotkeys

| Key | Action |
|---|---|
| `F3` | Start / stop |
| `F5` | Pause |
| `1` | Equip fishing rod (in-game) |
| `2` | Equipment bag |
| `3` | Special totem |
| `4` | Sundial |
| `5` | Potion |
| `7` | Mystic mirror |

All hotkeys are rebindable from the **Main** tab → *Hotkeys* section.

## What the app does

- **150 Hz screen capture** of just the fishing-bar region (DXCAM or
  BetterCam — switch in settings).
- **PWM controller** with predictive lookahead and panic-mode bypass
  for fast fish bolts.
- **HSV-band detection** for translucent / health-tinted "danger"
  bars that simple color matching misses.
- **Per-rod tunings** — separate profile per rod, all saved
  automatically.
- **Live tracker HUD** showing detection status, bar center, fish
  position, and error in real time.
- **Auto-updates** — when a new version lands, the app prompts you
  in the Extras tab. One click downloads, verifies (SHA-256), and
  restarts. Your config + license stay intact across updates.

## Where my settings live

`%LOCALAPPDATA%\fisch-macro\config.json`

This file holds your calibration, per-rod tunings, hotkey rebinds, and
license key. Reset to Defaults from the *Extras* tab wipes tunings but
**preserves your license key**.

## Troubleshooting

**The macro doesn't see the bar.** Re-pick the capture region (Fish
tab → Pick Region) and make sure the box tightly hugs the bar.

**False bites firing on UI elements.** Tighten the capture region
height — the picker often grabs a few pixels of arrows / icons above
or below the bar.

**Macro caught fish but spammed clicks afterward.** Increase
`bar_lost_timeout_ms` in the *Cast* tab if your bar tints to "danger"
mid-reel and disappears momentarily.

**Macro not detecting in danger / health-tint state.** Open the *Fish*
tab → *Color Options* card and either paste a color sample to derive
HSV, or manually tune the HSV ranges.

**Capture backend issues on BetterCam.** Switch to DXCAM in the *Main*
tab → *Capture* dropdown.

## Privacy & security

- **The app never phones home.** No telemetry, no auto-updates, no
  analytics.
- **License keys are validated locally.** Your key never leaves the
  machine.
- **Hotkeys use Windows' `RegisterHotKey` API.** This is *not* a
  keylogger — it only fires for the keys you rebind, and only while
  the app is running.
- See [SECURITY.md](SECURITY.md) for the full security model.

## License

This software is closed-source. The downloadable `.exe` is licensed
for personal, non-commercial use. Redistribution, reverse-engineering,
and removal of the license-key check are prohibited. See
[LICENSE](LICENSE) for full terms.

## Support

Open a [GitHub issue][issues] for bugs or feature requests.

[issues]: https://github.com/FemPoof/fisch-macro/issues
