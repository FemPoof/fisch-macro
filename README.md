# fisch-macro

A desktop automation tool for the [Roblox Fisch](https://www.roblox.com/games/16732694052/Fisch) reel minigame. Captures the screen, identifies the bar/fish indicator with color matching, and sends synthetic mouse + keyboard input to play the minigame for you.

Built with [Tauri 2](https://tauri.app/) (Rust backend + WebView frontend) + [SvelteKit](https://kit.svelte.dev/). Runs natively on macOS and Windows.

> [!WARNING]
> **Use at your own risk.** Automating gameplay almost certainly violates Roblox's Terms of Service. You can be banned. This project is intended for educational / personal use only — there are no warranties and the maintainer takes no responsibility for account actions taken against users.

## Features

- **Auto-loop fishing** — cast → wait for bite → reel → catch (or fail) → cast again, indefinitely, until you hit the stop hotkey
- **Color-tracking bar control** — watches the player bar and the fish indicator, drives M1 (left mouse) via PWM duty cycle to keep the bar centered. Defaults aligned with Hydra's published Color/Line tuning (Kp 0.7, Kd 0.3)
- **Auto-calibration** — captures a frame during the minigame and suggests color hex / tolerance / threshold values for your specific rod and lighting
- **F1 region picker** — overlay UI for selecting the screen regions to scan, with a pixel-precise magnifier and color picker
- **Status HUD** — small floating window showing live debug state during a run
- **Frame dumper** — saves every captured fish_bar tick as a PNG for offline diagnosis when detection breaks
- **Debug log** — full per-tick log written to a file under `~/Desktop/fisch-macro-debug/` for sharing when something goes wrong
- **Auto-updater** — checks GitHub Releases for new versions on launch; signed updates via the Tauri updater key

## Install

### Pre-built binaries (recommended)

Download the latest release from [the Releases page](https://github.com/FemPoof/fisch-macro/releases/latest):

| Platform | Download |
|---|---|
| macOS (Apple Silicon) | `fisch-macro_X.Y.Z_aarch64.dmg` |
| macOS (Intel) | `fisch-macro_X.Y.Z_x64.dmg` |
| Windows | `fisch-macro_X.Y.Z_x64-setup.exe` or `.msi` |

Open the installer and follow the prompts.

**macOS note**: the first time you launch, macOS will block the app because it isn't notarized. Right-click the app → Open → Open Anyway. (You can also do this from System Settings → Privacy & Security.)

**Windows note**: SmartScreen may block the unsigned installer. Click "More info" → "Run anyway". The installer is built and signed with the Tauri updater key, but Windows has its own code-signing layer that's separate.

### Build from source

You'll need:

- **Rust** (stable) — install via [rustup](https://rustup.rs/)
- **Node.js** 20+ — install via [nodejs.org](https://nodejs.org/) or your package manager
- **macOS**: Xcode Command Line Tools (`xcode-select --install`)
- **Windows**: [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/downloads/) with the "Desktop development with C++" workload
- [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) on Windows (pre-installed on Windows 11 and recent Windows 10)

Then:

```sh
git clone https://github.com/FemPoof/fisch-macro.git
cd fisch-macro
npm install
npm run tauri dev      # development build with hot-reload
npm run tauri build    # produces release binaries in src-tauri/target/release/bundle/
```

First build takes 5–10 minutes (Rust compiles a lot of dependencies). Subsequent builds are 10–30 seconds.

## Usage

### First time setup

1. **Launch the app**, then launch Roblox and join Fisch.
2. **Position the windows.** The macro window should be off to the side so it doesn't cover the game. The status HUD appears top-left during a run.
3. **Press F1** (default) to open the region picker. Drag rectangles around two regions:
   - **Shake** — covers the area where the SHAKE button appears (typically center-bottom of the screen)
   - **Fish Bar** — covers the slider that appears during the reel minigame (typically center-bottom, narrower than the shake region)
4. **Save & close** the picker (Enter, or click Save).
5. **Cast a real fish manually first.** When you land in the reel minigame, go to the **Tuning** tab → **Calibration** → click **Capture & analyze**. This samples your rod's actual colors and saves them as your defaults.
6. **Stop fishing manually**, then press your start hotkey (F6 by default) to start the macro.

### Daily use

1. Press start hotkey (F6) to begin auto-fishing.
2. Press it again to stop.

The macro will cast → wait for a bite → press SHAKE → play the reel minigame → cooldown → repeat.

If catches start failing, run **Capture & analyze** again — colors can drift between rods, locations, and times of day.

## How it works

### Phases

1. **Cast** — holds M1 for `castDurationMs` (default 1000ms) to throw the line.
2. **Lure** — waits for a fish to bite. Spams Enter via the in-game camera/photo-mode trick (Roblox accepts Enter as the SHAKE input that way).
3. **Reel** — when the bar UI appears, scans every tick:
   - **Color match** the player bar (left+right hex with tolerance) to find its current X position
   - **Compute error** = `target_x - player_x` where target is the fish_bar geometric center (or the fish position in tracking modes)
   - **Compute PWM duty** from the error: `neutral + Kp·error + Kd·(d_error/dt)`
   - **Apply duty** to a Rust thread that PWMs M1 at the configured cycle (default 100ms)
4. **Cycle end** — when the bar disappears for 30+ ticks (and at least 7s have passed), assume the catch animation is done.
5. **Cooldown** — wait 3.5s for any in-game animation to finish, then loop back to Cast.

### Why color matching, not neural nets

YOLO-style fish detection is more robust but adds 50+ MB of ML runtime, requires a trained model per rod variant, and is overkill when the bar UI has a fixed sprite. Hydra's published Color/Line modes — which we mimic — get 50–70% catch rate on color matching alone, and that's our ceiling without ML.

### Architecture

```
┌──────────────────────────┐         ┌─────────────────────────┐
│  Frontend (Svelte)       │ tauri   │  Backend (Rust)         │
│                          │ invoke  │                         │
│  - tick loop (~4–25 Hz)  │ ──────► │  - tick_macro()         │
│  - PID/PWM control logic │         │    capture + detect     │
│  - region picker UI      │         │  - PWM thread (M1)      │
│  - settings              │ ◄────── │  - Enter spam thread    │
│  - status HUD            │ event   │  - CG/Win32 capture     │
└──────────────────────────┘         └─────────────────────────┘
```

Tick rate is bottlenecked by the JS↔Rust IPC round-trip (~30–50ms each direction). On macOS we land at ~4–10 Hz; on Windows ~15–25 Hz with faster screen capture. Hydra (AHK, single-process) runs at 1000 Hz — that gap is architectural and can only be closed by moving the entire tick loop into a Rust thread (a future refactor).

## Settings reference

Settings persist in `localStorage` under `fm:*` keys. There's a versioned migration system (`fm:settingsVersion`) for upgrades that need to force-update bad values.

Key settings (Tuning + Advanced tabs):

- **Cast hold (ms)** — how long M1 stays down during cast (1000ms default)
- **Tracking mode** — `center` (hold bar at midpoint, recommended), `motion` (chase pixel motion), `color` (chase fish-color match)
- **Fish colors** — RGB hex per element (target line, left bar, right bar) + per-channel tolerance
- **White% / Density / Edge / Merge / Min count** — detection thresholds; lower these if the bar isn't detected
- **PWM** — Kp 0.7, Kd 0.3, Cycle 100ms, Neutral 50%, Max slew 80%/tick. Tune Neutral up if your rod's bar drifts left, down if it drifts right.

For everything else, see the comments in [`src/routes/+page.svelte`](src/routes/+page.svelte) — every setting has a `field-help` block explaining what it does.

## Troubleshooting

| Symptom | Cause / Fix |
|---|---|
| `whiteCols=0` for entire reel phase | Fish bar region isn't covering where the bar actually appears. Press F1, redo the Fish Bar region. |
| Detection works briefly, cycle ends in 5s, fish escapes | Bar transiently disappears mid-minigame (animation, fish indicator covers it). Already handled — minimum reel is 7s with 30-tick absence threshold. If still happening, share a debug log. |
| Bar oscillates wildly, never catches | Kp too aggressive for your rod, OR you're in `motion` mode chasing a jumpy fish position. Try `center` mode (default) and lower Kp to 0.5. |
| Hotkey "couldn't register" banner | Some other app or the OS is holding that key. Click the "Use F6/F7/F8" buttons in the banner to swap. F3 is reserved by macOS Mission Control. |
| Macro starts but nothing happens in-game | Cursor isn't over the Roblox window when the macro fires its M1 events. Move the macro window off the game viewport before starting. |
| Auto-updater doesn't see new version | The endpoint in `tauri.conf.json` must match the actual GitHub Releases URL. The bundled-into-app pubkey must match the private key used to sign the latest release. Mismatches → update silently fails. |

For anything else: turn on **Debug Logging** + **Frame dumper** in the Advanced tab, reproduce the issue, and share the resulting files from `~/Desktop/fisch-macro-debug/`.

## Contributing

Forks welcome. PRs welcome but no guarantees about merge timeline — this is a personal project.

If you're forking to ship your own version: replace the Tauri signing keypair (don't reuse mine, or your users will get update prompts from my releases). Generate a new pair with `npx tauri signer generate` and update both the GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY` and the `pubkey` in `src-tauri/tauri.conf.json`.

## Acknowledgments

- **Hydra** — the discontinued Windows-only macro this project is structurally inspired by. The Color/Line mode parameters (target/arrow/left/right hex values, Kp/Kd defaults) come from Hydra's published config screenshots.
- **Tauri** — for making it possible to ship a tiny native binary without Electron's overhead.

## License

[MIT](LICENSE) — see LICENSE file. TL;DR: do whatever you want with this code, just keep the copyright notice and don't sue me if it breaks something.
