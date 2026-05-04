# Fisch Macro

A controller-driven macro for the Roblox game *Fisch*. Tracks the catch-bar
minigame via DXGI Desktop Duplication, predicts fish movement, and drives
mouse input through a PWM-PD controller with non-linear gain to keep the bar
locked on target.

> **Status:** v2.4.0 stable + Perfect Cast (experimental).
> Reel/control pipeline is production-ready and 1440p-calibrated out
> of the box; an overnight 3864-cycle run on v2.4.0 hit a 97.4%
> catch rate. Perfect Cast is opt-in and needs per-setup tuning —
> see [docs/perfect-cast.md](docs/perfect-cast.md). Other resolutions
> need a one-time bar-region re-pick via the in-app F1 calibrator.

> **Disclaimer:** Automation tools may violate Roblox's Terms of
> Service. Use at your own risk. The authors are not responsible for
> account action taken against users.

---

## Highlights

- **Live-tint sampling** (v2.4.0) — Fisch renders two small triangles
  flanking the catch bar whose color tracks the bar's *current* danger
  tint exactly. The macro samples them every frame and feeds the color
  into BGR detection, eliminating the need to pre-configure static
  danger-color lists. Calibrate via the F1 zone overview.
- **5-tier detection cascade** — BGR → bgr_edges → arrow → template →
  carryover. The arrow tier tracks a fixed-color UI element on the
  bar's edge; the live-tint sampler keeps BGR matching through every
  health-tied tint state.
- **PWM-PD controller** with non-linear gain (`kp_exponent`),
  derivative damping (`kd`), velocity-aware panic mode, and same-sign
  panic exit (prevents oscillation amplification).
- **DXCAM / BetterCam** dual-backend capture, typically 90-150 Hz.
- **Per-rod tunings** — separate profile per rod with calibrated
  colors, control parameters, and arrow indicator color. The default
  config seeds 27 rod presets (Default, Tryhard, Fabulous, Nates,
  Requiem, Thalassar, Blade of Glorp, plus 14 arrow-less rods).
- **Auto-totem & auto-potion** — interval-based hotkey firing with
  safe-state gating (only fires between fishing cycles, never mid-reel).
- **Auto rod-equip detector** — periodic hotbar-slot check presses
  the rod hotkey only when detection sees the rod is no longer
  selected. Robust to whatever combination of timer threads / focus
  changes / lost keystrokes left the rod missing.
- **Auto-reconnect** — detects Roblox disconnect overlays and runs a
  recovery flow (click Reconnect → wait → mystic-mirror back to
  fishing spot).
- **Unified F1 zone calibrator** — every screen region the macro can
  use is calibrated in one place. Cursor magnifier (press M) shows
  the exact pixel that will be recorded, so what you outline is what
  gets saved.
- **Perfect Cast (experimental)** — camera-force sequence drives zoom
  + look + M1 hold + detection-based release. Needs per-setup tuning;
  see [docs/perfect-cast.md](docs/perfect-cast.md).
- **Live tracker HUD** — bar position, fish marker, error magnitude.
- **Adaptive runtime tuning** — auto-scales select control params at
  low capture rates so the controller stays well-behaved when GPU
  thermals throttle Roblox's render rate.
- **Hardware-bound license keys** — single-use per machine; binding
  fingerprint baked at activation.
- **Auto-updater** — checks GitHub Releases on launch, one-click install.

## Recent results

Overnight unattended run on v2.4.0 (1440p, BetterCam, default rod):

| Metric | Value |
|--------|-------|
| Cycles | 3,864 |
| **Catch rate** | **97.4 %** |
| Escape rate | 0.5 % |
| Median cycle duration | 8.7 s |
| Capture rate | 135 Hz median |
| BGR detection share | 96.8 % of frames |
| Live-tint sampler fire rate | 100 % |

---

## What's new in v2.4.0

- **Live-tint sampling** — replaces the old static `danger_bar_color`
  field. Two small triangles flank the bar in-game; their color
  tracks the bar's current tint exactly, including health-tied dark
  states the static list could never anticipate. The macro samples
  every frame and feeds the color to BGR detection. See
  [docs/calibration.md](docs/calibration.md) for the F1 calibration
  walkthrough.
- **Unified F1 zone calibrator** — single dialog calibrates Fish Bar,
  Live Tint, Catch Progress, Perfect Cast Meter, Auto Rod-Equip, and
  Auto Reconnect zones. Per-tab "Pick Region" buttons removed.
- **Cursor magnifier in pickers** — press M inside any region picker
  to toggle a 6× zoomed inset that follows your cursor and outlines
  the exact pixel that will be recorded. Pixel-perfect placement
  without squinting.
- **Live-tint preview swatch** on the Fish tab updates in real time
  while the macro runs, giving visual confirmation the sampler is
  reading the right pixels.
- **Color swatches everywhere** — every hex color field has a live
  swatch that updates as you type.
- **Rod presets** — 27 rods seeded by default with their documented
  arrow colors (Tryhard, Fabulous, Nates, Requiem, Thalassar, Blade
  of Glorp, etc.). Pre-existing user configs gain the missing rods
  on next load without overwriting tuned values.
- **Threading hardening** — `ConfigManager.save` now snapshots
  settings under a lock before serializing, fixing a race where the
  state-machine worker thread's `bar_physics_*` writes could collide
  with the main thread's JSON dump and crash the GUI.
- **GUI cleanup** — removed the redundant Stats tab, a broken
  "Equip Rod Before Each Recast" toggle, the Active Totem Detection
  card, and per-tab region pickers (centralized into F1).
- **Faulthandler** enabled at startup — if the GUI ever crashes,
  `faulthandler.log` next to the regular session log captures a
  Python-level stack trace.

For older release notes see the commit history.

---

## Quick start

1. Download the latest release from [GitHub Releases](https://github.com/FemPoof/fisch-macro/releases)
2. Unzip and run `fisch-macro.exe`
3. Press **F1** anywhere to open the unified **Calibrate Zones** overview, then **Edit** the *Fish Bar* row and drag a tight rectangle around the catch-bar in Fisch
4. Press **F3** (or click Start) to begin fishing
5. Press **F3** again to stop

That's it — defaults are tuned for the reference run conditions and work
on most rods without further calibration.

For per-rod color sampling, hotkey rebinding, and advanced tuning, see
[docs/](docs/).

> **Heads up about antivirus.** A handful of AV heuristics flag the
> packaged EXE because it's Nuitka-packed Python with input-simulation
> APIs. None of them are signature-based detections of actual
> malicious behavior — every reputable signature engine clears the
> binary. See [docs/FALSE_POSITIVES.md](docs/FALSE_POSITIVES.md) for
> the full breakdown, the SHA-256 to verify against, and a
> run-from-source path if you don't want to whitelist.

---

## Install (from source, dev mode)

Requires Python 3.11+ on Windows 10/11.

```powershell
git clone https://github.com/FemPoof/fisch-macro.git
cd fisch-macro
py -3.11 -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -e ".[dev]"
python -m fisch_macro
```

To run the test suite:

```powershell
python -m pytest -q
```

---

## Architecture

Three-thread pipeline:

```
   Capture thread (DXCAM/BetterCam)         ← grabs frames
        │
        ▼
   State machine + Controller thread       ← detects bar, runs PWM-PD math
        │
        ▼
   Input thread (m1 press/release)          ← drives mouse via Win32
```

Auto-totem and auto-potion are independent timer threads that share the
state machine's stop event and check the state for a "safe to swap items"
signal before firing.

The detection cascade lives in `fisch_macro.core.detection` (BGR /
bgr_edges / edge / HSV) plus three sibling modules:

- `fisch_macro.core.arrow_detection` — color-stable fallback
- `fisch_macro.core.bar_template` — correlation-based fallback
- `fisch_macro.core.bar_tracker` — CSRT/MIL appearance tracker (default off)

---

## Documentation

| Page | Topic |
|------|-------|
| [Install](docs/install.md) | Download, system requirements, first launch |
| [Calibration](docs/calibration.md) | Bar region pick, per-rod color sampling |
| [Detection](docs/detection.md) | Cascade tiers, when each fires, telemetry |
| [Controller](docs/controller.md) | kp / kd / panic / slew tuning |
| [Auto-features](docs/auto-features.md) | Auto-totem + auto-potion + auto rod-equip |
| [Perfect Cast (experimental)](docs/perfect-cast.md) | Camera-force sequence + fill-%-based release; needs tuning |
| [Troubleshooting](docs/troubleshooting.md) | Common failure modes |
| [Advanced](docs/advanced.md) | Per-rod overrides, arrow-color reference table |
| [Antivirus false positives](docs/FALSE_POSITIVES.md) | Why some AVs flag the EXE; how to verify and what we're doing |

---

## Contributing

Tests live in `tests/`. Run `python -m pytest -q` before opening a PR.
The test suite is fast (~15 s) and covers the controller math, detection
tiers, license validation, capture pipeline, and config loading.

For a major change (new detection tier, controller architecture shift,
new auto-feature):

1. Open an issue first describing the proposed change and its motivation
2. Cite cycle-summary metrics from a real log if you're claiming a
   detection or controller improvement
3. Ship tests covering the new behavior — every existing tier has a
   `tests/test_<module>.py` to mirror

For minor changes (bug fixes, documentation, telemetry tweaks): direct PR
is fine.

---

## License

Proprietary. See `LICENSE`. Premium features (auto-totem, auto-potion,
auto-reconnect) require a license key; the macro's free tier covers the
core detection + controller pipeline.
