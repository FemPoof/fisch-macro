<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { Window } from "@tauri-apps/api/window";
  import { invoke } from "@tauri-apps/api/core";
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

  const OVERLAY_HOTKEY = "F1";
  const STOP_HOTKEY = "F3";
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

  let cfg = $state<RegionsConfig | null>(null);
  let status = $state(`Ready. Press ${OVERLAY_HOTKEY} to edit regions.`);
  let hotkeyOk = $state(false);

  let running = $state(false);
  let dryRun = $state<boolean>(loadSetting("dryRun", false));
  let castDurationMs = $state<number>(loadSetting("castDurationMs", 1500));
  // Minimum gap between ticks. Each tick does a screen capture + template
  // search which can take ~50-100ms with the IoU matcher. Lower = more
  // responsive but more CPU; higher = less CPU but slower SHAKE reaction.
  let tickGapMs = $state<number>(loadSetting("tickGapMs", 200));
  // Score threshold for a SHAKE match. With the IoU matcher: score ≤
  // max_avg_diff * 10 fires. Real button matches typically score 100-300.
  let shakeMaxAvgDiff = $state<number>(loadSetting("shakeMaxAvgDiff", 40));

  // Persist whenever any setting changes.
  $effect(() => { localStorage.setItem("fm:dryRun", JSON.stringify(dryRun)); });
  $effect(() => { localStorage.setItem("fm:castDurationMs", JSON.stringify(castDurationMs)); });
  $effect(() => { localStorage.setItem("fm:tickGapMs", JSON.stringify(tickGapMs)); });
  $effect(() => { localStorage.setItem("fm:shakeMaxAvgDiff", JSON.stringify(shakeMaxAvgDiff)); });
  let templateCaptured = $state(false);
  let m1Held = false;
  let lastShakeAt = 0;
  let nextTickTimer: number | null = null;
  let lastDebug = $state("");
  let lastError = $state("");

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

  async function tickOnce() {
    if (!cfg) return;
    const sh = cfg.shake;
    const fb = cfg.fish_bar;
    if (!sh && !fb) return;

    // The macro window stays visible in both dry and real runs so the user
    // can watch debug output update live. Always exclude its current bounds
    // from detection so it doesn't poison the search region.
    const mainRect = await getMainWindowExclude();
    let result: TickResult;
    try {
      result = await invoke<TickResult>("tick_macro", {
        shake: sh ?? null,
        fishBar: fb ?? null,
        maxAvgDiff: shakeMaxAvgDiff,
        extraExcludes: mainRect ? [mainRect] : null,
      });
    } catch (e) {
      logErr("tick_macro", e);
      return;
    }

    const dryTag = dryRun ? "[DRY] " : "";
    const score = result.shake_score;
    const thr = result.shake_threshold;
    const shakeStats = result.shake_has_template
      ? `score ${score} (≤${thr} = match)`
      : `no template captured`;

    // Track best (lowest) score observed during the run.
    if (
      result.shake_has_template &&
      result.shake_click &&
      (bestSeenScore == null || score < bestSeenScore)
    ) {
      bestSeenScore = score;
      bestSeenAt = result.shake_click;
    }

    // First successful template match in a dry run dumps a snapshot.
    if (
      dryRun &&
      !autoSnapshotTaken &&
      result.shake_click &&
      score <= thr
    ) {
      autoSnapshotTaken = true;
      saveSnapshot().catch((e) => logErr("auto-snapshot", e));
    }

    // Fire when template matches.
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
        // Move cursor back into the reel area so subsequent M1 hold/release
        // events land where they need to. Otherwise the cursor stays at the
        // SHAKE position and reel control fires from up there.
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
      lastDebug = `${dryTag}SHAKE @ (${cx},${cy}) | ${shakeStats} | cap ${result.capture_ms}ms`;
      return;
    }

    if (result.player_x != null && result.target_x != null) {
      const px = result.player_x;
      const tx = result.target_x;
      const wantHold = px < tx;
      if (!dryRun) await setM1(wantHold);
      const verb = wantHold ? "HOLD" : "RELEASE";
      const cmp = wantHold ? "<" : "≥";
      const fishLabel = result.fish_x != null ? `fish ${result.fish_x}` : `target ${tx} (no fish — using bar center)`;
      lastDebug = `${dryTag}player ${px} ${cmp} ${fishLabel} → ${verb} | ${shakeStats} | cap ${result.capture_ms}ms`;
    } else if (fb) {
      lastDebug = `${dryTag}no player marker | ${shakeStats} | cap ${result.capture_ms}ms`;
    } else {
      lastDebug = `${dryTag}${shakeStats} | cap ${result.capture_ms}ms`;
    }
  }

  function scheduleNext() {
    if (!running) return;
    nextTickTimer = window.setTimeout(async () => {
      if (!running) return;
      await tickOnce();
      scheduleNext();
    }, tickGapMs);
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

    // The macro window stays visible in both dry and real runs so the
    // debug line updates where you can see it. tickOnce auto-excludes the
    // window from detection so it doesn't poison the scan. Move it off the
    // game viewport (a corner) before pressing Start.

    if (dryRun) {
      status = `Dry run — detecting only, no clicks. ${STOP_HOTKEY} stops.`;
      lastDebug = "dry run started";
      scheduleNext();
      return;
    }

    // Countdown so the user can switch focus to the Roblox window.
    for (let i = COUNTDOWN_S; i > 0; i--) {
      if (!running) {
        status = "Stopped.";
        return;
      }
      status = `Starting in ${i}s — focus the Roblox window now (${STOP_HOTKEY} aborts).`;
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
      try {
        await invoke("mouse_move", { x: castCx, y: castCy });
      } catch (e) {
        logErr("mouse_move(pre-cast)", e);
      }
      await sleep(80);
    }

    // Cast phase: hold M1 for the configured duration. Re-warp the cursor
    // periodically so it doesn't drift away mid-hold (macOS will let the
    // hardware mouse re-take the cursor otherwise).
    status = `Casting (holding M1 for ${castDurationMs}ms). ${STOP_HOTKEY} aborts.`;
    lastDebug = "casting";
    await setM1(true);
    const start = Date.now();
    while (running && Date.now() - start < castDurationMs) {
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
      return;
    }

    // Reel phase
    status = `Reeling. ${STOP_HOTKEY} stops.`;
    scheduleNext();
  }

  async function stopMacro() {
    running = false;
    if (nextTickTimer != null) {
      clearTimeout(nextTickTimer);
      nextTickTimer = null;
    }
    await setM1(false);
    const main = await Window.getByLabel("main");
    if (main) {
      await main.show();
      await main.setFocus();
    }
    status = "Stopped.";
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

  onMount(async () => {
    await refresh();
    try {
      for (const hk of [OVERLAY_HOTKEY, STOP_HOTKEY]) {
        if (await isRegistered(hk)) await unregister(hk);
      }
      await register(OVERLAY_HOTKEY, async (event) => {
        if (event.state === "Pressed") await toggleOverlay();
      });
      await register(STOP_HOTKEY, async (event) => {
        if (event.state === "Pressed") {
          if (running) await stopMacro();
        }
      });
      hotkeyOk = true;
    } catch (e) {
      status = `Could not register hotkeys: ${e}`;
    }
    // Don't await — let the update check run in the background so it doesn't
    // block first paint.
    checkForUpdate();
  });

  onDestroy(async () => {
    if (running) await stopMacro();
    for (const hk of [OVERLAY_HOTKEY, STOP_HOTKEY]) {
      try {
        await unregister(hk);
      } catch {}
    }
  });
</script>

<main>
  <h1>Fisch Macro</h1>
  <p class="hint">
    <kbd>{OVERLAY_HOTKEY}</kbd> opens the region overlay. <kbd>{STOP_HOTKEY}</kbd> stops the macro.
    {#if !hotkeyOk}<span class="warn"> (hotkeys unavailable)</span>{/if}
  </p>

  {#if updateAvailable}
    <div class="update-banner">
      <strong>Update available — v{updateAvailable.version}</strong>
      {#if updateAvailable.notes}<div class="update-notes">{updateAvailable.notes}</div>{/if}
      {#if updateError}<div class="update-error">{updateError}</div>{/if}
      <div class="update-actions">
        <button class="primary" onclick={installUpdate} disabled={updateInstalling}>
          {updateInstalling ? "Installing…" : "Install & restart"}
        </button>
        <button onclick={() => (updateAvailable = null)} disabled={updateInstalling}>Later</button>
      </div>
    </div>
  {/if}

  <section>
    <h2>Regions</h2>
    {#if cfg}
      {#each KEYS as key}
        {@const r = cfg[key] as Region | null}
        <div class="row">
          <span class="dot" style:background={REGION_META[key].color}></span>
          <span class="name">{REGION_META[key].label}</span>
          {#if r}
            <span class="ok">✓ {r.width}×{r.height} @ ({r.x},{r.y})</span>
            <button onclick={() => testRegion(key)}>Test</button>
            <button onclick={() => moveCursorToCenter(key)}>Move cursor</button>
            <button onclick={() => clickRegionCenter(key)}>Click here</button>
            {#if key === "shake"}
              <button onclick={showShakeDetection}>Show detect</button>
              <button onclick={saveSnapshot}>Save snapshot</button>
            {/if}
            {#if key === "shake_template"}
              <button onclick={captureTemplate}>
                {templateCaptured ? "Recapture template" : "Capture template"}
              </button>
              {#if templateCaptured}
                <button onclick={saveTemplateImage}>Save template PNG</button>
                <button onclick={clearTemplate}>Clear template</button>
              {/if}
              <button onclick={dumpCgFull}>Debug: full CG capture</button>
            {/if}
          {:else}
            <span class="missing">not set</span>
          {/if}
        </div>
        {#if testResults[key]}
          <div class="test-out">{testResults[key]}</div>
        {/if}
      {/each}
      {#if cfg.screen_width}
        <p class="meta">Saved at {cfg.screen_width}×{cfg.screen_height}.</p>
      {/if}
    {:else}
      <p>Loading…</p>
    {/if}
    <button onclick={toggleOverlay}>Open overlay</button>
  </section>

  <section>
    <h2>Macro</h2>
    <p class="hint">
      Start = {COUNTDOWN_S}s countdown → cast (hold M1) → reel loop.
      <kbd>{STOP_HOTKEY}</kbd> aborts at any point.
    </p>
    <div class="row">
      <label class="num-label">
        Cast hold (ms)
        <input type="number" bind:value={castDurationMs} min="100" step="100" disabled={running} />
      </label>
      <label class="num-label">
        Tick gap (ms) — bigger = less CPU
        <input type="number" bind:value={tickGapMs} min="0" step="50" />
      </label>
      <label class="num-label">
        Template match strictness
        <input type="number" bind:value={shakeMaxAvgDiff} min="0" max="255" step="5" />
      </label>
    </div>
    <p class="hint">
      Detection: <strong>template matching</strong>. Capture the SHAKE button
      as a template once (Regions section) and the macro looks for that exact
      image inside the Shake region every tick. Lower "max avg diff" =
      stricter match. {templateCaptured ? "Template captured ✓" : "No template captured."}
    </p>
    <div class="row">
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={dryRun} disabled={running} />
        Dry run (observe only, no clicks/holds)
      </label>
    </div>
    <div class="row">
      <button onclick={castOnce} disabled={running}>Cast only</button>
      {#if running}
        <button class="danger" onclick={stopMacro}>Stop ({STOP_HOTKEY})</button>
      {:else}
        <button class="primary" onclick={startMacro}>
          {dryRun ? "Start (dry run)" : "Start (cast + reel)"}
        </button>
      {/if}
    </div>
    <div class="debug">{lastDebug || "—"}</div>
    {#if bestSeenScore != null}
      <div class="hint">
        Best match this run — score <strong>{bestSeenScore}</strong>
        {#if bestSeenAt}@ ({bestSeenAt[0]}, {bestSeenAt[1]}){/if}
        {bestSeenScore <= shakeMaxAvgDiff * 1000 ? "(would fire)" : ""}
      </div>
    {/if}
    {#if lastError}
      <div class="err">⚠ {lastError}</div>
    {/if}
  </section>

  <p class="status">{status}</p>
</main>

<style>
  :global(body) {
    background: #1a1a1a;
    color: #eee;
    font-family: system-ui, -apple-system, sans-serif;
  }
  main {
    max-width: 640px;
    margin: 0 auto;
    padding: 2rem;
  }
  h1 { margin-bottom: 0.25rem; }
  h2 { margin-top: 0; font-size: 1.1rem; }
  section {
    margin: 1.5rem 0;
    padding: 1rem 1.25rem;
    background: #262626;
    border-radius: 10px;
  }
  .update-banner {
    margin: 1rem 0;
    padding: 0.85rem 1rem;
    background: #1a3a52;
    border: 1px solid #3b82f6;
    border-radius: 8px;
    font-size: 0.9rem;
  }
  .update-notes {
    margin-top: 0.4rem;
    color: #aaa;
    font-size: 0.85rem;
    white-space: pre-wrap;
  }
  .update-error {
    margin-top: 0.4rem;
    color: #fca5a5;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
  }
  .update-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.6rem;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0;
    flex-wrap: wrap;
  }
  .dot { width: 12px; height: 12px; border-radius: 50%; }
  .name { flex: 1; }
  .ok {
    color: #4ade80;
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
  }
  .missing { color: #888; font-style: italic; }
  .meta { color: #888; font-size: 0.8rem; margin: 0.5rem 0 0; }
  .test-out {
    margin-left: 1.7rem;
    margin-bottom: 0.4rem;
    color: #aaa;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
  }
  .num-label {
    display: flex;
    flex-direction: column;
    font-size: 0.75rem;
    color: #aaa;
    gap: 0.2rem;
  }
  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: #ccc;
    font-size: 0.9rem;
    cursor: pointer;
  }
  .checkbox-label input {
    width: auto;
  }
  .num-label input {
    width: 110px;
    padding: 0.3rem 0.4rem;
    background: #1a1a1a;
    color: #eee;
    border: 1px solid #444;
    border-radius: 4px;
    font: inherit;
  }
  button {
    margin-top: 0.5rem;
    padding: 0.5rem 1rem;
    background: #3b82f6;
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 0.95rem;
    cursor: pointer;
  }
  button:hover { background: #2563eb; }
  button:disabled { background: #555; cursor: not-allowed; }
  button.primary { background: #16a34a; }
  button.primary:hover { background: #15803d; }
  button.danger { background: #dc2626; }
  button.danger:hover { background: #b91c1c; }
  .hint { color: #aaa; font-size: 0.9rem; }
  .warn { color: #f87171; }
  kbd {
    background: #333;
    border: 1px solid #555;
    border-radius: 4px;
    padding: 1px 6px;
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
  }
  .status { color: #888; font-style: italic; font-size: 0.85rem; }
  .debug {
    margin-top: 0.6rem;
    padding: 0.5rem 0.7rem;
    background: #1a1a1a;
    border-radius: 6px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    color: #ccc;
    white-space: pre-wrap;
  }
  .err {
    margin-top: 0.4rem;
    padding: 0.5rem 0.7rem;
    background: #2a1414;
    color: #fca5a5;
    border-radius: 6px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    white-space: pre-wrap;
  }
</style>
