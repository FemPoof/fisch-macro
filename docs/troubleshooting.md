# Troubleshooting

Common failure modes and their fixes. If your issue isn't here, open a [GitHub issue](https://github.com/FemPoof/fisch-macro/issues) with your latest cycle summary line attached.

## Macro won't start

### "Calibrate the fish bar region first."

You haven't picked the region yet. **Main** tab → **Capture Region** card → **Pick Region**. See [calibration.md](calibration.md).

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

**Fix**: open Fisch with the rod equipped, get a bar visible, and re-sample colors via **Main** tab → **Sample Colors** row. Click **Bar (left)**, then click on the bar's left-edge pixel; repeat for **Bar (right)**.

### `via_bgr` share is < 95 %

Same issue as above. Recalibrate `left_bar_color` / `right_bar_color`.

### `phantoms` count is high (> 50 per cycle)

Your bar region includes panel chrome that's matching as a "phantom" bar. Re-pick the region with a tighter selection that excludes the panel borders.

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

Detection bug — your `active_color` is matching UI chrome that's always present, so detection thinks the totem is permanently active. **Fix**: either disable detection (`Detection enabled` off in the Totem tab), or recalibrate the active color on a different pixel.

### Auto-potion never fires

Most common cause: the **Potion** hotkey isn't bound. Check **Main** tab → **Hotkey Configuration** card → confirm the **Potion** field has a key (default `8`).

Also check the **Totem** tab → **Auto Potion** card → **Enabled** is on.

### Item swap mid-reel breaks cycles

This shouldn't happen — both runners gate fires on safe state. If it's happening, your `safe_check` is reporting safe when the macro is mid-cycle. File an issue with your latest log; we'll look at the state machine transitions.

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
