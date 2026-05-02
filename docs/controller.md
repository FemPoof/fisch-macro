# Controller Tuning

The bar is driven by a PD controller with non-linear proportional gain, panic mode for emergencies, and slew limiting for damping. All tunables live in **Main** tab → **Control** card.

## Defaults

| Parameter | Default | Effect |
|-----------|---------|--------|
| `kp` | 1.8 | Proportional gain. Higher = faster response, more overshoot risk |
| `kd` | 0.75 | Derivative damping. Higher = less overshoot, slower transients |
| `kp_exponent` | 1.2 | Non-linear gain shape. >1 = soft on small errors, aggressive on large |
| `pwm_cycle_ms` | 25 | M1 press/release cycle time |
| `pwm_max_slew` | 0.5 | Max duty change per controller tick (anti-overshoot) |
| `panic_error_threshold` | 100 | Pixel error that triggers bang-bang panic mode |
| `panic_exit_ratio` | 0.7 | Panic stays latched until \|err\| < 100 × 0.7 = 70 |
| `fish_lookahead_ms` | 50 | How far ahead to predict fish position |
| `fish_x_ema_alpha` | 0.85 | Smoothing for fish position |
| `deadband_pixels` | 3 | Errors below this are treated as zero |

These values produced the reference 130-cycle run at 96 % catch rate. Most rods don't need any change.

## Reading cycle telemetry

Each `Reel end:` log line includes per-cycle stats:

```
mean_abs=37 peak_err=+138/-122 panic_ep=2(max=212ms,avg=98ms)
```

- **`mean_abs=N`** — average absolute pixel error. < 50 is healthy.
- **`peak_err=+X/-Y`** — worst-case errors during the cycle in each direction.
- **`panic_ep=N(max=Xms,avg=Yms)`** — panic episode count and durations. Lower `avg` is better; > 200 ms means panic is dragging on.

## When to change what

### Bar overshoots dashes

Symptom: peak_err is high (200+) and `panic_ep avg ms` is climbing.

**Try (in order):**

1. Raise `kd` to 0.85 (more damping)
2. Drop `pwm_max_slew` to 0.4 (slower per-tick duty changes)
3. Drop `kp` to 1.6 if still overshooting

### Bar feels sluggish chasing fast fish

Symptom: peak_err is high and the cycle is winning slowly.

**Try (in order):**

1. Lower `panic_error_threshold` to 80 (panic engages earlier)
2. Raise `kp` to 2.0 (more aggressive PD)
3. Raise `pwm_max_slew` to 0.65 (faster per-tick changes)

### Bar oscillates around the target

Symptom: bar visibly twitches even when fish isn't moving fast.

**Try (in order):**

1. Raise `kd` to 0.85
2. Raise `deadband_pixels` to 5 (tolerate more error before reacting)
3. Drop `kp` to 1.6

### Long cycles where the bar never catches up

First check: is `mean_abs` < 50 in the cycle summary? If yes, the controller is fine — the issue is detection. Check `via=[...]` for tier engagement; recalibrate colors if BGR share is below 95 %.

If `mean_abs` is consistently > 60 even on simple fish, controller is the bottleneck. Try raising `kp` to 2.0.

## Panic mode

Panic mode bypasses PD math when error is extreme. The controller slams duty to 0 or 1 (full release / full press) for the duration. The v2.1.8 same-sign exit gate prevents the classic "panic re-engages on opposite side" oscillation by requiring the projected error to stay on the same side of zero before exiting panic.

### When to lower panic threshold

`panic_error_threshold: 100` (default) catches most dashes. Lower to **80** for:

- Catching legendary-tier fish that escape on harder dashes
- Rods with very fast bar physics (Tryhard, Masterline)

Risk: panic engages on smaller errors, briefly disrupting smooth tracking. The same-sign exit gate keeps this contained — episodes stay short (~100 ms) — so the trade is usually worth it.

### When to raise panic threshold

`panic_error_threshold: 130` (legacy) is appropriate when:

- The bar feels twitchy even with `kd: 0.85` — panic is engaging on noise
- You're catching only common fish and don't need dash-handling

Or just disable panic entirely with `panic_error_threshold: 0`.

## Adaptive tuning

The macro tracks per-cycle capture rate (in `hz`) and auto-adjusts a few control params when hz drops below 120. Specifically: `pwm_cycle_ms` extends, `fish_x_ema_alpha` shifts, slew tightens. This keeps the controller behaving sensibly at low frame rates.

Toggle in **Extra** tab → **Settings** → **adaptive_tuning_enabled**. Default on.

## Per-rod overrides

Every parameter on this page can be overridden per-rod. Switch rods in **Main** tab → **Configuration** card; the **Control** card auto-loads from the selected rod's tuning. Changes save back to that rod only.

To copy tuning between rods, see [advanced.md](advanced.md).
