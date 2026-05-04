# Perfect Cast (Experimental)

> **Status:** Experimental. Works in principle, but the camera-force
> sequence + release-timing slider need per-setup tuning to land
> consistently in the green zone. Expect to iterate. If you just want
> reliable fishing, leave **Cast Style** on **Normal**.

Perfect Cast replaces the flat M1 hold with a choreographed sequence
that:

1. Forces the camera into a known position (zoom out, zoom in, look
   down, look up) so the cast meter renders at a predictable on-screen
   location every time.
2. Holds M1 to charge the cast.
3. Watches the cast meter's white-fill percentage rise and releases M1
   when the predicted fill % crosses a target threshold, modulated by
   a single timing-offset slider.

**It is not yet calibrated to "just work" out of the box** — your
Roblox mouse sensitivity, screen resolution, and chosen rod all
change the right durations and drag rates.

## When to use Normal vs Perfect

| | Normal | Perfect (premium) |
|--|--|--|
| Setup time | None | 5-10 min, plus tuning passes |
| Reliability | High | Currently inconsistent; needs tuning |
| Output quality | OK casts | Perfect casts (better fish odds) |
| Camera moves | No | Yes — drives the camera every cast |
| Premium-only | No | Yes |

Stick with Normal until you've got time to iterate. Perfect Cast is
worth the effort for long farming runs once tuned, but is not a
turn-it-on-and-forget feature today.

## Calibration

All controls live in the **Cast** tab → **Cast Configuration** card →
set **Cast Style** to **Perfect (premium)**. The Perfect Cast section
expands inline below the dropdown.

### 1. Pick the cast meter region

1. **Manually** trigger a cast in Fisch (M1 hold) so the cast meter is
   on screen.
2. Click **Pick Search Area**. The macro window hides; drag a tight
   box around the cast meter.
3. Confirm.

The macro forces the camera into the same position before every cast,
so this region stays valid across all subsequent casts. **If you
change the cast sequence's zoom/look durations, re-pick the region.**

### 2. (Optional) Sample the white indicator color

Defaults to `#ffffff` which is correct for stock rods. If your rod has
a tinted indicator (rare), click **Sample White Cursor** and click on
the moving white pixel.

### 3. Verify detection

Click **Test Detection Now** with the cast meter on screen. You should
see something like:

```
✓ DETECTED  current fill = 23.4 % of region area
```

If you see ✗, the region is wrong (re-pick) or the indicator color is
off (sample).

### 4. Enable

Tick the **Enabled** checkbox. The macro will refuse to enable until
the region is calibrated.

## Tuning when it doesn't work

The most common failure modes during initial tuning:

### Character walks around during the cast

You're on a version older than this docs page. The earliest perfect-
cast prototype pressed arrow keys to "tilt the camera," but Roblox's
default arrow-key bindings move the character forward / back. Update
to the latest release — current code uses right-click + mouse drag,
which Roblox interprets as camera pitch.

### Camera doesn't tilt enough / tilts too far

Roblox mouse sensitivity varies per-user, so the same drag rate
produces different camera angles. Two knobs to adjust:

1. **Camera drag speed** (Cast tab → Perfect Cast → Camera drag speed).
   Defaults to 5 px/tick. Raise if camera doesn't tilt enough; lower
   if it over-tilts. Range 1-50.
2. **Cast sequence durations**. The default sequence has
   `look_down: 2000 ms` and `look_up: 900 ms`. Hand-edit
   `cast_sequence` in `%LOCALAPPDATA%\fisch-macro\config.json` if you
   need finer control over the camera path.

After adjusting, re-pick the meter region — its on-screen location
shifts when the camera ends up in a different position.

### Releases happen too early / too late

Adjust **Release Timing** (slider, -100 to +100 ms). Negative releases
earlier (compensates for a fast-moving meter); positive releases
later. Tune by ±5 increments while watching cast outcomes in-game.

The slider is layered on top of the **Target fill %** baseline (default
85 %). If the slider feels stuck at one extreme, try shifting the
baseline by 5-10 % first, then re-zero the slider.

### Camera ends up in the wrong place every time

The defaults (zoom_out 13, zoom_in 6, look_down 2000 ms, look_up 900 ms)
are field-tested. If your in-game camera starts at a wildly different
baseline (heavily zoomed in, mouse pointing at the sky), the
sequence's relative motions land you somewhere else.

Quick fix: before each fishing session, manually zoom all the way in
and look at the horizon. The macro's zoom_out + look_down then
operate from a known starting point. Long-term: editor-style sequence
authoring is on the roadmap.

### Detection fails consistently

After a successful camera-force sequence the meter should land in the
calibrated region. If **Test Detection Now** shows ✓ when you trigger
a cast manually but ✗ during macro runs, the camera sequence is
landing somewhere different than your manual setup.

Possible causes:

- Roblox window not focused at cast start (check **Auto-focus Roblox**
  in the Extra tab).
- Mouse-wheel zoom events not reaching Roblox (check
  pydirectinput.scroll fallback to 'I' / 'O' key presses — see
  `core/input_driver.py`'s `mouse_wheel_tick`).
- Frame rate too low for the detection loop (raise **Scan FPS** in
  the Cast tab; default 150).

## Settings reference

| Setting | Default | What it does |
|---------|---------|--------------|
| `enabled` | off | Master toggle. Refuses to enable without region calibrated. |
| `region` | not calibrated | Cast meter screen region. Pick once after the camera-force sequence is dialed in. |
| `white_color` | `#ffffff` | Indicator pixel color. Sample only for off-white rods. |
| `white_tolerance` | 15 | Per-channel BGR tolerance for matching white pixels. |
| `release_style` | `Idiotproof` | Single-slider mode. Multi-band styles may arrive later. |
| `release_timing` | 0 ms | Slider, -100 to +100 ms. Shifts release timing relative to the target fill %. |
| `base_target_fill_pct` | 85 % | Baseline fill % to release at. |
| `wait_for_second_pass` | on | Bottom-guard: wait for fill % to hit zero once before considering release (ensures we're on the upswing). |
| `scan_fps` | 150 | Per-frame poll rate of the release loop. |
| `fail_timeout_s` | 3.0 | Release M1 anyway after this long; cycle proceeds as a non-perfect cast. |
| `look_drag_speed_px_per_tick` | 5 | Pixels of vertical mouse drag per 20 ms tick during look_down / look_up. |
| `cast_sequence` | field-tested default | Camera-force + release ordering. Hand-edit in config.json. |

## Default cast sequence

```
delay 0
zoom_out 13         ← mouse wheel down ×13
delay 0
zoom_in 6           ← mouse wheel up ×6
delay 0
look_down 2000 ms   ← right-click held + cursor drag down
delay 0.2
look_up 900 ms      ← right-click held + cursor drag up
delay 0.2
hold_lmb            ← M1 down
delay 0
release             ← detection loop fires; M1 up on fill % crossing
delay 0
zoom_in 5           ← post-cast camera reset
delay 0
look_up 2000 ms
delay 0.2
```

## Roadmap

Things currently missing that would make Perfect Cast turn-key:

- In-app cast sequence editor (today: edit `cast_sequence` in
  `config.json` by hand)
- Per-rod overrides (today: one global setup applies to every rod)
- Calibration helper that auto-tunes `look_drag_speed_px_per_tick`
  against the user's Roblox sensitivity
- Multi-band release styles (today: Idiotproof slider only)

Until those land, **Normal cast is the recommended default for
unattended runs.**
