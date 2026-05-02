# Fisch Macro — Documentation

A controller-driven macro for Roblox's Fisch fishing minigame. Tracks the bar, predicts the fish, presses M1 in proportional pulses (PWM-PD with non-linear gain) to keep the bar locked on the target.

## Documentation pages

1. [Installation](install.md) — download, run, system requirements
2. [First-Run Calibration](calibration.md) — picking the bar region, sampling colors per rod
3. [Detection Cascade](detection.md) — BGR / arrow / template / carryover, when each tier fires
4. [Controller Tuning](controller.md) — `kp`, `kd`, panic mode, slew limiting; when to change what
5. [Auto-Totem & Auto-Potion](auto-features.md) — interval-based hotkey firing with safe-state gating
6. [Troubleshooting](troubleshooting.md) — common failure modes and fixes
7. [Advanced: Per-Rod Tuning](advanced.md) — per-rod color overrides, Hydra reference table, hand-editing config.json

## Quick reference: defaults that ship

| Setting | Default | Why |
|---------|---------|-----|
| `panic_error_threshold` | 100 | Catches dashes early; same-sign exit prevents oscillation |
| `kd` | 0.75 | Damps controller-induced overshoot |
| `kp` | 1.8 | Aggressive enough for fast fish, slew-limited |
| `bar_arrow_color` | `#848587` | Hydra's grey — works for the majority of rods |
| Detection cascade | bgr → bgr_edges → arrow → template → carryover | Color-stable arrow tier handles tint events |
| `cast_ms` | 1000 ms | Fast-turnaround casts |
| `cooldown_ms` | 500 ms | Short cycle gap |
| `auto_totem.enabled` | off | Toggle on after configuring interval |
| `auto_potion.enabled` | off | Toggle on after configuring interval |

These are the values from the [reference 130-cycle run](#) (96 % catch rate, mean abs error 37 px, panic episode avg 98 ms). Most users shouldn't need to touch them.

## Reporting issues

[Open a GitHub issue](https://github.com/FemPoof/fisch-macro/issues) with:

- The cycle summary line from your latest log (`Reel end: ...`)
- A screenshot of the bar's appearance during a problem cycle if relevant
- Your config file (strip the `license_key` and `license_binding` lines first)

The more cycle telemetry you include, the easier it is to diagnose.
