# Auto-Totem & Auto-Potion

Both fire a hotkey on a fixed interval while the macro runs. Built-in safety: they only fire **between fishing cycles** (state machine in IDLE or COOLDOWN), never mid-reel. A mid-reel item swap unequips your rod and breaks the cycle.

Both live in the **Totem** tab.

## Auto-Totem

| Field | Default | Notes |
|-------|---------|-------|
| Enabled | off | Toggle on after configuring interval |
| Interval | 300 s (5 min) | Most totem effects last ~5 min |
| Activate on start | on | Fires once at macro start |
| Re-equip rod after | on | Auto-presses fishing rod hotkey after totem |
| Detection enabled | off | Optional: skip fire if totem is detected as still active |

**Hotkey** is read from your **Hotkey Configuration → Special Totem** binding (default `7`).

### Active-totem detection (optional)

The runner can capture a screenshot region and check for a calibrated "totem active" pixel color. If it sees the totem is still buffed, it skips the fire. To set up:

1. With the totem active in-game, click **Pick Detection Zone** and drag a rectangle over the totem's active indicator.
2. Click **Sample Active Color** and click on the indicator's distinctive color pixel.
3. Toggle **Detection enabled** on.

If detection misfires (every fire gets skipped), the calibrated color is matching too much UI chrome. Recalibrate or disable detection and let the timer fire blindly.

### Live status

The **Status** card shows fire count, skip count (when detection skipped a fire), and time-until-next-fire. Refreshes once per second while the macro is running.

## Auto-Potion

New in v2.2.0. Simpler than auto-totem — no detection step (potions don't have a persistent active-state UI indicator).

| Field | Default | Notes |
|-------|---------|-------|
| Enabled | off | Toggle on after configuring interval |
| Interval | 600 s (10 min) | Tune to match your potion's duration |
| Activate on start | on | Fires once at macro start |
| Re-equip rod after | on | Same as totem |
| Rod equip delay | 250 ms | Pause before swapping back to rod |

**Hotkey** is read from your **Hotkey Configuration → Potion** binding (default `8`).

### Why no detection

Potions in Fisch don't expose a persistent visible "still active" indicator the way totems do (totems leave a glowing icon; potions just buff your stats). The runner therefore fires blindly on the configured interval. Tune `interval_seconds` to match your potion's duration plus a small margin.

## Safe-state gating

Both runners share the macro's stop event and check the state machine's current state before firing. The check returns "safe" when the state machine is in **IDLE** or **COOLDOWN** — the brief windows between fishing cycles. If the timer fires while a cycle is running:

- The runner waits up to 60 s for a safe state
- Fires once safety returns
- Logs a warning if 60 s pass without a safe state (something else is wrong; the runner fires anyway as a safety hatch)

This is why totem/potion fires sometimes happen 1-2 s late on the wall clock — the runner is politely waiting for the current cycle to end before swapping items.

## Stopping cleanly

Both runners are daemon threads tied to the macro's main stop event. Pressing F3 (stop hotkey) or clicking Stop on the GUI shuts both down within ~250 ms.

## Logs

Each fire emits a log line:

```
[INFO] fisch_macro.core.auto_totem : Auto-totem fired (key=7, count=12, next in ~300s).
[INFO] fisch_macro.core.auto_potion: Auto-potion fired (key=8, count=4, next in ~600s).
```

Skips show up too:

```
[INFO] fisch_macro.core.auto_totem : Auto-totem: detection says totem still active; skipping fire (skipped=3). Will re-check in ~300s.
```

Search the log file at `%LOCALAPPDATA%\fisch-macro\logs\sessions\<date>_<pid>.log` to verify fires are happening on schedule.
