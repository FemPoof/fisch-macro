# First-Run Calibration

The macro refuses to start until the **fish bar region** is calibrated. Other zones are optional but improve detection.

All region calibration goes through one place: **press F1** anywhere (including over the game) to open the **Calibrate Zones** overview. Every zone the macro can use is listed there — calibrate any of them with one click.

## Picking the bar region (mandatory)

1. **Open Fisch and start a fishing cycle** so the catch-bar UI is visible on screen.
2. **Press F1** to open Calibrate Zones.
3. Click **Edit** on the **Fish Bar (the minigame catch bar)** row.
4. **Drag a tight rectangle around the bar slot** — the white horizontal strip with the fish marker. Don't include the panel chrome above or below.
5. **Click OK**. The region saves into your config.

Re-pick any time if the UI moves (resolution change, different rod with a different panel size). Just hit F1 again.

### Pixel-perfect placement

Inside the picker, **press M to toggle the cursor magnifier**. A 6× zoomed inset follows your cursor and outlines the exact pixel that will be recorded — what you see is what gets saved. Use it to land the rectangle's corners on the bar's true edge instead of the anti-aliased outer pixels.

You can also nudge with **arrow keys** (1 px), **Shift+arrows** (10 px), or **Ctrl+arrows** to resize from the bottom-right corner.

### Tips

- **Tight is better than loose.** A region that includes the panel borders gets edge artifacts the BGR cascade then has to filter out (`phantom_width` rejects in cycle summaries). Trim to the actual bar slot.
- **Snap-to-detection.** When the picker can detect a bar inside your selection, it draws a cyan dashed outline showing where. Click "Snap to detected bar" to tighten your selection to those bounds with one click.
- **Multi-monitor.** The picker uses absolute screen coordinates, so the region is bound to a specific physical screen position. Move Roblox to a different monitor → re-pick.

## Live Bar Tint zone (recommended, v2.4.0+)

Fisch renders two small triangle indicators just outside the bar's left and right edges. Their color tracks the bar's *current* danger tint exactly — even when the bar is showing white, the triangles preview the tint the bar will take if the fish leaves the catch zone. The macro samples that color every frame and feeds it to the BGR detector, eliminating the need for users to pre-configure static danger-color lists.

1. Press **F1** → click **Edit** on **Live Bar Tint (triangle next to the bar)**.
2. With the bar visible, press **M** for the magnifier and drag a tight box over one of the triangles.
3. Click OK.

You can verify the sampler is working from the **Fish** tab → **Color Options** card → **Live Tint** swatch. While the macro is running it should mirror the in-game triangle color in real time.

If you skip this step, the macro auto-derives sample boxes from the bar region's edges. That works for stock UI but is fragile across UI scales / camera angles — explicit calibration is more reliable.

## Sampling rod colors (optional)

The default rod tuning works for stock rods. If you swap to a rod with non-standard bar appearance:

1. Open Fisch with that rod equipped, get a bar visible.
2. Use the **Sample colors from a captured frame** button row at the bottom of the **Fish** tab → **Color Options** card.
3. Click **Bar (left)**, then click on the bar's left-edge pixel in the captured frame.
4. Repeat for **Bar (right)**, **Target line** (the fish marker), and **Bar arrow** (the small directional indicator on the bar's edge).

Each sampled hex saves directly into the active rod's tuning and reflects in the **Color Options** card with a live-updating swatch next to each field. Cancel keeps the previous value untouched.

### Per-rod scopes

Calibration is **per-rod**. Switch rods in **Fish** tab → **Fish Configuration** card → **Rod Type** dropdown, and the colors auto-load from that rod's saved tuning. Change a color → it saves only to the currently selected rod.

The default config ships with **27 rod presets** seeded — Default, Tryhard, Fabulous, Nates, Requiem, Thalassar, Blade of Glorp, plus the rods that don't have a usable arrow indicator (Astraeus Serenade, Cerebra, Chrysalis, etc.). To add another:

1. Click **Add Rod** under the Rod Type dropdown.
2. Type a name. The new rod inherits the currently-selected rod's tuning.
3. Equip the rod in Fisch.
4. Re-sample the colors as needed.

## Optional: other zones

The same F1 dialog lets you calibrate zones for the other detection-driven features:

- **Catch Progress Bar** — diagnostic only; reads catch % at reel end and shows up in the cycle summary as `end_progress=N%`. The macro doesn't gate decisions on it.
- **Perfect Cast Meter** — required if you opt into the Perfect Cast feature ([docs/perfect-cast.md](perfect-cast.md)).
- **Auto Rod-Equip — hotbar slot zone** — required for the rod-equip safety net ([docs/auto-features.md](auto-features.md)).
- **Auto Reconnect — disconnect overlay zone** — required for auto-reconnect ([docs/auto-features.md](auto-features.md)).

Each zone is independent. Skip the ones you don't need.

## What's saved where

The config file lives at:

```
%LOCALAPPDATA%\fisch-macro\config.json
```

Region, color, and tuning all live in that file. You can hand-edit if you need to bulk-import settings; see [advanced.md](advanced.md).
