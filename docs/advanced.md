# Advanced: Per-Rod Tuning

Each rod can have its own complete tuning record. Switch rods in **Main** tab → **Configuration** card → **Rod** dropdown. The other tabs all re-populate from the selected rod's tuning.

## Adding a new rod

1. Type the name into the rod dropdown's editable field, press Enter.
2. A copy of the current rod's tuning saves under the new name.
3. Adjust per-rod fields (`kp`, `kd`, `bar_arrow_color`, etc.) without affecting other rods.

## Hand-editing config.json

The per-rod tuning store lives at:

```
%LOCALAPPDATA%\fisch-macro\config.json
```

Structure:

```jsonc
{
  "rod": "Default",                       // active rod name
  "rod_tunings": {
    "Default": {
      "kp": 1.8,
      "kd": 0.75,
      "panic_error_threshold": 100,
      "bar_arrow_color": "#848587",
      // ... all RodTuning fields
    },
    "Tryhard": {
      "kp": 0.45,
      "kd": 0.05,
      "bar_arrow_color": "#dfacae",       // pinkish, per Hydra reference
      // ... overrides only the fields you want different
    }
  }
}
```

Fields you don't specify in a per-rod block fall back to the schema defaults. So you can keep per-rod blocks minimal — just the tuning that diverges.

## Hydra arrow-color reference

The arrow tier (introduced in v2.1.10) needs to know each rod's arrow color. Hydra's reference table:

| Rod | `bar_arrow_color` |
|-----|-------------------|
| Default | `#848587` |
| Evil Pitch Fork | `#848587` |
| Luminescent Oath | `#848587` |
| Polaris Serenade | `#848587` |
| Ruinous Oath | `#848587` |
| Silly Fun | `#848587` |
| Merlin | `#848587` |
| Tryhard | `#dfacae` |
| Fabulous | `#a695a4` |
| Nates | `#bb8e20` |
| Requiem | `#040404` |
| Thalassar | `#000000` |
| Blade of Glorp | `#92af5d` |
| Astraeus Serenade | (None) |
| Cerebra | (None) |
| Chrysalis | (None) |
| Dreambreaker | (None) |
| Duskwire | (None) |
| Masterline | (None) |
| Onirifalx | (None) |
| Pinions Aria | (None) |
| Rainbow Cluster | (None) |
| Rod of Shadow | (None) |
| Sanguine Spire | (None) |
| Sword of Darkness | (None) |
| Verdant Oath | (None) |
| Wingripper | (None) |

For rods marked "(None)", set `bar_arrow_color: null` in the rod's tuning to disable the arrow tier; detection falls through to the template tier instead.

## Scan FPS per rod

Hydra also has per-rod `scan_fps` overrides. The macro doesn't use these in v2.2.0 — capture rate is global (`capture_target_fps` in the top-level config). Per-rod `scan_fps` values in `RodTuning` are kept for backwards compat with v2.1.x configs but ignored by the runtime.

To run a rod that needs faster capture (Masterline @ 420, Ruinous Oath @ 280), bump the global `capture_target_fps` while that rod is equipped. This will be addressed properly in a future version.

## Bar physics calibration (experimental)

There's an opt-in plant-model calibration mode that records `(duty, bar_velocity)` samples and fits a linear regression. Enable per-rod with `record_bar_physics: true`. The fitted coefficients save into `bar_physics_a / bar_physics_b / bar_physics_n` so they persist across sessions.

Field-tested over 87 cycles: the linear model didn't fit Fisch's actual bar physics well (coefficients flip signs cycle-to-cycle, residual std spikes). Disabled by default. Re-enable per rod if you want to gather data for a future non-linear model.

## Debug logging

**Extra** tab → **Settings** → **debug_log_enabled** turns on per-tick debug logging in cycle summaries. Useful for diagnosing controller behavior frame-by-frame, but generates a lot of data — disable for normal use.

When enabled, each reel cycle's debug log includes lines like:

```
reel t=  3.6s vis=True via=bgr L=931 R=1343 C=1137 W=413 fish_raw=1105 fish_smooth=1105 target=1102 vel=  -68px/s bar_vel= -136px/s err=-29 duty=0.50 reason=None panic+=0p/0r win=45f/32s stuck=0.0/8s
```

Fields:

- `t=Ns` — cycle elapsed seconds
- `vis=True/False` — bar visible this frame
- `via=bgr/bgr_edges/arrow/template` — detection tier that produced this frame
- `L/R/C/W` — bar's left edge / right edge / center / width
- `fish_raw / fish_smooth / target` — fish marker raw, smoothed, and lookahead-adjusted target
- `vel / bar_vel` — fish velocity / bar velocity (px/s)
- `err` — current error in pixels
- `duty` — controller's commanded duty cycle (0.0 = release, 1.0 = press)
- `panic+=Np/Mr` — panic press / release counter increments this tick
- `win=Nf/Ms` — recent window stats (frames / capture rate)
- `stuck=Ns/Ms` — stuck-detection elapsed / threshold

Useful for diagnosing "why isn't the controller doing what I expect" — every frame's full state is in the log.
