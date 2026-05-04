# fisch-macro v2.4.0

Minor release with one detection breakthrough, a major UX consolidation, and a pile of stability + observability work. The headline feature is **live-tint sampling**: the macro now reads the bar's danger color in real time from a small UI indicator instead of relying on a pre-configured static list, eliminating an entire class of "bar tracker loses the bar through tints" failures.

Overnight unattended run on this version: **3,864 cycles, 97.4% catch rate** (1440p / BetterCam / Default rod).

## What's new

### 1. Live-tint sampling — replaces the static `danger_bar_color` list

Fisch renders two small triangle indicators flanking the catch bar. Their color tracks the bar's *current* danger tint **exactly**, even when the bar is showing white — they preview the tint the bar will take on if the fish leaves the catch zone. We sample one of those triangles every frame and feed the resulting BGR into `detect_bar` as a per-frame `live_danger_color` candidate.

**Why it's a big deal:** the static `danger_bar_color` field required users to paste comma-separated hex lists for every background / lighting condition the bar could tint against. Translucent bars varied by what was behind them, and there was no way to anticipate every tint state. The live sampler reads whatever the game is currently rendering — no calibration of color values needed.

Calibration zone: F1 → **Live Bar Tint (triangle next to the bar)**. Drag a tight box over one of the triangles using the cursor magnifier (M).

A live-updating swatch on the Fish tab → Color Options card mirrors the sampler's output so you can confirm it's reading the right pixels while the macro runs. The old `Danger Bar` text field is gone; the schema field is preserved for legacy configs.

Field results: BGR detection share went from "fragile during heavy tints" to **96.8%** of all frames across 3,864 overnight cycles. Sampler fired 100% of attempts.

### 2. Unified F1 zone calibrator

Every screen region the macro can use — Fish Bar, Live Tint, Catch Progress Bar, Perfect Cast Meter, Auto Rod-Equip slot, Auto Reconnect zone — is now calibrated in one place. Press F1 anywhere (including over the game) to open the **Calibrate Zones** overview: a captured frame with every configured zone overlaid in a distinct color + label, plus a side panel with Edit / Clear buttons per zone.

Per-tab "Pick Region" / "Pick Detection Zone" / "Pick Search Area" buttons across the Fish, Perfect Cast, and Reconnect tabs are gone. A single discoverable hotkey replaces the scattered entry points.

### 3. Cursor magnifier in pickers

Inside any region picker (live overlay or frozen frame), **press M** to toggle a 6× zoomed inset that follows your cursor. The inset outlines the **exact pixel** that will be recorded if you click — what you see is what gets saved. Eliminates the "I thought I clicked that pixel but it saved the one next to it" failure mode that used to require post-pick arrow-key fine-tuning.

`MAGNIFIER_SOURCE_PX` is odd (25) so the cursor pixel is the unique geometric center of the crop, with `SmoothPixmapTransform=False` so individual pixels stay crisp instead of being smoothed. A faint orientation crosshair extends out of the target square so you can sight along it without occluding the target.

### 4. Color swatches everywhere

Every hex color field on the Fish tab now has a **live-updating color swatch** sibling that recolors as you type. No more mentally translating `#848587` to a color in your head. Comma-separated lists (the old danger-bar field) sample the first hex.

### 5. Threading hardening — fixes a Settings race

The state machine writes per-rod learned coefficients (`bar_physics_a/b/n`) to `Settings.rod_tunings[rod]` from the worker thread at every cycle end. Meanwhile, `ConfigManager.save` walks the same model via `model_dump_json` on the main thread when settings change. Without synchronization, the two collided sporadically and crashed the GUI deep inside Pydantic's slot dispatch.

Fix: `ConfigManager.SETTINGS_LOCK` (RLock). Worker writes acquire it briefly; `save` snapshots the model under the lock then releases before the (lock-free) JSON serialization. Smoke-tested under contention: 100 saves concurrent with ~65,000 worker writes, no crash, no corruption.

Companion fixes for related lifecycle issues:

- `HotkeyListener._dispatch` snapshots `_registered.items()` before iterating (callbacks can mutate the dict mid-iteration; CPython 3.12+ has segfaulted through ctypes-wrapped Win32 callbacks on iter-during-resize).
- The F1 zone-overview dialog now `deleteLater()`s after `exec()` so we don't accumulate leaked dialogs across F1 sessions.
- `faulthandler` is enabled at app startup, writing to `faulthandler.log` next to the regular session log on any segfault / abort.

### 6. Rod presets — 27 rods seeded by default

The default config now ships with 27 rod tunings pre-populated, each with the documented arrow color baked in:

- **Shared-grey-arrow group** (`#848587`): Default, Evil Pitch Fork, Luminescent Oath, Polaris Serenade, Ruinous Oath, Silly Fun, Merlin
- **Custom-arrow group**: Tryhard (`#dfacae`), Fabulous (`#a695a4`), Nates (`#bb8e20`), Requiem (`#040404`), Thalassar (`#000000`), Blade of Glorp (`#92af5d`)
- **Arrow-less group** (`bar_arrow_color = None`): Astraeus Serenade, Cerebra, Chrysalis, Dreambreaker, Duskwire, Masterline, Onirifalx, Pinions Aria, Rainbow Cluster, Rod of Shadow, Sanguine Spire, Sword of Darkness, Verdant Oath, Wingripper

`ConfigManager.load` merges new presets into existing user configs without overwriting tuned values, so upgrading doesn't clobber any rod-specific work.

### 7. Developer tab (maintainer-only)

When the active license key is a dev key, a new "Developer" tab appears (and the PRO pill swaps to a red "Developer" pill). Houses an in-app key generator: produces fresh `fisch-key-…` user-tier or `fisch-dev-…` dev-tier plaintext via `secrets.token_hex(32)`, computes the SHA-256 hash, and pre-formats it for pasting into `_KEY_HASHES` / `_DEV_KEY_HASHES` in source. A guarantee enforced by tests: a freshly generated key must validate `False` until its hash is added — the generator can't ship pre-valid keys.

Hidden from regular users (Pro or free).

### 8. GUI cleanup pass

After a user review of v2.3.0:

- **Stats tab removed** — the inline status panels on Buffs / Reconnect cover everything it used to show.
- **"Equip Rod Before Each Recast" toggle removed** — was actually broken (pressed the rod hotkey directly, which Roblox interprets as a rod-toggle-off; same bug v2.3.0's bag-then-rod sequence fixes elsewhere).
- **Active Totem Detection card removed** — the fixed-interval timer alone is sufficient; on-screen detection added complexity without correctness wins.
- **Per-tab region pickers removed** — centralized into F1.
- **Perfect Cast tab restructured** — single primary "Release Timing" knob promoted to the top; six secondary tunables tucked under "Advanced" sub-headers.

### 9. Antivirus false-positive documentation

The packaged Nuitka EXE gets flagged by ~7/61 AV heuristics (none signature-based — they're all "this binary is Nuitka-packed Python with input + capture APIs" pattern matches). New [docs/FALSE_POSITIVES.md](docs/FALSE_POSITIVES.md) explains exactly which vendors flag, what their labels mean, three independent verification paths (SHA-256 check, re-run the scan, build from source), and what we're doing about it (FP submissions, code-signing on the roadmap).

### 10. Observability for live-tint runs

Per-reel summary line now appends `tint=H/A(P%) last=#hex` showing how often the live-tint sampler fired and the most-recent sampled color. Session header dumps the live-tint configuration (enabled / explicit zone or auto-derived / offset / size / tolerance). Per-frame debug log gains a `tint=(b,g,r)` suffix so bar-tracking dropouts can be correlated with the sampled tint frame-by-frame.

## Upgrading

Drop-in. ConfigManager merges any missing fields and rod presets on first load. Existing per-rod tunings, hotkey bindings, license keys, and calibrated regions all carry forward.

The static `RodTuning.danger_bar_color` field is no longer surfaced in the GUI but the schema retains it — if you have a value set in your config it'll still be OR'd into the BGR mask alongside the live-tint sample. To clean up: just leave it; it stops mattering once live-tint is calibrated.

## Tests

482 tests pass on this release. New coverage:

- 6 tests for live-tint sampling (`sample_live_tint` + `detect_bar` integration)
- 4 tests for the key generator (format, uniqueness, validation-rejects-fresh-keys, hash-alias)

Plus the existing detection / controller / capture / config / license / state-machine suites.
