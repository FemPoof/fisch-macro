# Detection Cascade

The macro tries detectors in order; first success wins. Default order: `bgr_first`.

| Tier | What it does | When it fires |
|------|--------------|---------------|
| **BGR full-fill** | Strict color match on calibrated white bar | Most frames; ~98 % of detections |
| **bgr_edges** | White-edge-pair span (handles partial tinting) | Bar partially tints during fish movement |
| **Arrow** | Per-rod fixed-color indicator on the bar's edge | BGR fails on fully-tinted bar (fish dashes) |
| **Template** | OpenCV correlation match against a captured patch | Backstop — when arrow doesn't fire |
| **Carryover** | Velocity-projected last-known position | All other tiers fail; ≤2 s window |

## Why this order

**BGR is cheapest and most precise** when your calibrated bar colors match what's on screen. It scans columns of the calibrated region for pixels matching `left_bar_color` / `right_bar_color` within `tolerance`, then confirms the run with a density gate. ~0.5 ms per frame.

**bgr_edges** is the partial-tint helper. When the bar is tinting but the white edges are still visible (e.g. early in a dash), this tier finds the outer edge pair and spans them. Same color rules, looser sub-region constraints.

**Arrow** is the [Hydra](https://github.com/) reference macro's secret sauce. Fisch bars include a small directional arrow on either the left or right edge of the bar, rendered in a fixed per-rod color (default `#848587` medium grey). This color **does not tint during fish dashes** — the bar's white pixels go red/orange but the grey arrow stays grey. So the arrow tier is a perfectly reliable fallback when BGR fails on a tinted bar.

**Template** captures a reference patch of the bar at the first clean BGR detection, then uses `cv2.matchTemplate(TM_CCOEFF_NORMED)` to find the bar in subsequent BGR-failure frames. Refreshes every ~750 ms during BGR-clean periods so the seed stays current. Cheaper than CSRT trackers, no drift. Kept as a **backstop** for rods that have `bar_arrow_color: None` (no usable arrow indicator).

**Carryover** is the last resort. When all detection tiers fail, the controller projects the bar's last-known center forward using its velocity. This keeps the controller engaged through brief blackouts (e.g. a tinted-bar dash that exceeds the template's window). Carryover expires after 2 s; past that, the controller parks and the cycle's `bar_lost_timeout` kicks in.

## Cycle summary breakdown

Every cycle's `Reel end:` log line includes per-tier telemetry:

```
via=[bgr=531 bgr_edges=4 arrow=22]
arrow=engaged=22f/no_anchor=0
template=engaged=0f/cap=1/refresh=11/match=0/acc=0
```

- **`via=[...]`** — total visible-frame source counts. `bgr` is BGR full-fill, `bgr_edges` is the partial-tint sub-path, `arrow` is the arrow tier, `template` is template matching.
- **`arrow=engaged=N/no_anchor=K`** — N frames where the arrow tier rescued the cycle, K skipped because no anchor center existed for side-inference.
- **`template=engaged=N/cap=K/refresh=R/match=M/acc=A/rej_score=X`** — N rescue frames, K seed captures, R refreshes (Phase-2 online refresh), M match attempts, A accepts, X score-rejected.

### Healthy run signal

| Metric | Healthy value |
|--------|---------------|
| `via_bgr` share | 95-99 % |
| `via_bgr_edges` share | 0.5-2 % |
| `via_arrow` share | 0-3 % depending on fish behavior |
| `via_template` share | < 0.1 % usually 0 (arrow gets there first) |
| `arrow=engaged=N` | 0 on calm cycles, 20-100 on dashy fish |
| `template peak` | > 0.85 indicates well-calibrated; < 0.50 means seed pixels drifted |

## Tier toggles

In **Main** tab → **Detection Cascade** card:

- **`use_template_fallback`** — on by default
- **`bar_arrow_color`** — `#848587` by default; set to empty to disable the arrow tier for rods without a usable arrow
- **`use_edge_fallback`** — off by default (legacy edge detector; superseded by arrow + template)
- **`use_hsv_fallback`** — off by default (color-family fallback; rarely needed with the modern cascade)
- **`use_tracker_fallback`** — off by default (CSRT tracker; field-tested as unreliable on Fisch's bar)

## When detection regresses

If your `via_bgr` share drops below 95 % consistently, the BGR colors are off — recalibrate your `left_bar_color` / `right_bar_color` from a fresh captured frame. The most common cause: graphics setting changed since calibration (each setting renders bar pixels slightly differently because of anti-aliasing).

If `arrow=engaged` rises sharply on cycles you weren't catching, your rod might have a non-standard arrow color — check the [Hydra reference table](advanced.md#hydra-arrow-color-reference) and recalibrate `bar_arrow_color` for that rod.
