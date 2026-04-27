<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { Window } from "@tauri-apps/api/window";
  import { invoke } from "@tauri-apps/api/core";
  import { emit } from "@tauri-apps/api/event";
  import {
    register,
    unregister,
    isRegistered,
  } from "@tauri-apps/plugin-global-shortcut";
  import { check } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import {
    loadRegions,
    REGION_META,
    type RegionsConfig,
    type RegionKey,
    type Region,
  } from "$lib/regions";

  const KEYS: RegionKey[] = ["shake", "fish_bar", "shake_template"];
  const SHAKE_COOLDOWN_MS = 200;
  const COUNTDOWN_S = 3;

  type TickResult = {
    shake_click: [number, number] | null;
    shake_score: number;
    shake_threshold: number;
    shake_has_template: boolean;
    player_x: number | null;
    fish_x: number | null;
    target_x: number | null;
    capture_ms: number;
    fb_white_cols: number;
    fb_best_run_len: number;
    fb_max_grey_per_col: number;
    fb_top_runs: [number, number][];
    fb_run_count: number;
    fb_total_left: number;
    fb_total_right: number;
    fb_total_target: number;
    fb_total_arrow: number;
    fb_best_target_x: number | null;
    fb_best_target_score: number;
    detect_ms: number;
    motion_x: number | null;
    motion_score: number;
    motion_total: number;
  };

  type ShakeResult = {
    centroid: [number, number] | null;
    score: number;
    threshold: number;
    has_template: boolean;
  };

  // Persisted settings — restored from localStorage on page load and saved
  // on every change. Survives macro window close + reopen so the user
  // doesn't have to re-tune strictness / cast hold / tick gap each session.
  // Settings version migration. Bump this whenever we identify a stored
  // value that's known-bad and want to force-reset it for all users on
  // upgrade. Runs before any loadSetting call so the bad keys are gone
  // by the time state initializes.
  const CURRENT_SETTINGS_VERSION = 7;
  if (typeof localStorage !== "undefined") {
    const v = parseInt(localStorage.getItem("fm:settingsVersion") || "0", 10);
    if (v < 3) {
      // v3: targetWindow caused 500-1500ms cap_ms when set to anything,
      // even leftover values from the previous default. Force-clear so
      // every user is back on fast display capture as the baseline.
      localStorage.removeItem("fm:targetWindow");
    }
    if (v < 4) {
      // v4: F3 is reserved by macOS Mission Control — RegisterEventHotKey
      // fails for it, the macro never receives F3 keypresses. Migrate any
      // user still on the old F3 default to F6. Also re-clear
      // targetWindow because some users typed "Roblox" back in after v3.
      const oldStop = localStorage.getItem("fm:stopHotkey");
      if (oldStop === '"F3"' || oldStop === "F3" || oldStop === null) {
        localStorage.setItem("fm:stopHotkey", JSON.stringify("F6"));
      }
      localStorage.removeItem("fm:targetWindow");
    }
    if (v < 5) {
      // v5: 40% whitePct was too strict on macOS — the bar is typically
      // only ~12px tall inside a 40px-tall scan region, so even a perfect
      // color match maxes out at ~30% per column. Lower default to 20%.
      // Only migrate users still on the old default; respect any custom
      // value the user explicitly set. Same for the user's saved-default.
      const oldWp = localStorage.getItem("fm:fishWhitePct");
      if (oldWp === "40") {
        localStorage.setItem("fm:fishWhitePct", "20");
      }
      const savedWp = localStorage.getItem("fm:default:fishWhitePct");
      if (savedWp === "40") {
        localStorage.setItem("fm:default:fishWhitePct", "20");
      }
    }
    if (v < 6) {
      // v6: alwaysOnTop default flipped from true → false. The
      // always-on-top + cross-Spaces dance interacted badly with the
      // NSPanel class swap we tried for native-fullscreen overlay
      // support (ghosting + cursor not tracking). Default off; users
      // can re-enable from the Extra tab if they want the main
      // window pinned in non-fullscreen scenarios.
      localStorage.setItem("fm:alwaysOnTop", "false");
    }
    if (v < 7) {
      // v7: force-reset bar tracking + PWM tuning to the proven
      // "center hold + Hydra-style PID" recipe. Motion-mode tracking
      // chases a fish position that jumps around → bar overshoots and
      // misses. Center mode holds bar at fish_bar geometric center and
      // lets game physics bring the fish to the bar (50–70% catch in
      // user testing). PWM defaults bumped from kp=0.4/kd=1.5 to
      // kp=0.7/kd=0.3 (closer to Hydra's published 0.9/0.3) so the
      // bar actually keeps up with leftward drift instead of
      // oscillating in the right half. After 2 days of failed catches
      // with motion + low-kp, this is the unblock.
      localStorage.setItem("fm:fishTrackMode", JSON.stringify("center"));
      localStorage.setItem("fm:pwmKp", "0.7");
      localStorage.setItem("fm:pidKd", "0.3");
      localStorage.setItem("fm:default:fishTrackMode", JSON.stringify("center"));
      localStorage.setItem("fm:default:pwmKp", "0.7");
      localStorage.setItem("fm:default:pidKd", "0.3");
    }
    if (v < CURRENT_SETTINGS_VERSION) {
      localStorage.setItem(
        "fm:settingsVersion",
        String(CURRENT_SETTINGS_VERSION)
      );
    }
  }

  function loadSetting<T>(key: string, fallback: T): T {
    if (typeof localStorage === "undefined") return fallback;
    const raw = localStorage.getItem(`fm:${key}`);
    if (raw == null) return fallback;
    try {
      return JSON.parse(raw) as T;
    } catch {
      return fallback;
    }
  }

  // Like loadSetting but with a "saved defaults" layer between localStorage
  // and the hardcoded fallback. Order: live setting → saved defaults → fallback.
  // When the user runs auto-calibrate and clicks Apply, those values get
  // saved both as the live value AND as the new saved-default. So Reset
  // takes them back to their last known-good calibration instead of
  // factory Hydra values.
  function loadSettingWithDefault<T>(key: string, factoryFallback: T): T {
    if (typeof localStorage === "undefined") return factoryFallback;
    const live = localStorage.getItem(`fm:${key}`);
    if (live != null) {
      try {
        return JSON.parse(live) as T;
      } catch {}
    }
    const saved = localStorage.getItem(`fm:default:${key}`);
    if (saved != null) {
      try {
        return JSON.parse(saved) as T;
      } catch {}
    }
    return factoryFallback;
  }

  // Resolve "what should Reset go back to" — saved default if present,
  // else factory. Used by reset buttons.
  function loadSavedDefault<T>(key: string, factoryFallback: T): T {
    if (typeof localStorage === "undefined") return factoryFallback;
    const saved = localStorage.getItem(`fm:default:${key}`);
    if (saved == null) return factoryFallback;
    try {
      return JSON.parse(saved) as T;
    } catch {
      return factoryFallback;
    }
  }

  function saveAsDefault(key: string, value: unknown) {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(`fm:default:${key}`, JSON.stringify(value));
  }

  // Customizable hotkeys. Tauri's global-shortcut plugin uses Accelerator
  // strings like "F1", "F3", "Shift+F2", "CommandOrControl+R". Function keys
  // are the friendliest default for a fishing macro since they don't collide
  // with in-game movement keys.
  let overlayHotkey = $state<string>(loadSetting("overlayHotkey", "F1"));
  // Default F6 instead of F3: macOS reserves F3 for Mission Control by
  // default, which causes RegisterEventHotKey to fail silently — the
  // macro then doesn't respond to F3 at all because the hotkey never
  // bound. F6 is unused on default macOS keyboards.
  let stopHotkey = $state<string>(loadSetting("stopHotkey", "F6"));
  $effect(() => { localStorage.setItem("fm:overlayHotkey", JSON.stringify(overlayHotkey)); });
  $effect(() => { localStorage.setItem("fm:stopHotkey", JSON.stringify(stopHotkey)); });

  type TabId = "main" | "tuning" | "advanced";
  const TABS: { id: TabId; label: string }[] = [
    { id: "main", label: "Main" },
    { id: "tuning", label: "Tuning" },
    { id: "advanced", label: "Advanced" },
  ];
  let activeTab = $state<TabId>("main");

  let cfg = $state<RegionsConfig | null>(null);
  let status = $state(`Ready. Press ${overlayHotkey} to edit regions.`);
  let hotkeyOk = $state(false);

  let running = $state(false);
  let dryRun = $state<boolean>(loadSetting("dryRun", false));
  let castDurationMs = $state<number>(loadSetting("castDurationMs", 1500));
  // Tick interval — fish-bar tracking + PID update happen on this cadence.
  // 50ms is plenty for the player marker to react smoothly without burning CPU.
  let tickGapMs = $state<number>(loadSetting("tickGapMs", 50));
  // Score threshold for the legacy template-matching SHAKE detector. Only
  // used when shakeMode === "template". Default mode is "navigation" (Enter
  // spam) which doesn't use this.
  let shakeMaxAvgDiff = $state<number>(loadSetting("shakeMaxAvgDiff", 40));
  // SHAKE detection mode:
  //   "navigation" — spam Enter on a fast interval; no image work (Hydra-style,
  //                  default, far more reliable than image detection)
  //   "template"   — use the captured template + IoU/NCC matcher (legacy
  //                  fallback in case Enter doesn't trigger SHAKE in your game)
  let shakeMode = $state<"navigation" | "template">(
    loadSetting("shakeMode", "navigation")
  );
  // Time between Enter keypresses while reeling. 50ms = 20 presses/sec —
  // any human keypress speed and faster than the SHAKE prompt's reaction
  // window. Each press takes ~8ms in Rust so 50ms is sustainable.
  let enterSpamMs = $state<number>(loadSetting("enterSpamMs", 50));
  // PID control gains for the fish-bar player marker.
  //   error     = target_x - player_x  (positive: player needs to move RIGHT)
  //   output    = pidKp * error + pidKd * d_error/dt
  //   if output > +deadband: HOLD M1 (push player right)
  //   if output < -deadband: RELEASE M1 (let player drift left)
  // Deadband must be reasonably large because M1 control is binary — without
  // it the controller toggles every tick on tiny errors, causing oscillation.
  // KD is high to brake the player as it approaches the target so it doesn't
  // overshoot before the next tick.
  let pidKp = $state<number>(loadSetting("pidKp", 0.5));
  let pidKd = $state<number>(loadSetting("pidKd", 0.3));
  let pidDeadband = $state<number>(loadSetting("pidDeadband", 30));
  // Rapid-click cadence used when the PID output is inside the deadband.
  // In Roblox Fisch, alternating M1 down/up at high frequency is the
  // "hold position" command — neither hold (push right) nor release
  // (drift left) but somewhere between. Each toggle is one event;
  // 25ms gives ~20 click cycles/sec.
  let rapidClickMs = $state<number>(loadSetting("rapidClickMs", 25));
  // PWM cycle length in ms. The user reported 40ms (= ~50 M1 events/sec)
  // looked like aggressive spam and might confuse the game's click
  // handling. 100ms = 10 events/sec at 50% duty, much more like a real
  // player's tapping cadence. Still fast enough for proportional control
  // but slow enough that each click is registered cleanly.
  let pwmCycleMs = $state<number>(loadSetting("pwmCycleMs", 100));
  // Neutral (steady) duty %. The duty value at which the bar holds station
  // — depends on the game's natural left-drift force vs M1-down right-push
  // force. 50% is the safe starting point: log 1777247808 (neutral=50)
  // produced 7.4s of stable control, but lowering to 30 in 1777248304 made
  // it worse — the controller's resting duty was so low that the input
  // buffer accumulated leftward pressure during the startup lock period
  // and flushed all at once. Keep 50 default; user can fine-tune per rod.
  let pwmNeutralDuty = $state<number>(loadSetting("pwmNeutralDuty", 50));
  // Per-pixel-of-error gain for converting err → duty offset. duty = neutral
  // + kp_pwm * err. With kp_pwm = 0.4, an err of +50 maps to +20% duty (so
  // duty goes from 30% neutral to 50% — moderate rightward push).
  let pwmKp = $state<number>(loadSetting("pwmKp", 0.7));
  // Maximum percentage points the duty can change per tick. In theory
  // this damps the buffer-flush-amplification feedback loop. In practice
  // (log 1777248304) too low a value (40) prevented full-strength recovery
  // when the bar suddenly jumped — the controller wanted 100% but only
  // got 67%, and the bar got lost. Default raised to 80 so the limit
  // only catches the truly pathological 0→100 swings; 40 stays user-
  // tunable for those who want more aggressive damping.
  let pwmMaxSlewPct = $state<number>(loadSetting("pwmMaxSlewPct", 80));

  // Fish-bar Color-mode detection params (Hydra-style). Hex values are 6-char
  // strings without the leading #. Tolerances are per-channel (0-255). All
  // are user-tunable from the Fish tab. loadSettingWithDefault checks for
  // saved-from-auto-calibrate values before falling back to Hydra factory
  // defaults, so a user who has calibrated their rod gets their values back
  // when localStorage is partial / after a Reset.
  let fishTargetHex = $state<string>(loadSettingWithDefault("fishTargetHex", "434b5b"));
  let fishArrowHex = $state<string>(loadSettingWithDefault("fishArrowHex", "848587"));
  let fishLeftHex = $state<string>(loadSettingWithDefault("fishLeftHex", "f1f1f1"));
  let fishRightHex = $state<string>(loadSettingWithDefault("fishRightHex", "ffffff"));
  let fishTargetTol = $state<number>(loadSettingWithDefault("fishTargetTol", 2));
  let fishArrowTol = $state<number>(loadSettingWithDefault("fishArrowTol", 0));
  let fishLeftTol = $state<number>(loadSettingWithDefault("fishLeftTol", 3));
  let fishRightTol = $state<number>(loadSettingWithDefault("fishRightTol", 3));
  // Detection thresholds. Hydra ships 80% but their fish_bar region was tuned
  // tightly around the bar. With macOS color management plus a region that
  // includes vertical slack above/below the bar, the bar typically only
  // fills ~12px of a 40px-tall scan, so even a perfect color match maxes
  // around 30% per column. 20% is permissive enough to detect a thin bar
  // inside a slack region without false-positiving on background pixels.
  let fishWhitePct = $state<number>(loadSettingWithDefault("fishWhitePct", 20));
  let fishMinLineDensity = $state<number>(loadSettingWithDefault("fishMinLineDensity", 40));
  let fishEdgeTouch = $state<number>(loadSettingWithDefault("fishEdgeTouch", 1));
  let fishMergeDistance = $state<number>(loadSettingWithDefault("fishMergeDistance", 2));
  let fishMinLineCount = $state<number>(loadSettingWithDefault("fishMinLineCount", 4));

  function hexToRgb(h: string): [number, number, number] {
    const m = h.replace(/^#/, "").trim();
    if (!/^[0-9a-fA-F]{6}$/.test(m)) return [0, 0, 0];
    return [
      parseInt(m.slice(0, 2), 16),
      parseInt(m.slice(2, 4), 16),
      parseInt(m.slice(4, 6), 16),
    ];
  }

  function fishParamsForTick() {
    return {
      target_line: hexToRgb(fishTargetHex),
      arrow: hexToRgb(fishArrowHex),
      left_bar: hexToRgb(fishLeftHex),
      right_bar: hexToRgb(fishRightHex),
      target_tol: Math.max(0, Math.min(255, Math.round(fishTargetTol))),
      arrow_tol: Math.max(0, Math.min(255, Math.round(fishArrowTol))),
      left_tol: Math.max(0, Math.min(255, Math.round(fishLeftTol))),
      right_tol: Math.max(0, Math.min(255, Math.round(fishRightTol))),
      white_pct: Math.max(0, Math.min(100, Math.round(fishWhitePct))),
      min_line_density: Math.max(0, Math.min(100, Math.round(fishMinLineDensity))),
      edge_touch: Math.max(0, Math.round(fishEdgeTouch)),
      merge_distance: Math.max(0, Math.round(fishMergeDistance)),
      min_line_count: Math.max(1, Math.round(fishMinLineCount)),
    };
  }

  // Reset goes back to the user's "saved defaults" (last applied auto-cal)
  // if any exist, otherwise factory Hydra values. This way calibrating
  // a rod once means you can experiment freely and Reset always brings
  // you back to your known-good values, not factory.
  function resetFishColorDefaults() {
    fishTargetHex = loadSavedDefault("fishTargetHex", "434b5b");
    fishArrowHex = loadSavedDefault("fishArrowHex", "848587");
    fishLeftHex = loadSavedDefault("fishLeftHex", "f1f1f1");
    fishRightHex = loadSavedDefault("fishRightHex", "ffffff");
    fishTargetTol = loadSavedDefault("fishTargetTol", 2);
    fishArrowTol = loadSavedDefault("fishArrowTol", 0);
    fishLeftTol = loadSavedDefault("fishLeftTol", 3);
    fishRightTol = loadSavedDefault("fishRightTol", 3);
    fishWhitePct = loadSavedDefault("fishWhitePct", 20);
    fishMinLineDensity = loadSavedDefault("fishMinLineDensity", 40);
    fishEdgeTouch = loadSavedDefault("fishEdgeTouch", 1);
    fishMergeDistance = loadSavedDefault("fishMergeDistance", 2);
    fishMinLineCount = loadSavedDefault("fishMinLineCount", 4);
  }

  // Wipe all saved defaults and restore factory Hydra values. Behind a
  // separate button so a user has to deliberately discard their calibration.
  function resetFishColorFactory() {
    if (typeof localStorage !== "undefined") {
      [
        "fishTargetHex", "fishArrowHex", "fishLeftHex", "fishRightHex",
        "fishTargetTol", "fishArrowTol", "fishLeftTol", "fishRightTol",
        "fishWhitePct", "fishMinLineDensity", "fishEdgeTouch",
        "fishMergeDistance", "fishMinLineCount",
      ].forEach((k) => localStorage.removeItem(`fm:default:${k}`));
    }
    fishTargetHex = "434b5b";
    fishArrowHex = "848587";
    fishLeftHex = "f1f1f1";
    fishRightHex = "ffffff";
    fishTargetTol = 2;
    fishArrowTol = 0;
    fishLeftTol = 3;
    fishRightTol = 3;
    fishWhitePct = 20;
    fishMinLineDensity = 40;
    fishEdgeTouch = 1;
    fishMergeDistance = 2;
    fishMinLineCount = 4;
  }

  // When enabled, every tick's debug line plus key state transitions get
  // appended to a timestamped file under ~/Desktop/fisch-macro-debug/. We
  // open the file when the macro starts and close it when it stops.
  let debugLogging = $state<boolean>(loadSetting("debugLogging", false));
  // Diagnostic frame dumper. When ON, every tick saves the captured fish_bar
  // region as a PNG to ~/Desktop/fisch-macro-debug/frames/<run-ts>/tick-NNNN.png.
  // The whole point: stop guessing what the detector sees — just LOOK at the
  // frames and confirm whether the bar is in-region, what colors it actually
  // renders as, and which ticks were during cast/lure vs. reel. Adds ~1ms
  // per tick on a typical 700×40 region; off by default to keep disk clean.
  let dumpFrames = $state<boolean>(loadSetting("dumpFrames", false));
  let dumpFramesRunDir = "";
  let dumpFramesTick = 0;
  // Auto-calibrate the moment a minigame is detected this session.
  // Default OFF: in user testing across many rods, auto-cal repeatedly
  // *worsened* detection by replacing the Hydra factory defaults
  // (#f1f1f1/#ffffff for the bar — which match real anti-aliased pixels
  // through tol=3) with histogram-derived hexes like #d0d0d0/#e0e0e0
  // that match fewer columns. The Hydra defaults are good across rod
  // variants; auto-cal is opt-in for the unusual case where they don't
  // match this user's rod at all.
  let autoCalibrate = $state<boolean>(loadSetting("autoCalibrate", false));
  let hasCalibratedThisSession = false;
  $effect(() => { localStorage.setItem("fm:autoCalibrate", JSON.stringify(autoCalibrate)); });
  // Fish tracking strategy. Three options:
  //   "center" — hold bar at fish_bar geometric center (Hydra Color/Line).
  //              Default. Most reliable across rod variants. ~50% catch.
  //   "color"  — steer to detected fish-line position via color match.
  //              Requires manually-set Target Line hex; auto-cal tends to
  //              lock onto fixed UI elements like end-cap markers.
  //   "motion" — steer to the column with the most pixel motion between
  //              consecutive frames (excluding bar columns). No color
  //              calibration needed; the fish moves and the bar (mostly)
  //              doesn't. Tier-2 in our progression.
  type FishTrack = "center" | "color" | "motion";
  let fishTrackMode = $state<FishTrack>(
    (loadSetting("fishTrackMode", "center") as FishTrack)
  );
  $effect(() => { localStorage.setItem("fm:fishTrackMode", JSON.stringify(fishTrackMode)); });
  // Quiet mode: skip M1 clicking entirely while the bar is inside the
  // deadband (already at target). Eliminates the "macro is spamming M1
  // and bar isn't moving" symptom — the bar holds via game physics, not
  // through continuous PWM noise. Default ON because user feedback was
  // explicit that the rapid clicking felt useless when err was already 0.
  let quietMode = $state<boolean>(loadSetting("quietMode", true));
  $effect(() => { localStorage.setItem("fm:quietMode", JSON.stringify(quietMode)); });
  // Always-on-top + visible on all macOS Spaces. Defaults OFF on macOS:
  // we found no combination of NSPanel class swap, fullScreenAuxiliary,
  // and high window level that reliably puts the F1 region picker into
  // a green-button-fullscreen Roblox Space without breaking the WebView
  // (ghosting + cursor not tracked). The status HUD still elevates
  // independently via the elevate command. Users who want the main
  // window above other windows in non-fullscreen scenarios can re-
  // enable from the Extra tab.
  let alwaysOnTop = $state<boolean>(loadSetting("alwaysOnTop", false));
  // Target-window name. When non-empty, the Rust capture path uses
  // window-targeted capture (xcap Window::capture_image, which uses
  // CGWindowListCreateImage on macOS) instead of display capture. This
  // is what makes the macro work when Roblox is in macOS native
  // fullscreen on a different Space — display capture from the macro's
  // Space sees an empty desktop, but window capture follows the window
  // wherever it is. Empty string disables it (use display capture).
  // Default empty = use fast display capture (~25ms/tick). Setting this
  // to a window name routes through xcap window capture which DOES work
  // across macOS Spaces but currently costs ~1-2 SECONDS per tick (xcap
  // captures the full window then we crop). Only enable for fullscreen
  // games on macOS where display capture isn't an option, and accept the
  // perf hit. Future: direct CGWindowListCreateImage FFI with cropped rect
  // would make this fast — see lib.rs comment.
  let targetWindow = $state<string>(
    // Migration: in earlier builds the default was "Roblox" which forced
    // every user into the slow window-capture path. If a user has that
    // exact stale value in localStorage, treat it as "user never set this"
    // and fall back to the new empty default. They can re-set it manually
    // if they actually want window capture.
    (() => {
      const stored = loadSetting<string>("targetWindow", "");
      if (stored === "Roblox") return "";
      return stored;
    })()
  );
  $effect(() => {
    localStorage.setItem("fm:targetWindow", JSON.stringify(targetWindow));
    // Push to Rust on every change so the next tick uses the new target.
    invoke("set_target_window", { name: targetWindow }).catch(() => {});
  });

  // Debug: list all visible windows. Use this when capture isn't finding
  // the game (e.g., fullscreen Roblox shows up under a different name).
  type WindowEntry = {
    app_name: string;
    title: string;
    width: number;
    height: number;
    x: number;
    y: number;
    minimized: boolean;
  };
  let windowList = $state<WindowEntry[]>([]);
  let windowListStatus = $state<string>("");
  async function refreshWindowList() {
    try {
      windowList = await invoke<WindowEntry[]>("list_windows");
      windowListStatus = `Found ${windowList.length} windows.`;
    } catch (e) {
      windowListStatus = `error: ${e}`;
      windowList = [];
    }
  }

  /// Nuclear reset: wipe every fm:* localStorage key and reload. Use when
  /// settings are corrupted from many iterations of feature changes and
  /// you want a clean slate. Hotkeys, calibration, regions — all gone.
  function resetAllSettings() {
    if (typeof localStorage === "undefined") return;
    if (!confirm("Wipe ALL macro settings and reload? Your regions, hotkeys, and calibration will be reset to defaults.")) {
      return;
    }
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith("fm:")) keys.push(k);
    }
    for (const k of keys) localStorage.removeItem(k);
    location.reload();
  }
  $effect(() => { localStorage.setItem("fm:alwaysOnTop", JSON.stringify(alwaysOnTop)); });
  $effect(() => {
    // Apply the always-on-top toggle to the MAIN window only. Overlay
    // and status windows are intentional always-on-top overlays (their
    // purpose is to stay visible during gameplay) — those are pinned
    // by tauri.conf.json and the user's toggle doesn't affect them.
    const t = alwaysOnTop;
    (async () => {
      try {
        const w = await Window.getByLabel("main");
        if (w) await w.setAlwaysOnTop(t);
      } catch {}
    })();
  });
  let debugLogPath = $state<string>("");
  $effect(() => { localStorage.setItem("fm:debugLogging", JSON.stringify(debugLogging)); });
  $effect(() => { localStorage.setItem("fm:dumpFrames", JSON.stringify(dumpFrames)); });

  function resetPidDefaults() {
    // Aligned with Hydra's published Color/Line mode defaults: KP 0.9
    // (we use 0.7 here as our PWM is per-tick rather than steady),
    // KD 0.3. Earlier kp=0.4 / kd=1.5 was too sluggish — the bar
    // couldn't keep up with leftward drift and fish escaped.
    pidKp = 0.5;
    pidKd = 0.3;
    pidDeadband = 30;
    rapidClickMs = 25;
    pwmCycleMs = 100;
    pwmNeutralDuty = 50;
    pwmKp = 0.7;
    pwmMaxSlewPct = 80;
  }

  // Persist whenever any setting changes.
  $effect(() => { localStorage.setItem("fm:dryRun", JSON.stringify(dryRun)); });
  $effect(() => { localStorage.setItem("fm:castDurationMs", JSON.stringify(castDurationMs)); });
  $effect(() => { localStorage.setItem("fm:tickGapMs", JSON.stringify(tickGapMs)); });
  $effect(() => { localStorage.setItem("fm:shakeMaxAvgDiff", JSON.stringify(shakeMaxAvgDiff)); });
  $effect(() => { localStorage.setItem("fm:shakeMode", JSON.stringify(shakeMode)); });
  $effect(() => { localStorage.setItem("fm:enterSpamMs", JSON.stringify(enterSpamMs)); });
  $effect(() => { localStorage.setItem("fm:pidKp", JSON.stringify(pidKp)); });
  $effect(() => { localStorage.setItem("fm:pidKd", JSON.stringify(pidKd)); });
  $effect(() => { localStorage.setItem("fm:pidDeadband", JSON.stringify(pidDeadband)); });
  $effect(() => { localStorage.setItem("fm:rapidClickMs", JSON.stringify(rapidClickMs)); });
  $effect(() => { localStorage.setItem("fm:pwmCycleMs", JSON.stringify(pwmCycleMs)); });
  $effect(() => { localStorage.setItem("fm:pwmNeutralDuty", JSON.stringify(pwmNeutralDuty)); });
  $effect(() => { localStorage.setItem("fm:pwmKp", JSON.stringify(pwmKp)); });
  $effect(() => { localStorage.setItem("fm:pwmMaxSlewPct", JSON.stringify(pwmMaxSlewPct)); });
  $effect(() => { localStorage.setItem("fm:fishTargetHex", JSON.stringify(fishTargetHex)); });
  $effect(() => { localStorage.setItem("fm:fishArrowHex", JSON.stringify(fishArrowHex)); });
  $effect(() => { localStorage.setItem("fm:fishLeftHex", JSON.stringify(fishLeftHex)); });
  $effect(() => { localStorage.setItem("fm:fishRightHex", JSON.stringify(fishRightHex)); });
  $effect(() => { localStorage.setItem("fm:fishTargetTol", JSON.stringify(fishTargetTol)); });
  $effect(() => { localStorage.setItem("fm:fishArrowTol", JSON.stringify(fishArrowTol)); });
  $effect(() => { localStorage.setItem("fm:fishLeftTol", JSON.stringify(fishLeftTol)); });
  $effect(() => { localStorage.setItem("fm:fishRightTol", JSON.stringify(fishRightTol)); });
  $effect(() => { localStorage.setItem("fm:fishWhitePct", JSON.stringify(fishWhitePct)); });
  $effect(() => { localStorage.setItem("fm:fishMinLineDensity", JSON.stringify(fishMinLineDensity)); });
  $effect(() => { localStorage.setItem("fm:fishEdgeTouch", JSON.stringify(fishEdgeTouch)); });
  $effect(() => { localStorage.setItem("fm:fishMergeDistance", JSON.stringify(fishMergeDistance)); });
  $effect(() => { localStorage.setItem("fm:fishMinLineCount", JSON.stringify(fishMinLineCount)); });
  let templateCaptured = $state(false);
  let m1Held = false;
  let lastShakeAt = 0;
  let nextTickTimer: number | null = null;
  let enterSpamTimer: number | null = null;
  // Tri-state M1 control. In Roblox Fisch's reel minigame:
  //   "hold"    — M1 stays pressed, player drifts RIGHT
  //   "release" — M1 stays unpressed, player drifts LEFT
  //   "click"   — M1 alternates rapidly, player roughly STAYS in place
  // The PID's deadband zone uses "click" instead of "do nothing", which is
  // what produces the "stay" behavior the user described.
  type M1Mode = "hold" | "release" | "click";
  let m1Mode: M1Mode = "release";
  let rapidClickTimer: number | null = null;
  // PID controller state for the fish-bar player marker. Reset on every cast.
  let pidPrevError = 0;
  let pidPrevTime = 0;
  // Player position from the previous tick — used to log motion delta so we
  // can tell from the log whether the bar actually moved between ticks (the
  // "stuck at 793 for 9 frames" problem we want a single number for).
  let lastPlayerX: number | null = null;
  // Count of consecutive ticks where the bar didn't move at all. When this
  // crosses LOCK_THRESHOLD we treat it as the game's startup-lock period
  // (bar visible but physics inactive) and force neutral duty to avoid
  // accumulating buffered input that floods out when physics activates.
  let stuckTickCount = 0;
  const LOCK_TICK_THRESHOLD = 3;
  // Bar-visibility transition tracking. We log "reel phase DETECTED" once
  // per cycle, the first tick we see real bar pixels for two ticks running.
  // The streak guards against single-frame false positives from animation.
  // barAbsentStreak counts consecutive low-bar ticks AFTER the bar was first
  // seen — used to detect "reel cycle ended" so we can re-cast.
  let barVisibleStreak = 0;
  let barFirstSeenLogged = false;
  let barFirstSeenAt = 0;
  let barAbsentStreak = 0;
  // How many consecutive ticks of "no bar pixels" after the bar was seen
  // before we conclude the reel cycle is over. Bumped from 12 → 30 after
  // we observed false cycle-ends mid-minigame (the bar transiently
  // disappears during fast movement / when the fish indicator crosses
  // it). With ~250ms ticks, 30 ≈ 7.5s — longer than the minigame's
  // minimum reel time (6.8s per wiki) so we can never false-end on a
  // single-fish gap that's shorter than a real catch.
  const BAR_ABSENT_END_TICKS = 30;
  // Minimum elapsed time since first bar detection before we allow the
  // cycle to end. The wiki says minimum reel time is 6.8s for a perfect
  // catch — a "reel cycle ended" within 7s of detection is almost
  // certainly a transient gap, not a real ending. Defends against very
  // short bar disappearances early in the minigame.
  const MIN_REEL_DURATION_MS = 7000;
  // Cap a reel cycle at this duration. If the bar never appears we still
  // re-cast eventually so a missed bite doesn't lock the macro forever.
  // 60s covers any realistic fish-bite-then-reel time including locked
  // 1.2s + 6.8s minimum reel + buffer for poor cast luck.
  const REEL_CYCLE_TIMEOUT_MS = 60_000;
  // Inter-cycle cooldown so the catch result screen finishes before the
  // next cast. The catch animation + "Caught Fish!" toast notification
  // takes roughly 2.5–3s in practice; if we cast during it the click
  // gets eaten by the modal/toast instead of casting. 3500ms gives
  // generous headroom and lets the input buffer clear too.
  const INTER_CYCLE_COOLDOWN_MS = 3500;
  // Active flag for the tick loop. Separate from `running` because the
  // tick loop pauses between cycles (during cast hold + cooldown) but
  // the macro itself is still running. scheduleNext checks both.
  let tickLoopActive = false;
  // Resolver for the in-progress reel cycle's promise. Called once when
  // the cycle ends (bar gone after seen, or timeout). Set null when not
  // in a reel cycle.
  let reelCycleResolver: ((reason: string) => void) | null = null;
  // Rolling buffer of recent motion_x readings. Used to compute a
  // temporally-smoothed motion target so single-tick spikes (which
  // happen when a frame catches the fish indicator emerging or
  // disappearing) don't yank the controller. Combined with the Rust
  // centroid (spatial smoothing within a tick), this damps both axes
  // of motion noise.
  let motionHistory: number[] = [];
  const MOTION_SMOOTH_N = 3;
  // Lifecycle tracking surfaced to the status overlay.
  //   stage   — high-level macro phase shown in the overlay
  //   cycles  — number of completed cast→reel cycles this session
  let stage = $state<"Idle" | "Casting" | "Reeling" | "Cooldown">("Idle");
  let cycles = $state(0);
  let lastDebug = $state("");
  let lastError = $state("");

  // Push the live state to the status-overlay window. Throttled to once per
  // tick (called from tickOnce) plus state-transition points like cast start
  // and stop.
  async function emitStatus(opts: {
    mode?: "HOLD" | "RELEASE" | "CLICK" | "—";
    err?: number;
    player?: number | null;
    target?: number | null;
    capture_ms?: number;
  } = {}) {
    try {
      await emit("macro-status", {
        running,
        stage,
        mode: opts.mode ?? "—",
        err: opts.err ?? 0,
        player: opts.player ?? null,
        target: opts.target ?? null,
        cycles,
        capture_ms: opts.capture_ms ?? 0,
        keybind_start_stop: stopHotkey,
        keybind_overlay: overlayHotkey,
      });
    } catch {
      // status window may not be open yet on first paint
    }
  }

  // Push status whenever the basic running state, stage, cycles, or hotkey
  // labels change — keeps the overlay's idle display in sync without needing
  // a tick to fire.
  $effect(() => {
    running; stage; cycles; overlayHotkey; stopHotkey;
    emitStatus();
  });

  async function setStatusOverlayVisible(visible: boolean) {
    try {
      const w = await Window.getByLabel("status");
      if (!w) return;
      if (visible) {
        await w.show();
      } else {
        await w.hide();
      }
    } catch {
      // status window may not exist in dev hot-reload edge cases
    }
  }

  async function logDebug(line: string) {
    if (!debugLogging) return;
    try {
      await invoke("debug_log_append", { line });
    } catch {}
  }

  // Best (lowest) match score observed during the current run, plus where
  // it landed. For tuning shakeMaxAvgDiff.
  let bestSeenScore = $state<number | null>(null);
  let bestSeenAt = $state<[number, number] | null>(null);
  let autoSnapshotTaken = false;

  let testResults = $state<Record<RegionKey, string>>({
    shake: "",
    fish_bar: "",
    shake_template: "",
  });

  // Hotkey capture — when the user clicks "rebind", we listen for the next
  // keypress and convert it into a Tauri Accelerator string.
  let capturingFor = $state<"overlay" | "stop" | null>(null);

  function keyEventToAccelerator(e: KeyboardEvent): string | null {
    // Cancel the capture explicitly.
    if (e.key === "Escape") return null;
    const parts: string[] = [];
    if (e.metaKey) parts.push("CommandOrControl");
    if (e.ctrlKey && !e.metaKey) parts.push("Control");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    let main = e.key;
    // Function keys, Arrow keys come through as full names already.
    if (/^F\d+$/.test(main)) {
      // already F1, F2, ..., F12
    } else if (main.length === 1) {
      // Single character — uppercase letters/digits.
      main = main.toUpperCase();
    } else if (main === " ") {
      main = "Space";
    } else {
      // Strip arrow prefix etc.
      main = main.charAt(0).toUpperCase() + main.slice(1);
    }
    // Reject pure-modifier presses (only Shift held, etc.) — wait for an
    // actual non-modifier key.
    if (["Control", "Shift", "Alt", "Meta", "CommandOrControl"].includes(main)) {
      return "__pending__";
    }
    parts.push(main);
    return parts.join("+");
  }

  function startKeybindCapture(which: "overlay" | "stop") {
    capturingFor = which;
  }

  function onKeydownCapture(e: KeyboardEvent) {
    if (capturingFor == null) return;
    e.preventDefault();
    e.stopPropagation();
    const acc = keyEventToAccelerator(e);
    if (acc === "__pending__") return; // wait for non-modifier
    if (acc == null) {
      // Escape — cancel capture.
      capturingFor = null;
      return;
    }
    if (capturingFor === "overlay") overlayHotkey = acc;
    else if (capturingFor === "stop") stopHotkey = acc;
    capturingFor = null;
  }

  function logErr(prefix: string, e: unknown) {
    const msg = `${prefix}: ${e}`;
    lastError = msg;
    console.error(msg);
  }

  function sleep(ms: number) {
    return new Promise<void>((res) => setTimeout(res, ms));
  }

  /// Returns the main macro window's bounds in physical pixels, so we can
  /// auto-exclude it from shake detection (the window otherwise blocks the
  /// area of the screen the macro is trying to scan).
  async function getMainWindowExclude() {
    try {
      const main = await Window.getByLabel("main");
      if (!main) return null;
      const pos = await main.outerPosition();
      const size = await main.outerSize();
      return {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
      };
    } catch {
      return null;
    }
  }

  /// Hides the macro window, waits a moment for the OS to redraw, runs
  /// `action`, then shows the window again. Used so the macro window itself
  /// isn't present in screen captures.
  async function withMainWindowHidden<T>(action: () => Promise<T>): Promise<T> {
    const main = await Window.getByLabel("main");
    if (main) await main.hide();
    // Give the windowserver a beat to compose the next frame without us in it.
    await sleep(180);
    try {
      return await action();
    } finally {
      if (main) {
        await main.show();
        await main.setFocus();
      }
    }
  }

  async function refresh() {
    cfg = await loadRegions();
    try {
      templateCaptured = await invoke<boolean>("has_shake_template");
    } catch {
      templateCaptured = false;
    }
  }

  async function captureTemplate() {
    if (!cfg?.shake_template) {
      testResults.shake_template = "set the Template region first (F1 → drag the purple box tightly around the SHAKE button while it's visible)";
      return;
    }
    const r = cfg.shake_template;
    try {
      await withMainWindowHidden(async () => {
        await invoke<number>("capture_shake_template", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
        });
      });
      templateCaptured = true;
      testResults.shake_template = `template captured (${r.width}×${r.height})`;
    } catch (e) {
      testResults.shake_template = `capture error: ${e}`;
    }
  }

  async function clearTemplate() {
    try {
      await invoke("clear_shake_template");
      templateCaptured = false;
      testResults.shake_template = "template cleared — capture again to enable shake detection";
    } catch (e) {
      testResults.shake_template = `clear error: ${e}`;
    }
  }

  async function saveTemplateImage() {
    try {
      const path = await invoke<string>("save_shake_template_image");
      testResults.shake_template = `template saved to ${path} — open it to verify it's the SHAKE button`;
    } catch (e) {
      testResults.shake_template = `save error: ${e}`;
    }
  }

  /// Debug: dump what cg_capture sees of the full screen, so we can verify
  /// it matches what the user's screen actually shows.
  async function dumpCgFull() {
    try {
      await withMainWindowHidden(async () => {
        const result = await invoke<string>("debug_save_cg_full");
        testResults.shake_template = `[CG DUMP] ${result}`;
      });
    } catch (e) {
      testResults.shake_template = `cg dump error: ${e}`;
    }
  }

  /// Save debug PNGs to ~/Desktop/fisch-macro-debug so we can SEE what's
  /// in the shake region (current frame, baseline, diff visualization).
  async function saveSnapshot() {
    if (!cfg?.shake) return;
    const r = cfg.shake;
    try {
      const path = await withMainWindowHidden(async () => {
        const excludes = [];
        if (cfg!.fish_bar) excludes.push(cfg!.fish_bar);
        return await invoke<string>("save_shake_snapshot", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
          diffThreshold: 150,
          excludes: excludes.length ? excludes : null,
        });
      });
      testResults.shake = `snapshots saved to ${path}`;
    } catch (e) {
      testResults.shake = `snapshot error: ${e}`;
    }
  }

  /// Capture and save the current fish_bar region as a PNG. For ground-truthing
  /// hex values when the in-game rod variant doesn't match Hydra's defaults.
  async function saveFishSnapshot() {
    if (!cfg?.fish_bar) return;
    const r = cfg.fish_bar;
    try {
      const path = await withMainWindowHidden(async () => {
        return await invoke<string>("save_fish_bar_snapshot", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
        });
      });
      testResults.fish_bar = `PNG saved to ${path}`;
    } catch (e) {
      testResults.fish_bar = `snapshot error: ${e}`;
    }
  }

  // ---- Auto-calibration ----
  // The user wants to skip the "save PNG, eyedropper hex, paste back" loop.
  // Auto-calibrate captures the fish_bar region during a live minigame,
  // builds a color histogram, classifies dominant colors, and auto-fills
  // the hex inputs. User can review the suggestions before applying.
  type DominantColor = {
    hex: string;
    pixel_count: number;
    classification: string;
  };
  type CalibrationResult = {
    dominants: DominantColor[];
    suggested_left_bar: string | null;
    suggested_right_bar: string | null;
    suggested_target_line: string | null;
    suggested_arrow: string | null;
    suggested_fish: string | null;
    has_saturated_blue: boolean;
    region_pixels: number;
    region_width: number;
    region_height: number;
    suggested_white_pct: number | null;
    suggested_min_line_density: number | null;
    suggested_bar_tol: number | null;
    suggested_target_tol: number | null;
    bar_max_col_count: number;
    target_max_col_count: number;
    deep_blue_hex: string | null;
    deep_blue_count: number;
  };
  let calResult = $state<CalibrationResult | null>(null);
  let calCountdown = $state<number>(0);
  let calStatus = $state<string>("");

  // Silent in-tick variant — no countdown, no UI churn. Triggered by
  // tickOnce the first time a minigame is detected this session. Calls
  // calibrate_fish_colors against the current fish_bar region, then runs
  // applyCalibrationSuggestions to write hex values + tols + thresholds.
  async function runAutoCalibrationInline() {
    if (!cfg?.fish_bar) return;
    const r = cfg.fish_bar;
    try {
      const result = await invoke<CalibrationResult>("calibrate_fish_colors", {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
      });
      calResult = result;
      applyCalibrationSuggestions();
      calStatus =
        "Auto-calibrated during minigame: " +
        `target=${result.suggested_target_line ?? "?"} ` +
        `bar=${result.suggested_left_bar ?? "?"}/${result.suggested_right_bar ?? "?"} ` +
        `whitePct=${result.suggested_white_pct ?? "?"}`;
      logDebug("auto-calibrate fired: " + calStatus);
    } catch (e) {
      // Silent on failure — user shouldn't see error banners mid-cast.
      logErr("auto-calibrate", e);
    }
  }

  async function runAutoCalibrate() {
    if (!cfg?.fish_bar) {
      calStatus = "No fish_bar region calibrated. Set it on the Main tab first.";
      return;
    }
    calStatus = "Make sure the minigame is visible. Capturing in 3…";
    for (let i = 3; i > 0; i--) {
      calCountdown = i;
      calStatus = `Make sure the minigame is visible. Capturing in ${i}…`;
      await sleep(1000);
    }
    calCountdown = 0;
    const r = cfg.fish_bar;
    try {
      calStatus = "Analyzing…";
      const result = await withMainWindowHidden(async () => {
        return await invoke<CalibrationResult>("calibrate_fish_colors", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
        });
      });
      calResult = result;
      const counts = {
        bar: result.dominants.filter((d) => d.classification === "bar").length,
        dark_blue: result.dominants.filter((d) => d.classification === "dark_blue").length,
        arrow: result.dominants.filter((d) => d.classification === "arrow").length,
        fish: result.dominants.filter((d) => d.classification === "fish").length,
        track: result.dominants.filter((d) => d.classification === "track").length,
      };
      calStatus =
        `Found ${result.dominants.length} dominant colors ` +
        `(${counts.bar} bar, ${counts.dark_blue} divider, ${counts.arrow} arrow, ` +
        `${counts.fish} fish-blue, ${counts.track} track).` +
        (result.has_saturated_blue
          ? ""
          : " ⚠ No saturated blue found — fish indicator likely outside region. Try expanding fish_bar height upward.");
    } catch (e) {
      calStatus = `Calibrate error: ${e}`;
    }
  }

  function applyCalibrationSuggestions() {
    if (!calResult) return;
    if (calResult.suggested_left_bar) fishLeftHex = calResult.suggested_left_bar;
    if (calResult.suggested_right_bar) fishRightHex = calResult.suggested_right_bar;
    if (calResult.suggested_target_line)
      fishTargetHex = calResult.suggested_target_line;
    if (calResult.suggested_arrow) fishArrowHex = calResult.suggested_arrow;
    // If a saturated-blue (fish) hex was found and no dark-blue was found,
    // use fish as Target Line — better to track the actual fish indicator
    // than miss detection entirely.
    if (calResult.suggested_fish && !calResult.suggested_target_line) {
      fishTargetHex = calResult.suggested_fish;
    }
    // Threshold suggestions are derived from observed per-column density
    // and adapt to the user's region size — critical when the user has
    // expanded fish_bar height (Hydra's 80% default is impossible to meet
    // when bar fills only 40% of column height).
    if (calResult.suggested_white_pct != null) {
      fishWhitePct = calResult.suggested_white_pct;
    }
    if (calResult.suggested_min_line_density != null) {
      fishMinLineDensity = calResult.suggested_min_line_density;
    }
    // Apply the suggested tols. Bar tol bridges the gap between left/right
    // hexes (was the root cause of detection failing in log 1777250999 —
    // tol=3 with 16-unit-apart hexes left a dead zone where most bar
    // pixels matched neither). Target tol widens for thin/anti-aliased
    // fish lines.
    if (calResult.suggested_bar_tol != null) {
      fishLeftTol = calResult.suggested_bar_tol;
      fishRightTol = calResult.suggested_bar_tol;
    }
    if (calResult.suggested_target_tol != null) {
      fishTargetTol = calResult.suggested_target_tol;
    }
    // Persist as the user's "saved defaults" so a future Reset restores
    // these calibrated values instead of factory Hydra defaults. The live
    // values are already auto-saved by the $effect blocks; this writes a
    // separate "fm:default:*" snapshot that survives Reset.
    saveAsDefault("fishTargetHex", fishTargetHex);
    saveAsDefault("fishArrowHex", fishArrowHex);
    saveAsDefault("fishLeftHex", fishLeftHex);
    saveAsDefault("fishRightHex", fishRightHex);
    saveAsDefault("fishTargetTol", fishTargetTol);
    saveAsDefault("fishArrowTol", fishArrowTol);
    saveAsDefault("fishLeftTol", fishLeftTol);
    saveAsDefault("fishRightTol", fishRightTol);
    saveAsDefault("fishWhitePct", fishWhitePct);
    saveAsDefault("fishMinLineDensity", fishMinLineDensity);
    calStatus =
      "Applied + saved as your defaults. Review Color Options + Detection Sensitivity, then run a test.";
  }

  function applyDominantTo(hex: string, role: string) {
    if (role === "target") fishTargetHex = hex;
    else if (role === "arrow") fishArrowHex = hex;
    else if (role === "left") fishLeftHex = hex;
    else if (role === "right") fishRightHex = hex;
  }

  /// Expand the fish_bar region upward by 30 px so it captures the fish
  /// icon area above the white bar. The user can call this from the Color
  /// Options card when auto-cal reports "no saturated blue."
  async function expandFishRegionUp() {
    if (!cfg?.fish_bar) return;
    const r = cfg.fish_bar;
    const newY = Math.max(0, r.y - 30);
    const newHeight = r.height + (r.y - newY);
    cfg.fish_bar = { x: r.x, y: newY, width: r.width, height: newHeight };
    try {
      await invoke("save_regions", { config: cfg });
      calStatus = `fish_bar expanded: ${r.height}→${newHeight}px tall. Re-run auto-calibrate.`;
    } catch (e) {
      calStatus = `Could not save expanded region: ${e}`;
    }
  }

  async function toggleOverlay() {
    const overlay = await Window.getByLabel("overlay");
    if (!overlay) return;
    const visible = await overlay.isVisible();
    if (visible) {
      await overlay.hide();
      await refresh();
    } else {
      await overlay.show();
      await overlay.setFocus();
      // Emit AFTER the show so the overlay's listener can immediately
      // grab a fresh screenshot. Without this the overlay re-uses the
      // freeze from its onMount-time capture (which fires at app launch,
      // not at F1 press), producing the "screenshot from a minute ago,
      // doesn't line up with anything" symptom.
      await emit("overlay-shown");
    }
  }

  async function setM1(target: boolean) {
    if (target === m1Held) return;
    m1Held = target;
    try {
      await invoke(target ? "mouse_down" : "mouse_up");
    } catch (e) {
      logErr(target ? "mouse_down" : "mouse_up", e);
    }
  }

  // Rapid-click cadence and Enter-spam loops both run in dedicated Rust
  // threads (start_m1_rapid_click / start_enter_spam) so they tick on a
  // real OS scheduler instead of fighting the JS event loop. JS just
  // signals start/stop; no per-press IPC overhead.
  async function stopRapidClick() {
    if (rapidClickTimer != null) {
      // Legacy JS-side timer cleanup (in case anything still uses it).
      clearInterval(rapidClickTimer);
      rapidClickTimer = null;
    }
    try {
      await invoke("stop_m1_rapid_click");
    } catch {}
  }

  async function setM1Mode(mode: M1Mode) {
    if (mode === m1Mode) return;
    m1Mode = mode;
    if (mode === "hold") {
      await stopRapidClick();
      await setM1(true);
    } else if (mode === "release") {
      await stopRapidClick();
      await setM1(false);
    } else {
      // "click" — Rust thread starts toggling M1 at rapidClickMs cadence.
      // Force m1Held to false here so the JS state matches the Rust thread's
      // expected starting point (it begins with a press).
      m1Held = false;
      try {
        await invoke("start_m1_rapid_click", { intervalMs: rapidClickMs });
      } catch (e) {
        logErr("start_m1_rapid_click", e);
      }
    }
  }

  // PWM-style M1 control. Continuous duty cycle (0-100) replaces the
  // tri-state HOLD/RELEASE/CLICK. Solves overshoot in games with input
  // buffering: instead of binary mode swings, the Rust thread executes
  // a smooth on/off pattern at fine granularity.
  let pwmRunning = false;
  let lastDutyPct: number = 50;
  async function setM1Duty(dutyPct: number) {
    const clamped = Math.max(0, Math.min(100, Math.round(dutyPct)));
    if (!pwmRunning) {
      // Make sure no other M1-actuation paths are active before starting PWM.
      await stopRapidClick();
      m1Held = false;
      try {
        await invoke("start_m1_pwm", {
          dutyPct: clamped,
          cycleMs: pwmCycleMs,
        });
        pwmRunning = true;
        lastDutyPct = clamped;
      } catch (e) {
        logErr("start_m1_pwm", e);
      }
      return;
    }
    if (clamped === lastDutyPct) return;
    lastDutyPct = clamped;
    try {
      await invoke("set_m1_pwm_duty", { dutyPct: clamped });
    } catch (e) {
      logErr("set_m1_pwm_duty", e);
    }
  }
  async function stopPwm() {
    if (!pwmRunning) return;
    pwmRunning = false;
    try {
      await invoke("stop_m1_pwm");
    } catch {}
  }

  async function tickOnce() {
    if (!cfg) return;
    const sh = cfg.shake;
    const fb = cfg.fish_bar;
    if (!sh && !fb) return;

    // The macro window stays visible in both dry and real runs so the user
    // can watch debug output update live. Always exclude its current bounds
    // from detection so it doesn't poison the search region.
    const mainRect = await getMainWindowExclude();
    // In Navigation mode we never run the SHAKE matcher — pass shake: null
    // so the Rust side skips that whole code path. Fish-bar tracking is
    // always run if a fish_bar region is set.
    const sendShake = shakeMode === "template" ? (sh ?? null) : null;
    // Frame dumper. Increment the per-run tick counter and build the path
    // BEFORE the invoke so Rust gets the path for THIS tick's capture. The
    // run dir is set once at macro start (so a single run is one folder).
    let dumpPath: string | null = null;
    if (dumpFrames && fb && dumpFramesRunDir) {
      dumpFramesTick += 1;
      const tickStr = String(dumpFramesTick).padStart(4, "0");
      dumpPath = `${dumpFramesRunDir}/tick-${tickStr}.png`;
    }
    let result: TickResult;
    try {
      result = await invoke<TickResult>("tick_macro", {
        shake: sendShake,
        fishBar: fb ?? null,
        maxAvgDiff: shakeMaxAvgDiff,
        extraExcludes: mainRect ? [mainRect] : null,
        fishParams: fishParamsForTick(),
        dumpFramePath: dumpPath,
      });
    } catch (e) {
      logErr("tick_macro", e);
      return;
    }

    // Bar-visibility gate + reel-cycle-end detection.
    //
    // Logs a one-shot "reel phase DETECTED" line the moment we see real
    // bar pixels for two consecutive ticks. After that, count consecutive
    // ticks where bar pixels drop back to ~zero — when we hit
    // BAR_ABSENT_END_TICKS, the reel cycle is over (catch screen showing,
    // fish escaped, or returned to idle) and we resolve the cycle promise
    // so startMacro can move to the next cast. The 50-pixel threshold is
    // tuned to ignore stray matches from background animation while
    // triggering as soon as a real bar appears.
    const barPixelsTotal = result.fb_total_left + result.fb_total_right;
    if (barPixelsTotal >= 50) {
      barVisibleStreak += 1;
      barAbsentStreak = 0;
      if (barVisibleStreak === 2 && !barFirstSeenLogged) {
        barFirstSeenLogged = true;
        barFirstSeenAt = Date.now();
        logDebug(
          `reel phase DETECTED — bar pixels visible (totL=${result.fb_total_left} totR=${result.fb_total_right})`
        );
      }
    } else {
      barVisibleStreak = 0;
      if (barFirstSeenLogged) {
        barAbsentStreak += 1;
        const elapsedMs = Date.now() - barFirstSeenAt;
        // Two safeguards before declaring the reel cycle over:
        // (1) minimum elapsed time since first detection — a real reel
        //     can't finish in under ~6.8s (wiki), so anything earlier
        //     is a transient gap;
        // (2) sustained bar absence — 30 ticks (~7.5s) of no bar
        //     pixels, longer than typical mid-minigame gaps.
        if (
          elapsedMs >= MIN_REEL_DURATION_MS &&
          barAbsentStreak >= BAR_ABSENT_END_TICKS
        ) {
          logDebug(
            `reel cycle ended — bar absent for ${barAbsentStreak} ticks (${(elapsedMs / 1000).toFixed(1)}s since first seen)`
          );
          endReelCycle("bar gone");
        }
      }
    }

    const dryTag = dryRun ? "[DRY] " : "";

    // ---- Legacy template-matching SHAKE path (only when shakeMode = template) ----
    if (shakeMode === "template" && sh) {
      const score = result.shake_score;
      const thr = result.shake_threshold;
      if (
        result.shake_has_template &&
        result.shake_click &&
        (bestSeenScore == null || score < bestSeenScore)
      ) {
        bestSeenScore = score;
        bestSeenAt = result.shake_click;
      }
      if (
        result.shake_click &&
        result.shake_has_template &&
        score <= thr &&
        Date.now() - lastShakeAt > SHAKE_COOLDOWN_MS
      ) {
        const [cx, cy] = result.shake_click;
        if (!dryRun) {
          const wasHeld = m1Held;
          if (wasHeld) await setM1(false);
          try {
            await invoke("click_at", { x: cx, y: cy });
          } catch (e) {
            logErr("click_at", e);
          }
          if (fb) {
            const fbx = fb.x + Math.floor(fb.width / 2);
            const fby = fb.y + Math.floor(fb.height / 2);
            try {
              await invoke("mouse_move", { x: fbx, y: fby });
            } catch {}
          }
          if (wasHeld) await setM1(true);
        }
        lastShakeAt = Date.now();
        lastDebug = `${dryTag}SHAKE @ (${cx},${cy}) | score ${score} | cap ${result.capture_ms}ms`;
        return;
      }
    }

    // ---- Auto-calibration trigger ----
    // First time we see significant minigame pixels this session, fire a
    // calibration capture in the background and silently apply the
    // suggestions. Doesn't block the tick — runs concurrently. By the next
    // tick, the new colors/tols are live and detection should improve.
    if (autoCalibrate && !hasCalibratedThisSession && cfg?.fish_bar) {
      // Heuristic for "minigame visible": meaningful pixels matching either
      // bar color, OR significant target-line matches in the central area.
      const minigameSeen =
        result.fb_white_cols > 5 ||
        result.fb_total_left + result.fb_total_right > 200 ||
        result.fb_max_grey_per_col > 8;
      if (minigameSeen) {
        hasCalibratedThisSession = true;
        runAutoCalibrationInline();
      }
    }

    // ---- Fish-bar bang-bang control with rapid-click deadband ----
    // Switched from PID to error-only because PID's D term inflates the
    // output when the player is moving fast, causing the controller to
    // toggle direction near the target and oscillate. Pure bang-bang on
    // the error keeps decisions tied to actual position, and the
    // rapid-click third state (managed by Rust) brakes the player
    // smoothly inside the deadband without flipping M1 each tick.
    if (result.player_x != null && result.target_x != null) {
      const px = result.player_x;
      // Pick controller target based on fishTrackMode.
      //   center — fixed bar-region center (Hydra Color/Line strategy)
      //   color  — fish_x from color-line detection (advanced)
      //   motion — fish_x from temporal pixel diff (tier-2)
      const barCenter = cfg?.fish_bar
        ? cfg.fish_bar.x + Math.floor(cfg.fish_bar.width / 2)
        : result.target_x;
      let tx: number;
      if (fishTrackMode === "color") {
        tx = result.target_x ?? barCenter ?? px;
      } else if (fishTrackMode === "motion") {
        // Push current motion_x into the rolling buffer (when found with
        // meaningful peak score) and use the average. This damps the
        // tick-to-tick "left edge → right edge" jitter that happens when
        // the motion blob is wider than 1 px and centroid+score-tie don't
        // perfectly resolve. With Rust's spatial centroid AND this 3-tick
        // temporal average, both axes of jitter are filtered.
        if (result.motion_x != null && result.motion_score >= 4) {
          motionHistory.push(result.motion_x);
          if (motionHistory.length > MOTION_SMOOTH_N) motionHistory.shift();
        }
        if (motionHistory.length > 0) {
          const avg =
            motionHistory.reduce((a, b) => a + b, 0) / motionHistory.length;
          tx = Math.round(avg);
        } else {
          tx = barCenter ?? px;
        }
      } else {
        tx = barCenter ?? px;
      }
      const error = tx - px; // positive ⇒ player too far LEFT, need to push right

      // PWM duty cycle = neutral + kp * error + kd * d_error.
      // 0% duty  ⇒ M1 always up      (bar drifts left)
      // 50% duty ⇒ ~half-on            (varies by game; tuned via neutralDuty)
      // 100% duty ⇒ M1 always down   (bar pushes right)
      const now = Date.now();
      const dt = pidPrevTime > 0 ? Math.max(1, now - pidPrevTime) : 16;
      const dError = pidPrevTime > 0 ? error - pidPrevError : 0;
      pidPrevError = error;
      pidPrevTime = now;
      // Lock-period detection. When the bar appears but physics aren't
      // active yet, we see the same player_x for many ticks. If we send
      // a non-neutral duty during this period, the game's input buffer
      // accumulates that bias and dumps it as a big bar jump the moment
      // physics activate. Force neutral duty during suspected lock to
      // prevent the buffer-accumulation spike.
      const moved = lastPlayerX != null && px !== lastPlayerX;
      if (lastPlayerX == null || moved) {
        stuckTickCount = 0;
      } else {
        stuckTickCount += 1;
      }
      const inLockPeriod = stuckTickCount >= LOCK_TICK_THRESHOLD;
      // Quiet mode: when the bar is inside the deadband AND quietMode is
      // on, STOP clicking. Removes the "M1 spam doing nothing" perception
      // when the bar is already at target. The bar then holds via game
      // physics (or drifts; depends on the rod). PWM only kicks in when
      // the bar drifts out of the deadband. When quietMode is OFF
      // (default), behaves as before — full PWM throughout.
      const inDeadband = Math.abs(error) < pidDeadband;
      const dutyRaw = inLockPeriod
        ? pwmNeutralDuty
        : (quietMode && inDeadband)
          ? 0
          : pwmNeutralDuty + pwmKp * error + pidKd * (dError / dt) * 1000 * 0.05;
      const dutyTarget = Math.max(0, Math.min(100, dutyRaw));
      // Slew-rate limit: when input buffering causes a delayed bar jump,
      // the controller wants to swing duty hard in response. That jump
      // gets buffered too, producing the next bar jump. Capping per-tick
      // delta breaks the amplification loop.
      const prevDuty = pwmRunning ? lastDutyPct : pwmNeutralDuty;
      const maxStep = Math.max(1, pwmMaxSlewPct);
      const delta = dutyTarget - prevDuty;
      const limitedDelta = Math.max(-maxStep, Math.min(maxStep, delta));
      const duty = Math.max(0, Math.min(100, prevDuty + limitedDelta));
      const slewClamped = Math.abs(delta) > maxStep;
      // Verb derived from duty for the human-readable status line.
      let verb: "HOLD" | "RELEASE" | "CLICK";
      if (duty >= 80) verb = "HOLD";
      else if (duty <= 20) verb = "RELEASE";
      else verb = "CLICK";

      if (!dryRun) await setM1Duty(duty);

      const fishLabel =
        result.fish_x != null ? `fish ${result.fish_x}` : `center ${tx}`;
      lastDebug =
        `${dryTag}player ${px} → ${fishLabel} | err ${error.toFixed(0)} ` +
        `| duty ${duty.toFixed(0)}% (${verb}) | cap ${result.capture_ms}ms`;
      emitStatus({
        mode: verb,
        err: Math.round(error),
        player: px,
        target: tx,
        capture_ms: result.capture_ms,
      });
      const motion = lastPlayerX != null ? px - lastPlayerX : 0;
      lastPlayerX = px;
      const topRunsStr = result.fb_top_runs
        .map(([s, l]) => `${s}:${l}`)
        .join(",");
      const slewTag = slewClamped
        ? `(want ${dutyTarget.toFixed(0)}% slewed)`
        : "";
      const lockTag = inLockPeriod ? " [LOCK]" : "";
      logDebug(
        `tick stage=${stage} player=${px} fish=${result.fish_x ?? "null"} target=${tx} err=${Math.round(
          error
        )} duty=${duty.toFixed(0)}%${slewTag}${lockTag} mode=${verb} dx=${motion} cap=${result.capture_ms}ms detect=${result.detect_ms}ms ` +
          `whiteCols=${result.fb_white_cols} runLen=${result.fb_best_run_len} runs=${result.fb_run_count} top=[${topRunsStr}] ` +
          `maxGrey=${result.fb_max_grey_per_col} bestTargetX=${result.fb_best_target_x ?? "null"} ` +
          `tot[L=${result.fb_total_left} R=${result.fb_total_right} T=${result.fb_total_target} A=${result.fb_total_arrow}] ` +
          `motion[x=${result.motion_x ?? "null"} score=${result.motion_score} total=${result.motion_total}]`
      );
    } else if (fb) {
      // Player marker not found this tick (region empty between minigame
      // animation frames, or the white-bar threshold didn't match). Drop
      // duty to 0 so M1 fully releases — safer than holding a stale duty.
      if (!dryRun) await setM1Duty(0);
      lastDebug = `${dryTag}no player marker | cap ${result.capture_ms}ms`;
      emitStatus({ capture_ms: result.capture_ms });
      lastPlayerX = null;
      stuckTickCount = 0;
      motionHistory = [];
      const topRunsStr = result.fb_top_runs
        .map(([s, l]) => `${s}:${l}`)
        .join(",");
      logDebug(
        `tick stage=${stage} player=null target=null mode=release cap=${result.capture_ms}ms detect=${result.detect_ms}ms ` +
          `whiteCols=${result.fb_white_cols} runLen=${result.fb_best_run_len} runs=${result.fb_run_count} top=[${topRunsStr}] ` +
          `maxGrey=${result.fb_max_grey_per_col} bestTargetX=${result.fb_best_target_x ?? "null"} ` +
          `tot[L=${result.fb_total_left} R=${result.fb_total_right} T=${result.fb_total_target} A=${result.fb_total_arrow}] ` +
          `motion[x=${result.motion_x ?? "null"} score=${result.motion_score} total=${result.motion_total}]`
      );
    } else {
      if (!dryRun) await setM1Duty(0);
      lastDebug = `${dryTag}cap ${result.capture_ms}ms`;
      emitStatus({ capture_ms: result.capture_ms });
      logDebug(`tick stage=${stage} (no fish_bar region) cap=${result.capture_ms}ms`);
    }
  }

  // Enter-spam loop. Lives in a dedicated Rust thread so the cadence is
  // driven by an OS sleep instead of the JS event loop, eliminating the
  // 80-100ms scheduling jitter we'd see with setInterval + Tauri IPC.
  async function startEnterSpam() {
    await stopEnterSpam();
    if (shakeMode !== "navigation" || dryRun) return;
    try {
      await invoke("start_enter_spam", {
        intervalMs: Math.max(10, enterSpamMs),
      });
    } catch (e) {
      logErr("start_enter_spam", e);
    }
  }

  async function stopEnterSpam() {
    if (enterSpamTimer != null) {
      clearInterval(enterSpamTimer);
      enterSpamTimer = null;
    }
    try {
      await invoke("stop_enter_spam");
    } catch {}
  }

  function scheduleNext() {
    if (!running || !tickLoopActive) return;
    nextTickTimer = window.setTimeout(async () => {
      if (!running || !tickLoopActive) return;
      await tickOnce();
      scheduleNext();
    }, tickGapMs);
  }

  // Called from tickOnce when bar disappears for BAR_ABSENT_END_TICKS after
  // having been visible — or by the cycle timeout. Resolves the in-flight
  // reel-cycle promise once. Subsequent calls within the same cycle are
  // no-ops (resolver is nulled out after first resolve).
  function endReelCycle(reason: string) {
    const r = reelCycleResolver;
    if (r) {
      reelCycleResolver = null;
      r(reason);
    }
  }

  async function startMacro() {
    if (running) return;
    if (!cfg?.shake && !cfg?.fish_bar) {
      status = "Set at least one region first.";
      return;
    }
    running = true;
    lastError = "";
    bestSeenScore = null;
    bestSeenAt = null;
    autoSnapshotTaken = false;
    pidPrevError = 0;
    pidPrevTime = 0;
    // Auto-calibrate fires once per session when minigame first appears.
    // Reset the gate here so each F3-start gets a fresh calibration pass.
    hasCalibratedThisSession = false;
    motionHistory = [];
    // Reset bar-visibility tracking so each run gets its own transition log.
    barVisibleStreak = 0;
    barFirstSeenLogged = false;
    // Frame dumper: pick a fresh per-run directory based on Unix timestamp,
    // so a single run's frames stay together. The Rust side mkdir -p's the
    // parent on first save, so we don't need to pre-create it here.
    if (dumpFrames) {
      const ts = Math.floor(Date.now() / 1000);
      try {
        dumpFramesRunDir = await invoke<string>("get_frames_run_dir", {
          runTs: ts,
        });
      } catch {
        dumpFramesRunDir = "";
      }
      dumpFramesTick = 0;
    } else {
      dumpFramesRunDir = "";
      dumpFramesTick = 0;
    }
    await setStatusOverlayVisible(true);
    // Hide the main window so the macro stays out of the way of the game.
    // Status overlay (top-left, draggable) shows live state during the run.
    // Restored on stopMacro.
    try {
      const main = await Window.getByLabel("main");
      if (main) await main.hide();
    } catch {}

    // If debug logging is enabled, open a fresh log file and write a header
    // describing the active configuration. This makes the log self-contained
    // for diagnosis without me needing to ask "what were your settings?".
    if (debugLogging) {
      try {
        debugLogPath = await invoke<string>("debug_log_start");
        const cfgSummary = cfg
          ? KEYS.map((k) => {
              const r = cfg![k];
              return r ? `${k}=${r.width}x${r.height}@(${r.x},${r.y})` : `${k}=null`;
            }).join(" ")
          : "no cfg";
        await logDebug(`config: ${cfgSummary}`);
        await logDebug(
          `settings: shakeMode=${shakeMode} enterSpamMs=${enterSpamMs} ` +
            `tickGapMs=${tickGapMs} pidDeadband=${pidDeadband} ` +
            `rapidClickMs=${rapidClickMs} dryRun=${dryRun} castMs=${castDurationMs}`
        );
        await logDebug(
          `pwm: cycleMs=${pwmCycleMs} neutralDuty=${pwmNeutralDuty} ` +
            `kp=${pwmKp} kd=${pidKd} maxSlewPct=${pwmMaxSlewPct}`
        );
        await logDebug(
          `fish_color: target=${fishTargetHex}±${fishTargetTol} ` +
            `arrow=${fishArrowHex}±${fishArrowTol} ` +
            `left=${fishLeftHex}±${fishLeftTol} ` +
            `right=${fishRightHex}±${fishRightTol} ` +
            `whitePct=${fishWhitePct} minLineDensity=${fishMinLineDensity} ` +
            `edgeTouch=${fishEdgeTouch} mergeDist=${fishMergeDistance} ` +
            `minLineCount=${fishMinLineCount}`
        );
        await logDebug(
          `capture: targetWindow="${targetWindow}" ` +
            `mode=${targetWindow ? "WINDOW (slow)" : "DISPLAY (fast)"} ` +
            `trackFish=${fishTrackMode} alwaysOnTop=${alwaysOnTop} ` +
            `quietMode=${quietMode}`
        );
        if (dumpFrames) {
          await logDebug(`frames: dumping to ${dumpFramesRunDir}`);
        }
      } catch (e) {
        logErr("debug_log_start", e);
      }
    }

    // The macro window stays visible in both dry and real runs so the
    // debug line updates where you can see it. tickOnce auto-excludes the
    // window from detection so it doesn't poison the scan. Move it off the
    // game viewport (a corner) before pressing Start.

    if (dryRun) {
      status = `Dry run — detecting only, no clicks. ${stopHotkey} stops.`;
      lastDebug = "dry run started";
      // Dry run skips cast+reel cycling; the tick loop just runs detection
      // continuously so the user can watch the debug output. tickLoopActive
      // gates scheduleNext so we must set it explicitly here.
      tickLoopActive = true;
      scheduleNext();
      return;
    }

    // Countdown so the user can switch focus to the Roblox window.
    for (let i = COUNTDOWN_S; i > 0; i--) {
      if (!running) {
        status = "Stopped.";
        return;
      }
      status = `Starting in ${i}s — focus the Roblox window now (${stopHotkey} aborts).`;
      lastDebug = `countdown ${i}`;
      await sleep(1000);
    }
    if (!running) {
      status = "Stopped.";
      return;
    }

    // Move cursor into the Roblox viewport so M1-down lands on the game,
    // not on this app. Fish-bar center is a safe bet (it's inside Roblox).
    const targetRegion = cfg.fish_bar ?? cfg.shake;
    let castCx = 0;
    let castCy = 0;
    if (targetRegion) {
      castCx = targetRegion.x + Math.floor(targetRegion.width / 2);
      castCy = targetRegion.y + Math.floor(targetRegion.height / 2);
    }

    // Continuous cast→reel→cooldown loop. Each iteration is one fishing
    // attempt. A previous version did a single cast and ran the tick loop
    // forever, which meant catching one fish (or failing once) left the
    // macro idle until manually restarted. Now we loop until the user
    // hits the stop hotkey or running flips false for any reason.
    while (running) {
      // === CAST PHASE ===
      // Re-warp cursor each iteration in case the user (or another app)
      // moved it during the cooldown.
      if (targetRegion) {
        try {
          await invoke("mouse_move", { x: castCx, y: castCy });
        } catch (e) {
          logErr("mouse_move(pre-cast)", e);
        }
        await sleep(80);
      }
      status = `Casting (cycle ${cycles + 1}, holding M1 for ${castDurationMs}ms). ${stopHotkey} aborts.`;
      lastDebug = "casting";
      stage = "Casting";
      cycles += 1;
      await logDebug(`cast start (cycle ${cycles}) holdMs=${castDurationMs}`);
      await setM1(true);
      const castStart = Date.now();
      while (running && Date.now() - castStart < castDurationMs) {
        if (targetRegion) {
          try {
            await invoke("mouse_move", { x: castCx, y: castCy });
          } catch {}
        }
        await sleep(100);
      }
      await setM1(false);
      if (!running) {
        status = "Stopped during cast.";
        break;
      }

      // === REEL PHASE ===
      // Reset per-cycle state so reel-end detection works on a clean
      // slate (the bar must be SEEN this cycle before its absence
      // counts as "cycle ended").
      barVisibleStreak = 0;
      barFirstSeenLogged = false;
      barFirstSeenAt = 0;
      barAbsentStreak = 0;
      lastPlayerX = null;
      stuckTickCount = 0;
      motionHistory = [];
      pidPrevError = 0;
      pidPrevTime = 0;
      hasCalibratedThisSession = false;

      status =
        shakeMode === "navigation"
          ? `Reeling — Enter spam every ${enterSpamMs}ms, PID-tracking fish bar. ${stopHotkey} stops.`
          : `Reeling — template-matching SHAKE, PID-tracking fish bar. ${stopHotkey} stops.`;
      stage = "Reeling";
      await logDebug(`reel phase start`);
      startEnterSpam();
      tickLoopActive = true;
      scheduleNext();

      // Wait for either (a) the bar to appear and then disappear for
      // BAR_ABSENT_END_TICKS — meaning the reel minigame ended — or
      // (b) the cycle timeout, after which we re-cast even if no bar
      // ever appeared (failed cast, no fish bit in time, etc).
      const reelTimeout = setTimeout(() => {
        endReelCycle("timeout");
      }, REEL_CYCLE_TIMEOUT_MS);
      const reason = await new Promise<string>((resolve) => {
        reelCycleResolver = resolve;
      });
      clearTimeout(reelTimeout);
      await logDebug(`reel cycle ended (${reason}) — preparing next cast`);

      // Stop the tick loop and the Enter spam, release M1. M1 must be
      // OFF before the next cast or the next setM1(true) is a no-op.
      tickLoopActive = false;
      if (nextTickTimer != null) {
        clearTimeout(nextTickTimer);
        nextTickTimer = null;
      }
      await stopEnterSpam();
      await stopRapidClick();
      await stopPwm();
      await sleep(50);
      await setM1(false);

      if (!running) break;

      // === COOLDOWN ===
      // Catch result screen / inventory animation finishes and the
      // game returns to a state where casting works again.
      stage = "Cooldown";
      status = `Cooldown ${INTER_CYCLE_COOLDOWN_MS}ms before next cast. ${stopHotkey} stops.`;
      await logDebug(`cooldown ${INTER_CYCLE_COOLDOWN_MS}ms — stopHotkey ${stopHotkey} aborts`);
      // Sleep in 200ms slices instead of one long sleep so we can react
      // to a stop press within ~200ms instead of up to 2s. Not strictly
      // needed but better UX during cooldown.
      const cooldownStart = Date.now();
      while (running && Date.now() - cooldownStart < INTER_CYCLE_COOLDOWN_MS) {
        await sleep(200);
      }
    }
    // If we exited the while(running) loop, log why so debug logs make
    // clear when the cycle stopped. Without this, a silent exit (running
    // flipped false unexpectedly) would just look like "macro stopped"
    // with no record of which iteration / phase ended it.
    await logDebug(`startMacro loop exited (running=${running}, stage=${stage})`);
  }

  async function stopMacro() {
    running = false;
    tickLoopActive = false;
    stage = "Idle";
    await logDebug("stopMacro called");
    // Unblock any awaiter inside the cast→reel cycle loop so startMacro
    // exits cleanly instead of dangling on a never-resolving promise.
    endReelCycle("stopped");
    if (nextTickTimer != null) {
      clearTimeout(nextTickTimer);
      nextTickTimer = null;
    }
    await stopEnterSpam();
    await stopRapidClick();
    await stopPwm();
    // Brief pause so the Rust spam threads notice the stop flag and exit
    // their loops before we send the final mouse_up — otherwise an in-flight
    // toggle from a thread can leave M1 stuck pressed.
    await sleep(50);
    m1Mode = "release";
    await setM1(false);
    await setStatusOverlayVisible(false);
    const main = await Window.getByLabel("main");
    if (main) {
      await main.show();
      await main.setFocus();
    }
    status = "Stopped.";
    if (debugLogging) {
      try {
        await invoke("debug_log_stop");
      } catch {}
    }
  }

  async function castOnce() {
    if (running) return;
    lastError = "";
    status = `Casting once (${castDurationMs}ms)…`;
    await setM1(true);
    await sleep(castDurationMs);
    await setM1(false);
    status = "Cast complete.";
  }

  async function testRegion(key: RegionKey) {
    if (!cfg) return;
    const r = cfg[key];
    if (!r) {
      testResults[key] = "region not set";
      return;
    }
    try {
      if (key === "shake") {
        if (!templateCaptured) {
          testResults[key] = "no template captured — set Template region in F1 overlay, then click 'Capture template'";
          return;
        }
        const res = await withMainWindowHidden(async () => {
          const excludes = [];
          if (cfg!.fish_bar) excludes.push(cfg!.fish_bar);
          return await invoke<ShakeResult>("detect_shake", {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
            maxAvgDiff: shakeMaxAvgDiff,
            excludes: excludes.length ? excludes : null,
          });
        });
        if (res.centroid) {
          testResults[key] = `WOULD FIRE @ (${res.centroid[0]}, ${res.centroid[1]}) — score ${res.score} (≤${res.threshold} = match)`;
        } else {
          testResults[key] = `no match — best score not under threshold ${res.threshold}`;
        }
      } else if (key === "fish_bar") {
        const px = await invoke<number | null>("find_player_x", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
        });
        const targetX = r.x + Math.floor(r.width / 2);
        testResults[key] =
          px == null
            ? "no player marker found"
            : `player_x = ${px}, target (center) = ${targetX}`;
      }
    } catch (e) {
      testResults[key] = `error: ${e}`;
    }
  }

  async function moveCursorToCenter(key: RegionKey) {
    if (!cfg) return;
    const r = cfg[key];
    if (!r) return;
    const cx = r.x + Math.floor(r.width / 2);
    const cy = r.y + Math.floor(r.height / 2);
    try {
      await invoke("mouse_move", { x: cx, y: cy });
      testResults[key] = `cursor moved to physical (${cx}, ${cy})`;
    } catch (e) {
      testResults[key] = `mouse_move error: ${e}`;
    }
  }

  async function clickRegionCenter(key: RegionKey) {
    if (!cfg) return;
    const r = cfg[key];
    if (!r) return;
    const cx = r.x + Math.floor(r.width / 2);
    const cy = r.y + Math.floor(r.height / 2);
    try {
      await invoke("click_at", { x: cx, y: cy });
      testResults[key] = `clicked at physical (${cx}, ${cy}) — did Roblox react?`;
    } catch (e) {
      testResults[key] = `click_at error: ${e}`;
    }
  }

  /// Run shake detection and move cursor to the detected point (no click).
  /// Use this to visually verify whether the algorithm is finding the button.
  async function showShakeDetection() {
    if (!cfg?.shake) return;
    if (!templateCaptured) {
      testResults.shake = "no template captured — capture one first";
      return;
    }
    const r = cfg.shake;
    try {
      const res = await withMainWindowHidden(async () => {
        const excludes = [];
        if (cfg!.fish_bar) excludes.push(cfg!.fish_bar);
        return await invoke<ShakeResult>("detect_shake", {
          x: r.x,
          y: r.y,
          width: r.width,
          height: r.height,
          maxAvgDiff: shakeMaxAvgDiff,
          excludes: excludes.length ? excludes : null,
        });
      });
      if (res.centroid) {
        await invoke("mouse_move", { x: res.centroid[0], y: res.centroid[1] });
        testResults.shake = `cursor → (${res.centroid[0]}, ${res.centroid[1]}) | score ${res.score} (≤${res.threshold} = match)`;
      } else {
        testResults.shake = `no match — best score above threshold ${res.threshold}`;
      }
    } catch (e) {
      testResults.shake = `error: ${e}`;
    }
  }

  // Auto-update banner state. Populated on mount when an update is found at
  // the manifest endpoint configured in tauri.conf.json. The user clicks
  // "Install & restart" to download, verify the signature, and relaunch.
  let updateAvailable = $state<{ version: string; notes?: string } | null>(null);
  let updateInstalling = $state(false);
  let updateError = $state("");

  async function checkForUpdate() {
    // Updater plugin only works in production builds (it has no manifest in
    // dev mode), so silently skip in dev. The check makes a single HTTP
    // request to the latest.json manifest URL.
    try {
      const update = await check();
      if (update) {
        updateAvailable = {
          version: update.version,
          notes: update.body,
        };
      }
    } catch (e) {
      // Don't surface in UI — likely "no manifest" in dev or offline.
      console.warn("update check failed:", e);
    }
  }

  async function installUpdate() {
    updateInstalling = true;
    updateError = "";
    try {
      const update = await check();
      if (!update) {
        updateAvailable = null;
        return;
      }
      await update.downloadAndInstall();
      // On Windows the installer relaunches automatically; on macOS we need
      // to call relaunch() ourselves after the .app bundle is replaced.
      await relaunch();
    } catch (e) {
      updateError = String(e);
      updateInstalling = false;
    }
  }

  // Track the keys we currently have registered with the OS so we can unbind
  // them when the user picks new ones. Without this, changing the keybind
  // would leave the old key still wired to start/stop.
  let registeredOverlayKey: string | null = null;
  let registeredStopKey: string | null = null;

  async function syncHotkeys() {
    try {
      // Unregister anything that's changed.
      if (registeredOverlayKey && registeredOverlayKey !== overlayHotkey) {
        try { await unregister(registeredOverlayKey); } catch {}
        registeredOverlayKey = null;
      }
      if (registeredStopKey && registeredStopKey !== stopHotkey) {
        try { await unregister(registeredStopKey); } catch {}
        registeredStopKey = null;
      }
      // Register what's missing.
      if (registeredOverlayKey !== overlayHotkey) {
        if (await isRegistered(overlayHotkey)) {
          try { await unregister(overlayHotkey); } catch {}
        }
        await register(overlayHotkey, async (event) => {
          if (event.state === "Pressed") await toggleOverlay();
        });
        registeredOverlayKey = overlayHotkey;
      }
      if (registeredStopKey !== stopHotkey) {
        if (await isRegistered(stopHotkey)) {
          try { await unregister(stopHotkey); } catch {}
        }
        await register(stopHotkey, async (event) => {
          // Toggle: same key starts when idle, stops when running.
          if (event.state === "Pressed") {
            if (running) {
              await stopMacro();
            } else {
              await startMacro();
            }
          }
        });
        registeredStopKey = stopHotkey;
      }
      hotkeyOk = true;
    } catch (e) {
      hotkeyOk = false;
      status = `Could not register hotkeys: ${e}`;
    }
  }

  // Re-sync hotkeys whenever the user picks new ones.
  $effect(() => {
    overlayHotkey; stopHotkey;
    syncHotkeys();
  });

  onMount(async () => {
    await refresh();
    await syncHotkeys();
    // Window elevation (fullScreenAuxiliary + high level) is handled by
    // the alwaysOnTop $effect — it must run AFTER Tauri's
    // setVisibleOnAllWorkspaces or that call resets collectionBehavior.
    // Don't await — let the update check run in the background so it doesn't
    // block first paint.
    checkForUpdate();
  });

  onDestroy(async () => {
    if (running) await stopMacro();
    if (registeredOverlayKey) {
      try { await unregister(registeredOverlayKey); } catch {}
    }
    if (registeredStopKey) {
      try { await unregister(registeredStopKey); } catch {}
    }
  });
</script>

<svelte:window on:keydown={onKeydownCapture} />

<div class="app">
  <header class="app-header">
    <div class="brand">
      <span class="brand-icon">🎣</span>
      <h1>Fisch Macro</h1>
      <span class="version">v0.1.0</span>
    </div>
    <div class="status-pill" class:on={running}>
      <span class="status-dot"></span>
      {running ? "Running" : "Idle"}
    </div>
  </header>

  {#if !hotkeyOk}
    <div class="hotkey-banner">
      <div class="hotkey-banner-body">
        <strong>Hotkey {stopHotkey} couldn't register</strong>
        <span> — another app or the OS is holding it. You can still use
        the on-screen Start/Stop buttons. To use a hotkey, pick a different
        one below or rebind from the Main tab.</span>
      </div>
      <div class="hotkey-banner-actions">
        {#each ["F6", "F7", "F8", "F9"] as candidate}
          {#if candidate !== stopHotkey}
            <button class="btn btn-sm" onclick={() => { stopHotkey = candidate; }}>
              Use {candidate}
            </button>
          {/if}
        {/each}
      </div>
    </div>
  {/if}

  <nav class="tabs">
    {#each TABS as t}
      <button
        class="tab"
        class:active={activeTab === t.id}
        onclick={() => (activeTab = t.id)}
      >
        {t.label}
      </button>
    {/each}
  </nav>

  {#if updateAvailable}
    <div class="banner update-banner">
      <div>
        <strong>Update available — v{updateAvailable.version}</strong>
        {#if updateAvailable.notes}<div class="banner-sub">{updateAvailable.notes}</div>{/if}
      </div>
      {#if updateError}<div class="banner-error">{updateError}</div>{/if}
      <div class="banner-actions">
        <button class="btn btn-primary" onclick={installUpdate} disabled={updateInstalling}>
          {updateInstalling ? "Installing…" : "Install & restart"}
        </button>
        <button class="btn" onclick={() => (updateAvailable = null)} disabled={updateInstalling}>Later</button>
      </div>
    </div>
  {/if}

  <main class="content">
    {#if activeTab === "main"}
      <!-- Run controls — the thing you press most -->
      <div class="card">
        <div class="action-row">
          {#if running}
            <button class="btn btn-danger btn-lg" onclick={stopMacro}>Stop ({stopHotkey})</button>
          {:else}
            <button class="btn btn-primary btn-lg" onclick={startMacro}>
              {dryRun ? "Start (dry run)" : "Start"}
            </button>
            <button class="btn" onclick={castOnce} disabled={running}>Cast once</button>
          {/if}
        </div>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={dryRun} disabled={running} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Dry run — observe only, no clicks</span>
        </label>
      </div>

      <!-- Regions (calibration) — second-most-pressed thing -->
      <div class="card">
        <div class="card-header">
          <h2>Regions</h2>
          <button class="btn btn-sm" onclick={toggleOverlay}>Open picker ({overlayHotkey})</button>
        </div>
        {#if cfg}
          <div class="region-list">
            {#each KEYS as key}
              {@const r = cfg[key] as Region | null}
              <div class="region-item">
                <span class="region-dot" style:background={REGION_META[key].color}></span>
                <span class="region-name">{REGION_META[key].label}</span>
                {#if r}
                  <span class="region-meta">{r.width}×{r.height} @ ({r.x},{r.y})</span>
                  <span class="badge badge-ok">set</span>
                {:else}
                  <span class="badge badge-missing">not set</span>
                {/if}
              </div>
            {/each}
          </div>
        {:else}
          <p class="meta">Loading…</p>
        {/if}
      </div>

      <!-- Hotkeys -->
      <div class="card">
        <div class="card-header"><h2>Hotkeys</h2></div>
        <div class="field-row">
          <span class="field-label">Start / Stop</span>
          <button
            class="btn btn-sm hotkey-btn"
            class:capturing={capturingFor === "stop"}
            onclick={() => startKeybindCapture("stop")}
          >
            {capturingFor === "stop" ? "Press a key…" : stopHotkey}
          </button>
        </div>
        <div class="field-row">
          <span class="field-label">Open region picker</span>
          <button
            class="btn btn-sm hotkey-btn"
            class:capturing={capturingFor === "overlay"}
            onclick={() => startKeybindCapture("overlay")}
          >
            {capturingFor === "overlay" ? "Press a key…" : overlayHotkey}
          </button>
        </div>
        <p class="field-help">
          Click a binding, then press the key. Esc cancels. Function keys
          (F1–F12) avoid collisions with Roblox movement.
          {#if !hotkeyOk}<br /><span class="warn">⚠ hotkeys not registered — pick different keys.</span>{/if}
        </p>
      </div>
    {/if}

    {#if activeTab === "tuning"}
      <!-- Cast -->
      <div class="card">
        <div class="card-header"><h2>Cast</h2></div>
        <div class="field-row">
          <label class="field-label" for="cast-hold">Hold duration (ms)</label>
          <input id="cast-hold" type="number" bind:value={castDurationMs} min="100" step="100" disabled={running} />
        </div>
        <p class="field-help">How long to hold M1 during the cast phase. 1000 ms is fine for most rods.</p>
      </div>

      <!-- Bar tracking strategy -->
      <div class="card">
        <div class="card-header"><h2>Bar tracking</h2></div>
        <div class="field-row">
          <label class="field-label" for="fish-mode">Strategy</label>
          <select id="fish-mode" bind:value={fishTrackMode}>
            <option value="center">Hold bar at center (default)</option>
            <option value="motion">Motion detection</option>
            <option value="color">Color tracking</option>
          </select>
        </div>
        <p class="field-help">
          <strong>Center</strong> holds the bar at the slider midpoint and lets game physics bring the
          fish to you — most reliable across rods. <strong>Motion</strong> chases pixel motion; works
          without color calibration but oscillates more. <strong>Color</strong> follows the detected
          fish-line color; only useful after a known-good Target Line hex is set.
        </p>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={quietMode} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Quiet mode (skip clicks inside deadband)</span>
        </label>
      </div>

      <!-- Calibration -->
      <div class="card">
        <div class="card-header">
          <h2>Calibration</h2>
          <button
            class="btn btn-sm btn-primary"
            onclick={runAutoCalibrate}
            disabled={calCountdown > 0}
          >
            {calCountdown > 0 ? `${calCountdown}…` : "Capture & analyze"}
          </button>
        </div>
        <p class="field-help">
          Run during an active reel minigame. Captures the fish_bar region and suggests color hex / tolerance
          values matched to your rod and lighting.
          {#if calStatus}<br /><br /><span class="cal-status-text">{calStatus}</span>{/if}
        </p>
        {#if calResult}
          <details class="cal-results-wrap">
            <summary>Last calibration details</summary>
            <div class="cal-actions">
              <button class="btn btn-sm" onclick={applyCalibrationSuggestions}>Re-apply suggestions</button>
              {#if !calResult.has_saturated_blue}
                <button class="btn btn-sm" onclick={expandFishRegionUp}>Expand region up 30 px</button>
              {/if}
            </div>
            <div class="cal-suggested">
              <div class="cal-suggested-row">
                <span class="cal-role">Target Line:</span>
                {#if calResult.suggested_target_line}
                  <span class="color-swatch" style:background={"#" + calResult.suggested_target_line}></span>
                  <code>#{calResult.suggested_target_line}</code>
                {:else if calResult.suggested_fish}
                  <span class="color-swatch" style:background={"#" + calResult.suggested_fish}></span>
                  <code>#{calResult.suggested_fish}</code>
                  <span class="cal-note">(fish-blue fallback)</span>
                {:else}
                  <span class="cal-none">— not detected</span>
                {/if}
              </div>
              <div class="cal-suggested-row">
                <span class="cal-role">Left Bar:</span>
                {#if calResult.suggested_left_bar}
                  <span class="color-swatch" style:background={"#" + calResult.suggested_left_bar}></span>
                  <code>#{calResult.suggested_left_bar}</code>
                {:else}
                  <span class="cal-none">— not detected</span>
                {/if}
              </div>
              <div class="cal-suggested-row">
                <span class="cal-role">Right Bar:</span>
                {#if calResult.suggested_right_bar}
                  <span class="color-swatch" style:background={"#" + calResult.suggested_right_bar}></span>
                  <code>#{calResult.suggested_right_bar}</code>
                {:else}
                  <span class="cal-none">— not detected</span>
                {/if}
              </div>
              <div class="cal-suggested-row">
                <span class="cal-role">White %:</span>
                {#if calResult.suggested_white_pct != null}
                  <code>{calResult.suggested_white_pct}%</code>
                {:else}
                  <span class="cal-none">— couldn't measure</span>
                {/if}
              </div>
            </div>
            <details class="cal-dominants-wrap">
              <summary>All {calResult.dominants.length} dominant colors</summary>
              <div class="cal-dominants">
                {#each calResult.dominants as d}
                  <div class="cal-dom-row">
                    <span class="color-swatch" style:background={"#" + d.hex}></span>
                    <code class="cal-hex">#{d.hex}</code>
                    <span class="cal-class">{d.classification}</span>
                    <span class="cal-count">{d.pixel_count} px</span>
                    <span class="cal-assign">
                      <button class="btn btn-xs" onclick={() => applyDominantTo(d.hex, "target")}>Target</button>
                      <button class="btn btn-xs" onclick={() => applyDominantTo(d.hex, "left")}>Left</button>
                      <button class="btn btn-xs" onclick={() => applyDominantTo(d.hex, "right")}>Right</button>
                    </span>
                  </div>
                {/each}
              </div>
            </details>
          </details>
        {/if}
      </div>

      <!-- Fish colors -->
      <div class="card">
        <div class="card-header">
          <h2>Fish colors</h2>
          <div class="header-actions">
            <button class="btn btn-sm" onclick={resetFishColorDefaults} title="Restore your last applied calibration">Reset</button>
            <button class="btn btn-sm" onclick={resetFishColorFactory} title="Discard saved calibration and restore Hydra factory defaults">Factory</button>
          </div>
        </div>
        <div class="color-row">
          <span class="color-label">Target line</span>
          <span class="color-swatch" style:background={"#" + fishTargetHex}></span>
          <input class="hex-input" type="text" bind:value={fishTargetHex} maxlength="7" />
          <input class="tol-input" type="number" bind:value={fishTargetTol} min="0" max="50" />
          <span class="tol-label">tol</span>
        </div>
        <div class="color-row">
          <span class="color-label">Left bar</span>
          <span class="color-swatch" style:background={"#" + fishLeftHex}></span>
          <input class="hex-input" type="text" bind:value={fishLeftHex} maxlength="7" />
          <input class="tol-input" type="number" bind:value={fishLeftTol} min="0" max="50" />
          <span class="tol-label">tol</span>
        </div>
        <div class="color-row">
          <span class="color-label">Right bar</span>
          <span class="color-swatch" style:background={"#" + fishRightHex}></span>
          <input class="hex-input" type="text" bind:value={fishRightHex} maxlength="7" />
          <input class="tol-input" type="number" bind:value={fishRightTol} min="0" max="50" />
          <span class="tol-label">tol</span>
        </div>
        <p class="field-help">
          RGB hex (no #) and per-channel tolerance. Widen tolerance if matches fail in the debug log
          (<code>maxGrey=0</code> or <code>tot[L=0 R=0]</code>); rod variants and lighting can shift the in-game color.
        </p>
      </div>

      <!-- Detection thresholds — collapsed by default -->
      <details class="card collapsible">
        <summary class="card-header"><h2>Detection thresholds</h2></summary>
        <div class="card-body">
          <div class="field-row">
            <label class="field-label" for="white-pct">White % required</label>
            <input id="white-pct" type="number" bind:value={fishWhitePct} min="10" max="100" step="5" />
          </div>
          <div class="field-row">
            <label class="field-label" for="min-line-density">Min line density (%)</label>
            <input id="min-line-density" type="number" bind:value={fishMinLineDensity} min="10" max="100" step="5" />
          </div>
          <div class="field-row">
            <label class="field-label" for="edge-touch">Edge touch (px)</label>
            <input id="edge-touch" type="number" bind:value={fishEdgeTouch} min="0" max="50" step="1" />
          </div>
          <div class="field-row">
            <label class="field-label" for="merge-distance">Merge distance (px)</label>
            <input id="merge-distance" type="number" bind:value={fishMergeDistance} min="0" max="20" step="1" />
          </div>
          <div class="field-row">
            <label class="field-label" for="min-line-count">Min line count</label>
            <input id="min-line-count" type="number" bind:value={fishMinLineCount} min="1" max="200" step="1" />
          </div>
          <p class="field-help">
            Hydra defaults: White% 80, Density 80, Edge 1, Merge 2, Count 4. Lower the percentages if
            the bar isn't being detected; raise them if false positives appear.
          </p>
        </div>
      </details>
    {/if}

    {#if activeTab === "advanced"}
      <!-- PWM control -->
      <details class="card collapsible" open>
        <summary class="card-header">
          <h2>PWM control</h2>
          <button class="btn btn-sm" onclick={resetPidDefaults}>Reset</button>
        </summary>
        <div class="card-body">
          <div class="field-row">
            <label class="field-label" for="pwm-neutral">Neutral duty (%)</label>
            <input id="pwm-neutral" type="number" bind:value={pwmNeutralDuty} min="0" max="100" step="5" />
          </div>
          <div class="field-row">
            <label class="field-label" for="pwm-kp">PWM Kp</label>
            <input id="pwm-kp" type="number" bind:value={pwmKp} min="0" step="0.05" />
          </div>
          <div class="field-row">
            <label class="field-label" for="pid-kd">Kd</label>
            <input id="pid-kd" type="number" bind:value={pidKd} min="0" step="0.1" />
          </div>
          <div class="field-row">
            <label class="field-label" for="pwm-cycle">Cycle (ms)</label>
            <input id="pwm-cycle" type="number" bind:value={pwmCycleMs} min="10" max="200" step="5" />
          </div>
          <div class="field-row">
            <label class="field-label" for="pwm-slew">Max slew (% / tick)</label>
            <input id="pwm-slew" type="number" bind:value={pwmMaxSlewPct} min="5" max="100" step="5" />
          </div>
          <div class="field-row">
            <label class="field-label" for="tick-gap">Tick gap (ms)</label>
            <input id="tick-gap" type="number" bind:value={tickGapMs} min="20" step="10" />
          </div>
          <div class="field-row">
            <label class="field-label" for="pid-deadband">Deadband (px)</label>
            <input id="pid-deadband" type="number" bind:value={pidDeadband} min="0" step="1" />
          </div>
          <p class="field-help">
            M1 is held as a continuous duty cycle. Neutral is the resting duty (tune up if bar drifts left,
            down if right). Kp and Kd shape how aggressively duty deviates from neutral per pixel of error.
          </p>
        </div>
      </details>

      <!-- SHAKE handling -->
      <details class="card collapsible">
        <summary class="card-header"><h2>SHAKE handling</h2></summary>
        <div class="card-body">
          <div class="field-row">
            <label class="field-label" for="shake-style">Mode</label>
            <select id="shake-style" bind:value={shakeMode} disabled={running}>
              <option value="navigation">Navigation (Enter spam)</option>
              <option value="template">Template (image match)</option>
            </select>
          </div>
          {#if shakeMode === "navigation"}
            <div class="field-row">
              <label class="field-label" for="enter-spam">Enter spam delay (ms)</label>
              <input id="enter-spam" type="number" bind:value={enterSpamMs} min="20" step="20" />
            </div>
            <p class="field-help">
              Spams Enter during the reel phase. Roblox Fisch accepts Enter as the SHAKE input
              (via camera-button trick), so no image detection is needed. Default for all users.
            </p>
          {:else}
            <div class="field-row">
              <label class="field-label" for="shake-strictness">Match strictness</label>
              <input id="shake-strictness" type="number" bind:value={shakeMaxAvgDiff} min="0" max="255" step="5" />
            </div>
            <p class="field-help">
              Legacy template matching. Capture a SHAKE button image via the F1 overlay's Template region.
              Lower strictness ⇒ stricter match. {templateCaptured ? "Template captured." : "No template captured yet."}
            </p>
          {/if}
        </div>
      </details>

      <!-- Window behavior -->
      <div class="card">
        <div class="card-header"><h2>Window behavior</h2></div>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={alwaysOnTop} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Pin macro window above other windows</span>
        </label>
        <p class="field-help">
          Affects the main macro window only. The status HUD and F1 region picker are always-on-top by design.
        </p>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={autoCalibrate} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Auto-calibrate when minigame first appears</span>
        </label>
        <p class="field-help">
          OFF by default — testing showed auto-cal frequently <em>worsens</em> detection by replacing
          factory hex values with histogram-derived ones that miss anti-aliased edges. Leave off unless
          your rod's bar fails detection completely.
        </p>
      </div>

      <!-- Debug -->
      <div class="card">
        <div class="card-header"><h2>Debug</h2></div>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={debugLogging} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Record debug log to file</span>
        </label>
        <p class="field-help">
          Writes every tick + lifecycle transition to <code>~/Desktop/fisch-macro-debug/macro-log-&lt;ts&gt;.txt</code>.
          {#if debugLogPath}<br /><br />Last log: <code>{debugLogPath}</code>{/if}
        </p>
        <label class="toggle-row">
          <input type="checkbox" bind:checked={dumpFrames} />
          <span class="toggle-track"><span class="toggle-thumb"></span></span>
          <span class="toggle-label">Dump fish_bar PNG every tick</span>
        </label>
        <p class="field-help">
          Saves the captured fish_bar region as a PNG every tick — to verify detection is scanning the
          right pixels. Costs ~1ms per tick; off when not debugging.
        </p>
      </div>

      <!-- Region tools (testing) -->
      {#if cfg}
        <div class="card">
          <div class="card-header"><h2>Region tools</h2></div>
          {#each KEYS as key}
            {@const r = cfg[key] as Region | null}
            {#if r}
              <div class="tools-row">
                <span class="region-dot" style:background={REGION_META[key].color}></span>
                <span class="region-name">{REGION_META[key].label}</span>
                <button class="btn btn-sm" onclick={() => testRegion(key)}>Test</button>
                <button class="btn btn-sm" onclick={() => moveCursorToCenter(key)}>Move cursor</button>
                <button class="btn btn-sm" onclick={() => clickRegionCenter(key)}>Click</button>
                {#if key === "shake"}
                  <button class="btn btn-sm" onclick={showShakeDetection}>Show detect</button>
                  <button class="btn btn-sm" onclick={saveSnapshot}>Save snapshot</button>
                {/if}
                {#if key === "fish_bar"}
                  <button class="btn btn-sm" onclick={saveFishSnapshot}>Save PNG</button>
                {/if}
                {#if key === "shake_template"}
                  <button class="btn btn-sm" onclick={captureTemplate}>
                    {templateCaptured ? "Recapture" : "Capture"}
                  </button>
                  {#if templateCaptured}
                    <button class="btn btn-sm" onclick={saveTemplateImage}>Save PNG</button>
                    <button class="btn btn-sm" onclick={clearTemplate}>Clear</button>
                  {/if}
                  <button class="btn btn-sm" onclick={dumpCgFull}>Debug CG</button>
                {/if}
              </div>
              {#if testResults[key]}
                <div class="test-out">{testResults[key]}</div>
              {/if}
            {/if}
          {/each}
        </div>
      {/if}

      <!-- Target window (rare) -->
      <details class="card collapsible">
        <summary class="card-header"><h2>Target window</h2></summary>
        <div class="card-body">
          <div class="field-row">
            <label class="field-label" for="target-window">Window name</label>
            <input id="target-window" type="text" bind:value={targetWindow} placeholder="" />
          </div>
          <p class="field-help">
            Leave empty for fast display capture. Setting this routes capture through window-targeted
            mode (works across virtual desktops / Spaces) but is significantly slower.
          </p>
          <div class="cal-actions">
            <button class="btn btn-sm" onclick={refreshWindowList}>List visible windows</button>
          </div>
          {#if windowListStatus}
            <p class="field-help" style="padding-top:0;">{windowListStatus}</p>
          {/if}
          {#if windowList.length > 0}
            <div class="window-list">
              {#each windowList as w}
                <div class="window-row">
                  <code class="app-name">{w.app_name || "(no app)"}</code>
                  <span class="title">{w.title || "(no title)"}</span>
                  <span class="dims">{w.width}×{w.height}{w.minimized ? " · min" : ""}</span>
                  <button class="btn btn-xs" onclick={() => { targetWindow = w.app_name || w.title; }}>Use</button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </details>

      <!-- Reset -->
      <div class="card">
        <div class="card-header"><h2>Reset</h2></div>
        <button class="btn btn-danger" onclick={resetAllSettings}>Wipe all settings & reload</button>
        <p class="field-help">
          Clears every <code>fm:*</code> localStorage key and reloads. Use when state is corrupted from
          many iterations. Hotkeys, regions, calibration — all gone.
        </p>
      </div>
    {/if}

  </main>

  <footer class="status-bar">
    <div class="debug-line">{lastDebug || "—"}</div>
    {#if lastError}<div class="err">⚠ {lastError}</div>{/if}
    <div class="status-text">{status}</div>
  </footer>
</div>

<style>
  :global(html, body) {
    margin: 0;
    background: #0a0b0d;
    color: #e8e9ec;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "Inter", system-ui, sans-serif;
    font-size: 15px;
    line-height: 1.45;
    -webkit-font-smoothing: antialiased;
    text-rendering: optimizeLegibility;
  }
  :global(*) { box-sizing: border-box; }
  :global(button, input, select) { font-family: inherit; font-size: inherit; }

  .app {
    max-width: 760px;
    margin: 0 auto;
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    padding-bottom: 4.5rem; /* leaves space for sticky status-bar */
  }

  /* ---------- Header ---------- */
  .app-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1.5rem 1.75rem 1rem;
  }
  .brand { display: flex; align-items: center; gap: 0.7rem; }
  .brand-icon { display: none; } /* removed, cleaner without */
  .brand h1 {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 600;
    letter-spacing: -0.015em;
  }
  .version {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.72rem;
    color: #6c7077;
    padding: 0.18rem 0.45rem;
    border: 1px solid #26282c;
    border-radius: 4px;
  }

  /* ---------- Banners ---------- */
  .hotkey-banner, .update-banner {
    margin: 0 1.75rem 1rem;
    padding: 0.85rem 1.1rem;
    border-radius: 8px;
    font-size: 0.88rem;
    line-height: 1.5;
  }
  .hotkey-banner {
    background: #2a1414;
    border: 1px solid #5a2424;
    color: #ffd0d0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .hotkey-banner strong { color: #ffeaea; }
  .hotkey-banner-body { flex: 1; min-width: 240px; }
  .hotkey-banner-actions {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    flex-shrink: 0;
  }
  .update-banner {
    background: #14223a;
    border: 1px solid #2a4a7a;
    color: #cfe2ff;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .update-banner strong { color: #ffffff; }
  .banner-sub { font-size: 0.8rem; color: #9ca0a6; margin-top: 0.25rem; }
  .banner-error { color: #f5a3a3; font-size: 0.8rem; }
  .banner-actions { display: flex; gap: 0.5rem; flex-shrink: 0; }

  /* ---------- Status pill ---------- */
  .status-pill {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.75rem;
    background: #16181b;
    border: 1px solid #26282c;
    border-radius: 999px;
    font-size: 0.8rem;
    color: #9ca0a6;
  }
  .status-pill.on { color: #65c97e; border-color: #2c4d36; background: #16221b; }
  .status-dot {
    width: 7px; height: 7px;
    border-radius: 50%;
    background: #4d5158;
  }
  .status-pill.on .status-dot { background: #65c97e; box-shadow: 0 0 6px rgba(101, 201, 126, 0.5); }

  /* ---------- Tabs ---------- */
  .tabs {
    display: flex;
    gap: 0.25rem;
    padding: 0 1.75rem;
    border-bottom: 1px solid #1d1f22;
    margin-bottom: 1.25rem;
  }
  .tab {
    background: transparent;
    border: none;
    color: #9ca0a6;
    padding: 0.7rem 1rem;
    font-size: 0.9rem;
    font-weight: 500;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    transition: color 0.12s, border-color 0.12s;
  }
  .tab:hover { color: #e8e9ec; }
  .tab.active {
    color: #e8e9ec;
    border-bottom-color: #5b9bf5;
  }

  /* ---------- Content ---------- */
  .content {
    padding: 0 1.75rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    flex: 1;
  }

  /* ---------- Cards ---------- */
  .card {
    background: #16181b;
    border: 1px solid #26282c;
    border-radius: 10px;
    padding: 1.25rem 1.5rem;
  }
  .card.collapsible {
    padding: 0;
  }
  .card.collapsible > .card-header {
    padding: 1rem 1.5rem;
    cursor: pointer;
    margin-bottom: 0;
    user-select: none;
  }
  .card.collapsible[open] > .card-header {
    border-bottom: 1px solid #26282c;
  }
  .card.collapsible > .card-header::-webkit-details-marker { display: none; }
  .card.collapsible > .card-header::before {
    content: "▸";
    margin-right: 0.5rem;
    color: #6c7077;
    font-size: 0.7rem;
    transition: transform 0.15s;
    display: inline-block;
  }
  .card.collapsible[open] > .card-header::before {
    transform: rotate(90deg);
  }
  .card.collapsible > .card-body {
    padding: 1.25rem 1.5rem;
  }
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.85rem;
    gap: 0.75rem;
  }
  .card-header h2 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 600;
    letter-spacing: -0.005em;
    color: #e8e9ec;
  }
  .header-actions { display: flex; gap: 0.4rem; }

  /* ---------- Buttons ---------- */
  .btn {
    background: #1f2226;
    border: 1px solid #2c2f34;
    color: #e8e9ec;
    padding: 0.5rem 0.95rem;
    border-radius: 6px;
    font-size: 0.85rem;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.12s, border-color 0.12s;
  }
  .btn:hover:not(:disabled) {
    background: #25282d;
    border-color: #353941;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .btn-primary {
    background: #1f4ea0;
    border-color: #2a5fbb;
    color: #ffffff;
  }
  .btn-primary:hover:not(:disabled) {
    background: #2659ba;
    border-color: #3a72d0;
  }
  .btn-danger {
    background: #6b1f1f;
    border-color: #8b2a2a;
    color: #ffe7e7;
  }
  .btn-danger:hover:not(:disabled) {
    background: #832525;
    border-color: #a13030;
  }
  .btn-sm {
    padding: 0.32rem 0.7rem;
    font-size: 0.78rem;
  }
  .btn-xs {
    padding: 0.18rem 0.5rem;
    font-size: 0.7rem;
  }
  .btn-lg {
    padding: 0.7rem 1.5rem;
    font-size: 0.95rem;
    font-weight: 600;
  }

  .action-row {
    display: flex;
    gap: 0.6rem;
    margin-bottom: 0.85rem;
    align-items: center;
  }

  /* ---------- Toggles ---------- */
  .toggle-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.55rem 0;
    cursor: pointer;
    font-size: 0.88rem;
  }
  .toggle-row input[type="checkbox"] {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .toggle-track {
    width: 34px;
    height: 18px;
    background: #2a2d33;
    border-radius: 999px;
    position: relative;
    transition: background 0.15s;
    flex-shrink: 0;
  }
  .toggle-thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    background: #6c7077;
    border-radius: 50%;
    transition: left 0.15s, background 0.15s;
  }
  .toggle-row input:checked ~ .toggle-track {
    background: #2a5fbb;
  }
  .toggle-row input:checked ~ .toggle-track .toggle-thumb {
    left: 18px;
    background: #ffffff;
  }
  .toggle-row input:disabled ~ .toggle-track { opacity: 0.4; }
  .toggle-label { color: #d3d5d9; }

  /* ---------- Field rows ---------- */
  .field-row {
    display: flex;
    align-items: center;
    gap: 0.85rem;
    padding: 0.45rem 0;
  }
  .field-label {
    flex: 1;
    color: #b8bbc1;
    font-size: 0.86rem;
  }
  .field-row input[type="number"],
  .field-row input[type="text"],
  .field-row select {
    width: 200px;
    background: #0d0e10;
    border: 1px solid #26282c;
    color: #e8e9ec;
    padding: 0.42rem 0.6rem;
    border-radius: 5px;
    font-size: 0.85rem;
  }
  .field-row input:focus,
  .field-row select:focus {
    outline: none;
    border-color: #5b9bf5;
    background: #131517;
  }
  .field-help {
    margin: 0.7rem 0 0;
    font-size: 0.78rem;
    color: #8a8e95;
    line-height: 1.55;
  }
  .field-help strong { color: #c8cbd1; font-weight: 600; }
  .field-help code {
    background: #0d0e10;
    border: 1px solid #1d1f22;
    padding: 0.05rem 0.35rem;
    border-radius: 3px;
    font-size: 0.78rem;
    color: #b8bbc1;
  }
  .warn { color: #ec9006; }

  /* ---------- Regions list ---------- */
  .region-list {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  .region-item {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    padding: 0.5rem 0.7rem;
    background: #0d0e10;
    border: 1px solid #1d1f22;
    border-radius: 6px;
    font-size: 0.86rem;
  }
  .region-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .region-name { font-weight: 500; flex: 1; }
  .region-meta {
    color: #8a8e95;
    font-size: 0.78rem;
    font-family: ui-monospace, monospace;
  }
  .badge {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    font-weight: 500;
  }
  .badge-ok { background: #1c3826; color: #65c97e; }
  .badge-missing { background: #3a2424; color: #f0a0a0; }

  /* ---------- Hotkey button ---------- */
  .hotkey-btn {
    min-width: 130px;
    font-family: ui-monospace, monospace;
    text-align: center;
  }
  .hotkey-btn.capturing {
    background: #1f4ea0;
    border-color: #5b9bf5;
    color: #ffffff;
  }

  /* ---------- Color rows ---------- */
  .color-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.45rem 0;
  }
  .color-label {
    flex: 1;
    color: #b8bbc1;
    font-size: 0.86rem;
  }
  .color-swatch {
    width: 22px;
    height: 22px;
    border-radius: 4px;
    border: 1px solid #26282c;
    flex-shrink: 0;
  }
  .hex-input {
    width: 110px;
    background: #0d0e10;
    border: 1px solid #26282c;
    color: #e8e9ec;
    padding: 0.4rem 0.55rem;
    border-radius: 5px;
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
  }
  .tol-input {
    width: 60px;
    background: #0d0e10;
    border: 1px solid #26282c;
    color: #e8e9ec;
    padding: 0.4rem 0.55rem;
    border-radius: 5px;
    font-size: 0.82rem;
  }
  .tol-label { color: #6c7077; font-size: 0.75rem; }

  /* ---------- Calibration results ---------- */
  .cal-status-text { color: #b8bbc1; font-size: 0.82rem; }
  .cal-results-wrap {
    margin-top: 0.85rem;
    border-top: 1px solid #1d1f22;
    padding-top: 0.85rem;
  }
  .cal-results-wrap > summary {
    cursor: pointer;
    font-size: 0.85rem;
    color: #b8bbc1;
    list-style: none;
  }
  .cal-results-wrap > summary::before {
    content: "▸ ";
    color: #6c7077;
    font-size: 0.7rem;
  }
  .cal-results-wrap[open] > summary::before { content: "▾ "; }
  .cal-actions {
    display: flex;
    gap: 0.4rem;
    margin: 0.7rem 0 0.85rem;
    flex-wrap: wrap;
  }
  .cal-suggested {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin-top: 0.5rem;
    background: #0d0e10;
    border: 1px solid #1d1f22;
    border-radius: 6px;
    padding: 0.85rem 1rem;
  }
  .cal-suggested-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
  }
  .cal-role { color: #8a8e95; min-width: 90px; }
  .cal-suggested-row code {
    background: #16181b;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    font-size: 0.78rem;
  }
  .cal-note { color: #6c7077; font-size: 0.72rem; }
  .cal-none { color: #6c7077; font-size: 0.78rem; font-style: italic; }
  .cal-dominants-wrap { margin-top: 0.85rem; }
  .cal-dominants-wrap > summary {
    cursor: pointer;
    font-size: 0.78rem;
    color: #8a8e95;
    list-style: none;
  }
  .cal-dominants {
    margin-top: 0.6rem;
    max-height: 320px;
    overflow-y: auto;
    background: #0d0e10;
    border: 1px solid #1d1f22;
    border-radius: 6px;
    padding: 0.5rem;
  }
  .cal-dom-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.4rem;
    font-size: 0.78rem;
    border-bottom: 1px solid #16181b;
  }
  .cal-dom-row:last-child { border-bottom: none; }
  .cal-hex { font-family: ui-monospace, monospace; }
  .cal-class { color: #8a8e95; min-width: 70px; }
  .cal-count { color: #6c7077; font-size: 0.72rem; }
  .cal-assign { display: flex; gap: 0.25rem; margin-left: auto; }

  /* ---------- Region tools ---------- */
  .tools-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.5rem 0;
    flex-wrap: wrap;
  }
  .test-out {
    margin: 0 0 0.5rem;
    padding: 0.5rem 0.7rem;
    background: #0d0e10;
    border: 1px solid #1d1f22;
    border-radius: 5px;
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    color: #8a8e95;
    white-space: pre-wrap;
  }

  /* ---------- Window list ---------- */
  .window-list {
    margin-top: 0.6rem;
    max-height: 240px;
    overflow-y: auto;
    background: #0d0e10;
    border: 1px solid #1d1f22;
    border-radius: 6px;
  }
  .window-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #16181b;
    font-size: 0.78rem;
  }
  .window-row:last-child { border-bottom: none; }
  .window-row .app-name { color: #8eb4f0; }
  .window-row .title { color: #8a8e95; flex: 1; }
  .window-row .dims { color: #6c7077; font-size: 0.72rem; }

  /* ---------- Status bar (footer) ---------- */
  .status-bar {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    background: #0a0b0d;
    border-top: 1px solid #1d1f22;
    padding: 0.6rem 1.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    z-index: 10;
  }
  .debug-line {
    font-family: ui-monospace, monospace;
    font-size: 0.72rem;
    color: #6c7077;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .err {
    color: #f5a3a3;
    font-size: 0.78rem;
  }
  .status-text {
    font-size: 0.78rem;
    color: #b8bbc1;
  }
  .meta { color: #6c7077; font-size: 0.78rem; margin: 0.6rem 0 0; }
</style>
