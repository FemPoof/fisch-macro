# Troubleshooting

Common failure modes and their fixes. If your issue isn't here, open a [GitHub issue](https://github.com/FemPoof/fisch-macro/issues) with your latest cycle summary line attached.

## Macro won't start

### "Calibrate the fish bar region first."

You haven't picked the region yet. **Press F1** to open Calibrate Zones, then click **Edit** on **Fish Bar (the minigame catch bar)**. See [calibration.md](calibration.md).

### "Could not capture a frame."

DXCAM / BetterCam can't read your screen. Check:

- Roblox is in **Windowed** or **Fullscreen Borderless** mode (true fullscreen blocks DXGI capture).
- No HDR / variable refresh rate on the monitor (occasionally interferes with DXGI).
- Try the other capture backend: **Extra** tab → switch `capture_mode` from DXCAM to BetterCam.
- If still failing, check Windows Defender / your antivirus — some flag DXGI capture as suspicious.

### Roblox window keeps stealing focus mid-run

Disable **Auto-focus Roblox** and **Auto-maximize Roblox** in the **Extra** tab if your setup doesn't need them. They're meant for single-monitor setups; on multi-monitor they can fight your other windows.

## Bar isn't being detected

### Cycle summaries show low `detect=` percentage (< 60 %)

Your bar colors are off. Most likely cause: graphics setting changed since calibration, so the bar's rendered pixels shifted slightly and BGR matching is failing on the edges.

**Fix**: open Fisch with the rod equipped, get a bar visible, and re-sample colors via **Fish** tab → **Color Options** card → **Sample colors from a captured frame** row. Click **Bar (left)**, then click on the bar's left-edge pixel; repeat for **Bar (right)**.

### `via_bgr` share is < 95 %

Same issue as above. Recalibrate `left_bar_color` / `right_bar_color`.

### `phantoms` count is high (> 50 per cycle)

Your bar region includes panel chrome that's matching as a "phantom" bar. Re-pick the region with a tighter selection that excludes the panel borders.

### `detect=` percentage is 70-80 % even on a clean run

Expected on **lowest Roblox graphics**. The bar's white pixels are rendered with less anti-aliasing at low graphics, so the BGR cascade catches partial frames where it would normally hit clean ones. The arrow tier (fixed-color indicator on the bar's edge) absorbs the gap — look at `arrow=engaged=Nf` in the cycle summary; if `N` is non-zero, the cascade is working as designed.

Field-tested on the 2026-05-02 230-cycle BetterCam session at lowest graphics:

- mean detect % = 73, but mean abs error = 39 px (basically identical to the v2.2.0 reference run's 37 px on full graphics)
- arrow tier rescued ~130 frames/cycle on 24 cycles where BGR couldn't keep up

**`mean_abs` error is the better health indicator on low-graphics setups** — if it's still under 50 px the controller is fine, regardless of `detect=`. If `mean_abs` climbs above 60 too, recalibrate `left_bar_color` / `right_bar_color` and `bar_arrow_color` from a freshly-captured frame at your current graphics setting.

## Bar is detected but the controller fights it

### Bar oscillates around the target

Controller is overcorrecting. **Try (in order):**

1. Raise `kd` to 0.85 in the **Control** card
2. Raise `deadband_pixels` to 5
3. Drop `kp` to 1.6

See [controller.md](controller.md) for full tuning guidance.

### Bar overshoots dashes hard

Panic mode might be re-engaging. Check `panic_ep avg ms` in the cycle summary — if > 200 ms, panic is dragging on.

**Try**: lower `panic_error_threshold` from 100 to 80 (engage earlier, recover faster). The same-sign exit gate prevents oscillation amplification.

### Long cycles where bar never catches the fish

First check: is `mean_abs` < 50 in the cycle summary? If yes, the controller is fine — the issue is detection. Check `via=[...]` for tier engagement.

If `mean_abs` is consistently > 60 even on simple fish, controller is the bottleneck. Try raising `kp` from 1.8 to 2.0.

## Auto-totem / auto-potion misbehavior

### Auto-totem fires once and then stops firing

Cycle is mid-run when the next interval expires; the runner waits for a "safe to swap items" window before firing. If the macro is in `Reel`/`Cast`/`WaitForBite`, the next totem fire is delayed until cooldown. Looks like "stopped firing" but is just waiting — check the Buffs tab for **Next fire in** countdown.

### Auto-potion never fires

Most common cause: the **Potion** hotkey isn't bound. Check **Main** tab → **Hotkey Configuration** card → confirm the **Potion** field has a key (default `5`).

Also check the **Buffs** tab → **Auto Potion** card → **Enabled** is on.

### Item swap mid-reel breaks cycles

This shouldn't happen — both runners gate fires on safe state. If it's happening, your `safe_check` is reporting safe when the macro is mid-cycle. File an issue with your latest log; we'll look at the state machine transitions.

## Auto rod-equip detector reports "rod missing" but pressing doesn't fix it

If the macro's log shows entries like:

```
[WARNING] Rod-equip: backing off for 30s after 2 consecutive presses
didn't take effect. Roblox may be unfocused, the keypress isn't
reaching the game, or your detection zone / equipped color may need
recalibration.
```

it means the detector pressed `1` but the post-press detection still
read "rod missing" — so the keypress never landed. Most common causes:

1. **Roblox lost foreground focus.** Pydirectinput sends keypresses
   to whichever window has focus. If you alt-tabbed to a different
   app, the rod hotkey goes there instead of Roblox. Re-focus
   Roblox; the back-off automatically clears on the next macro cycle
   (long unsafe windows count as "the situation may have resolved").
   Enable **Auto-focus Roblox** in the **Extra** tab to make the
   macro pull Roblox to the front automatically when it starts.
2. **Detection zone / equipped color mis-calibrated.** When Roblox
   is unfocused, the hotbar dims and the slot border color shifts.
   If you calibrated the equipped color while Roblox was focused,
   the dimmed-state color won't match — every poll reads "missing"
   even when the rod is actually held. Re-pick the detection zone +
   re-sample the equipped color, ideally while Roblox is in the
   foreground state the macro will run in.
3. **The hotbar slot moved.** If you rearranged your hotbar, slot 1
   may not contain the rod anymore. Check **Main → Hotkeys → Fishing
   Rod** — the macro presses whatever is bound there, not always `1`.

Until the underlying cause is fixed, **the back-off prevents the
detector from spam-pressing into a void** (or worse: toggling the
rod off if focus comes back briefly between presses). It cools off
for 30 seconds, or earlier if the macro stays in cycle for a while
(safe-state transitions reset the back-off).

If you're seeing the "rod un-equipped, macro not fishing" pattern
without the back-off log line, the detector may not be catching it —
file an issue with your latest log.

## Lowest-graphics setups

Running Roblox on lowest graphics ("Manual graphics quality 1") changes
the bar's rendered appearance enough that some default thresholds
become too strict. **The macro still works**, but expect:

- **`detect %` averaging 70-80** instead of the documented 95-99 healthy
  band. The arrow tier rescues most failure frames; cycle outcomes are
  unaffected. **Use `mean_abs` as the better health indicator on low
  graphics** — anything under 50 px is fine regardless of detect %.
- **Higher no-bite rate** — the bar's first-frame appearance has fewer
  cleanly-rendered white pixels, so the 3-frame consensus gate is
  harder to satisfy within the 15 s wait window. Field-tested on
  lowest graphics: ~18 % no-bite rate vs the ~2 % reference run on
  full graphics.
- **Higher stale-frame rate** — Roblox renders at a lower (and more
  variable) FPS than the macro captures at. The cycle telemetry's
  `fresh=N stale=K` will show 25-35 % stale on lowest. Doesn't impact
  control quality.

### Tuning to recover throughput on lowest graphics

If you're losing 15-20 % of casts to no-bite timeouts:

1. **Lower `bite_consensus_frames` from 3 to 2** in your rod tuning.
   Hand-edit `%LOCALAPPDATA%\fisch-macro\config.json` and add
   `"bite_consensus_frames": 2` to the active rod's tuning. Costs a
   small false-bite-risk delta; the `bite_require_bgr=True` gate
   (default on) keeps single-frame UI matches from triggering.
2. If still bad, **lower `bite_min_width` from 200 to 160** — narrower
   detected bars get accepted at first sight. Valid because lowest-
   graphics anti-aliasing renders the bar slightly thinner.
3. If you have GPU headroom, **raise Roblox graphics one notch**. The
   `mean_abs` is already healthy, so existing control tuning carries
   over without re-calibration.

## Performance

### Capture rate drops mid-session

Roblox's render rate is the upper bound on DXCAM's fresh-frame rate. If your GPU thermals throttle or you're on graphics 7+, expect 60-90 hz. The adaptive tuner (Extra tab → `adaptive_tuning_enabled`) auto-scales controller params to match low-hz captures.

### Drop graphics setting for higher capture rate

Lowering Roblox's graphics quality often boosts capture rate by 30-50 %. The macro's BGR detection is robust to render-quality drift, but if you change graphics, recalibrate bar colors.

### Macro feels laggy / GUI unresponsive

The macro pipeline runs on background threads; the GUI shouldn't be affected. If the GUI is laggy specifically, check whether you have **debug_log_enabled** on (Extra tab) — that floods the log with per-tick debug data and can slow disk I/O on slow drives. Disable it for normal use.

## License / activation

### "License key already used on another device"

Each license key can only be activated on one machine. The single-use enforcement means once you've activated on Machine A, the same key fails on Machine B.

To migrate:

1. On the old machine, **Extra** tab → **Reset to Defaults** to clear the license binding (this preserves the license key).
2. On the new machine, enter the key in the **License** field and click **Activate**.

If you've lost access to the old machine, contact support with your license key — they can manually re-bind.

### Activated but features still locked

Restart the macro. The license check happens at startup; toggling features on requires a fresh launch.

### Want a developer / multi-machine key

There's a special **dev key** that bypasses the single-use enforcement. Contact the project maintainer to get one issued.
