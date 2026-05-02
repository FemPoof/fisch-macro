# First-Run Calibration

The macro refuses to start until the **fish bar region** is calibrated. Optional: per-rod color sampling for non-default rods.

## Picking the bar region (mandatory)

1. **Open Fisch and start a fishing cycle** so the catch-bar UI is visible on screen.
2. **In the macro**, click **Main** tab → **Capture Region** card → **Pick Region** button. The main window hides; the screen freezes on a captured frame.
3. **Drag a tight rectangle around the bar slot** — the white horizontal strip with the fish marker. Don't include the panel chrome above or below.
4. **Click "Confirm"**. The region saves into your config.

Re-pick anytime if the UI moves (resolution change, different rod with a different panel size).

### Tips

- **Tight is better than loose.** A region that includes the panel borders gets edge artifacts the BGR cascade then has to filter out (`phantom_width` rejects in cycle summaries). Trim to the actual bar slot.
- **Snap-to-detection.** The picker shows a live preview of where the bar would be detected within your selection. If the preview doesn't latch onto the bar, your selection is too narrow or too wide — adjust until it does.
- **Multi-monitor.** The picker uses absolute screen coordinates, so the region is bound to a specific physical screen position. Move Roblox to a different monitor → re-pick.

## Sampling rod colors (optional)

The default rod tuning works for stock rods. If you swap to a rod with non-standard bar appearance:

1. Open Fisch with that rod equipped, get a bar visible.
2. Use the **Sample colors from a captured frame** button row in **Main** tab → **Capture Region** card.
3. Click **Bar (left)**, then click on the bar's left-edge pixel in the captured frame.
4. Repeat for **Bar (right)**, **Target line** (the fish marker), and **Bar arrow** (the small directional indicator on the bar's edge).

Each sampled hex saves directly into the active rod's tuning and reflects in the **Color Options** card. Cancel keeps the previous value untouched.

### Per-rod scopes

Calibration is **per-rod**. Switch rods in **Main** tab → **Configuration** card → **Rod** dropdown, and the colors auto-load from that rod's saved tuning. Change a color → it saves only to the currently selected rod.

To set up a new rod:

1. Type a name into the rod dropdown's editable field, press Enter. A copy of the current rod's tuning saves under the new name.
2. Equip the rod in Fisch.
3. Re-sample the colors as needed.

## Optional: catch-progress region

If you want the macro to track your catch percentage (the green-to-red progress bar that fills during the minigame), pick its region too:

1. Open the **Totem** tab → **Active Totem Detection** card → **Pick Detection Zone**, but pick the catch-progress bar's region instead of a totem indicator. (The picker is general-purpose despite the card name.)
2. Save.

This is purely diagnostic — it shows up in the cycle summary as `progress=N %` and helps classify cycles as caught vs. escaped. The macro doesn't gate any decisions on the progress bar.

## What's saved where

The config file lives at:

```
%LOCALAPPDATA%\fisch-macro\config.json
```

Region, color, and tuning all live in that file. You can hand-edit if you need to bulk-import settings; see [advanced.md](advanced.md).
