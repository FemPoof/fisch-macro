# fisch-macro

Windows automation for the Roblox game *Fisch*. Tracks the catch-bar minigame via DXGI Desktop Duplication, predicts fish movement, and drives mouse input through a PWM-PD controller to keep the bar locked on target.

> ⚠️ **Disclaimer:** Automation tools may violate Roblox's Terms of Service. Use at your own risk. The authors are not responsible for any account action taken against users of this software.

## Download

Grab the latest release from the [**Releases**][releases] tab. Download `fisch-macro.exe` and run it — no installer needed.

[releases]: https://github.com/FemPoof/fisch-macro/releases/latest

**System requirements:** Windows 10 or 11, 64-bit. No admin rights needed. No .NET, no Python install, no Visual C++ runtime — everything is bundled.

## First-time setup

1. Launch `fisch-macro.exe`.
2. Open the **Fish** tab → *Capture Region* card.
3. Click **Pick Region (drag on screen)**.
4. Drag a tight box around the in-game fishing bar.
5. Press **Enter** to accept.

That's it. Press **F3** to start / stop the macro.

For per-rod color sampling, hotkey rebinding, and advanced tuning, see [docs/](docs/).

## What's new in v2.2.0

- **5-tier detection cascade** — BGR → bgr_edges → arrow → template → carryover. The arrow tier (Hydra-style) tracks a fixed-color UI indicator on the bar's edge that doesn't tint during fish dashes, making it a reliable color-anchored fallback when BGR fails.
- **PWM-PD controller** with non-linear gain, derivative damping, velocity-aware panic mode, and same-sign panic exit (prevents the oscillation re-engagement loop that drove deep outliers in pre-2.2 builds).
- **Auto-totem & auto-potion** — interval-based hotkey firing with safe-state gating (only fires between fishing cycles, never mid-reel).
- **Live capture-rate health** — Stats tab now shows current / min / mean Hz with color thresholds so capture-rate drift is visible at a glance.
- **Per-tab Reset to Defaults** — Main / Cast / Fish / Totem each have their own reset; license, hotkeys, and other tabs untouched.
- **Multi-file documentation** — see [docs/](docs/) for install, calibration, detection, controller, auto-features, troubleshooting, and advanced guides.

## Reference performance (130-cycle run, lowest-graphics 1440p)

| Metric | Value |
|--------|-------|
| Mean abs error | 37 px |
| Mean panic episode | 98 ms |
| Outliers > 200 px | 11.4 % |
| Outliers > 300 px | 1.4 % |
| Capture rate | 115-130 Hz |
| Cycles / hour | ~390 |

## Default hotkeys

| Key | Action |
|---|---|
| `F3` | Start / stop |
| `F5` | Pause |
| `1` | Equip fishing rod |
| `2` | Equipment bag |
| `3` | Special totem |
| `4` | Sundial |
| `5` | Potion |
| `7` | Mystic mirror |

All hotkeys are rebindable from the **Main** tab → *Hotkey Configuration* card.

## What the app does

- **DXGI Desktop Duplication** screen capture (DXCAM or BetterCam — switch in Main tab settings)
- **PWM-PD controller** with non-linear gain, derivative damping, velocity-aware panic mode, slew limiting, and same-sign panic exit
- **5-tier detection cascade** — BGR / bgr_edges / arrow / template / carryover
- **Per-rod tunings** — separate profile per rod with calibrated colors, control parameters, and arrow indicator color
- **Auto-totem** + **auto-potion** with safe-state gating
- **Live tracker HUD** — bar position, fish marker, error magnitude
- **Adaptive runtime tuning** auto-scales control params at low capture rates
- **Auto-updater** checks GitHub Releases on launch; one-click install with SHA-256 verification

## Documentation

| Page | Topic |
|------|-------|
| [Installation](docs/install.md) | Download, system requirements, first launch |
| [Calibration](docs/calibration.md) | Bar region picking, per-rod color sampling |
| [Detection](docs/detection.md) | Cascade tiers, when each fires, telemetry |
| [Controller](docs/controller.md) | kp / kd / panic / slew tuning |
| [Auto-features](docs/auto-features.md) | Auto-totem + auto-potion |
| [Troubleshooting](docs/troubleshooting.md) | Common failure modes |
| [Advanced](docs/advanced.md) | Per-rod overrides, Hydra reference table |

## Where my settings live

```
%LOCALAPPDATA%\fisch-macro\config.json
```

Holds your calibration, per-rod tunings, hotkey rebinds, and license key. Reset buttons in each tab are scoped to that tab's domain — your license key is never wiped.

## Privacy & security

- **No telemetry, no analytics, no phone-home.** The optional online license check (off by default) is the only network call the app makes; everything else runs locally.
- **License keys are hardware-bound.** Your key is computed against a SHA-256 fingerprint of your machine's GUID + MAC + CPU on activation; sharing your key with someone else doesn't unlock features on their PC.
- **Hotkeys use Windows' `RegisterHotKey` API** — this is *not* a keylogger; it fires only for the keys you rebind and only while the app is running.
- See [SECURITY.md](SECURITY.md) for the full security model.

## Quick troubleshooting

**Bar isn't being detected** → re-pick the capture region (Fish tab → Pick Region) with a tighter box. See [docs/troubleshooting.md](docs/troubleshooting.md) for more.

**Macro feels laggy / capture rate dropped** → close other GPU-using apps (Discord calls with screen share, OBS, browsers with hardware acceleration). Check the **Stats** tab → *Capture Health* card → if **Min Hz** is red (< 80), something is competing for the GPU. Also confirm Roblox is in Windowed or Fullscreen Borderless (true fullscreen blocks DXGI capture).

**Auto-totem fires once and then stops** → detection-zone calibration issue. Either disable detection (Totem tab → uncheck *Detection enabled*) or recalibrate the active color on a different pixel. See [docs/troubleshooting.md](docs/troubleshooting.md).

**Bar overshoots dashes** → bump `kd` to 0.85 or drop `panic_error_threshold` to 80 in the Fish tab → Control card. See [docs/controller.md](docs/controller.md).

## License

Closed-source. The downloadable `.exe` is licensed for personal, non-commercial use. Redistribution, reverse-engineering, and removal of the license-key check are prohibited. See [LICENSE](LICENSE) for full terms.

## Support

Open a [GitHub issue][issues] for bugs or feature requests. Include the relevant cycle summary line from your latest log and any screenshots if applicable.

[issues]: https://github.com/FemPoof/fisch-macro/issues
