<script lang="ts">
  import { onMount } from "svelte";
  import { Window, getCurrentWindow } from "@tauri-apps/api/window";
  import { invoke } from "@tauri-apps/api/core";
  import {
    loadRegions,
    saveRegions,
    getScreenSize,
    REGION_META,
    type Region,
    type RegionKey,
    type RegionsConfig,
  } from "$lib/regions";

  type Handle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

  const KEYS: RegionKey[] = ["shake", "fish_bar", "shake_template"];

  let regions = $state<Record<RegionKey, Region | null>>({
    shake: null,
    fish_bar: null,
    shake_template: null,
  });
  let selected = $state<RegionKey | null>(null);
  let scale = $state(1);
  let screenW = $state(0);
  let screenH = $state(0);

  let dragging = $state<{
    key: RegionKey;
    handle: Handle;
    startMouseX: number;
    startMouseY: number;
    startRegion: Region;
  } | null>(null);

  // While this overlay is open, region coordinates are stored as
  // OVERLAY-RELATIVE PHYSICAL pixels (so display = stored * scale, no
  // offsets needed). Persistent storage on disk uses SCREEN-ABSOLUTE
  // PHYSICAL coords; we convert in/out via the overlay's outer position
  // on load/save/capture.

  onMount(async () => {
    await refreshScale();
    await refreshDebug();
    window.addEventListener("resize", async () => {
      await refreshScale();
      await refreshDebug();
    });

    const cfg = await loadRegions();
    const offset = await currentOriginLogical();
    for (const k of KEYS) {
      const r = cfg[k];
      if (r) {
        regions[k] = {
          ...r,
          x: r.x - offset.x,
          y: r.y - offset.y,
        };
      }
    }
  });

  /// Overlay window's screen origin in LOGICAL pixels. Tauri's outerPosition
  /// returns PHYSICAL pixels, but our region coordinates are logical (xcap
  /// reports its monitor size in logical pixels and we standardize on that
  /// throughout the system), so we divide by devicePixelRatio.
  async function currentOriginLogical(): Promise<{ x: number; y: number }> {
    const pos = await getCurrentWindow().outerPosition();
    const dpr = window.devicePixelRatio || 1;
    return { x: pos.x / dpr, y: pos.y / dpr };
  }

  async function refreshScale() {
    const [w, h] = await getScreenSize();
    screenW = w;
    screenH = h;
    scale = window.innerWidth / w;
  }

  function toCss(r: Region) {
    return {
      left: r.x * scale,
      top: r.y * scale,
      width: r.width * scale,
      height: r.height * scale,
    };
  }

  function ensureRegion(key: RegionKey) {
    if (regions[key]) return regions[key]!;
    const meta = REGION_META[key];
    const r: Region = {
      x: Math.round((screenW - meta.defaultSize.w) / 2),
      y: Math.round((screenH - meta.defaultSize.h) / 2),
      width: meta.defaultSize.w,
      height: meta.defaultSize.h,
    };
    regions[key] = r;
    return r;
  }

  function startDrag(e: PointerEvent, key: RegionKey, handle: Handle) {
    e.stopPropagation();
    e.preventDefault();
    selected = key;
    const r = ensureRegion(key);
    dragging = {
      key,
      handle,
      startMouseX: e.clientX,
      startMouseY: e.clientY,
      startRegion: { ...r },
    };
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const dxCss = e.clientX - dragging.startMouseX;
    const dyCss = e.clientY - dragging.startMouseY;
    const dx = Math.round(dxCss / scale);
    const dy = Math.round(dyCss / scale);

    const s = dragging.startRegion;
    let { x, y, width, height } = s;

    const minSize = 10;

    switch (dragging.handle) {
      case "move":
        x = s.x + dx;
        y = s.y + dy;
        break;
      case "n":
        y = s.y + Math.min(dy, s.height - minSize);
        height = s.height - Math.min(dy, s.height - minSize);
        break;
      case "s":
        height = Math.max(minSize, s.height + dy);
        break;
      case "w":
        x = s.x + Math.min(dx, s.width - minSize);
        width = s.width - Math.min(dx, s.width - minSize);
        break;
      case "e":
        width = Math.max(minSize, s.width + dx);
        break;
      case "nw":
        x = s.x + Math.min(dx, s.width - minSize);
        width = s.width - Math.min(dx, s.width - minSize);
        y = s.y + Math.min(dy, s.height - minSize);
        height = s.height - Math.min(dy, s.height - minSize);
        break;
      case "ne":
        width = Math.max(minSize, s.width + dx);
        y = s.y + Math.min(dy, s.height - minSize);
        height = s.height - Math.min(dy, s.height - minSize);
        break;
      case "sw":
        x = s.x + Math.min(dx, s.width - minSize);
        width = s.width - Math.min(dx, s.width - minSize);
        height = Math.max(minSize, s.height + dy);
        break;
      case "se":
        width = Math.max(minSize, s.width + dx);
        height = Math.max(minSize, s.height + dy);
        break;
    }

    // Clamp to screen
    x = Math.max(0, Math.min(x, screenW - width));
    y = Math.max(0, Math.min(y, screenH - height));
    width = Math.min(width, screenW - x);
    height = Math.min(height, screenH - y);

    regions[dragging.key] = { x, y, width, height };
  }

  function endDrag() {
    dragging = null;
  }

  let templateStatus = $state("");
  let debugOriginX = $state(0);
  let debugOriginY = $state(0);
  let debugInnerW = $state(0);
  let debugInnerH = $state(0);
  let debugDpr = $state(1);

  async function refreshDebug() {
    const pos = await getCurrentWindow().outerPosition();
    debugOriginX = pos.x;
    debugOriginY = pos.y;
    debugInnerW = window.innerWidth;
    debugInnerH = window.innerHeight;
    debugDpr = window.devicePixelRatio || 1;
  }

  /// Capture the SHAKE template from the current Template region position.
  /// Hides the overlay + main window briefly so neither shows up in the
  /// captured pixels. Also saves diagnostic full-screen images with a red
  /// rect drawn at the exact capture coords so we can verify alignment.
  async function captureTemplateFromOverlay() {
    if (!regions.shake_template) {
      templateStatus = "Set the purple Template box around the SHAKE button first.";
      return;
    }
    const r = regions.shake_template;
    const overlayWin = getCurrentWindow();
    const mainWin = await Window.getByLabel("main");

    // IMPORTANT: read overlay origin while overlay is still visible — once
    // hidden the position can be stale.
    const rawPos = await getCurrentWindow().outerPosition();
    const dpr = window.devicePixelRatio || 1;
    const offset = { x: rawPos.x / dpr, y: rawPos.y / dpr };
    const screenX = r.x + offset.x;
    const screenY = r.y + offset.y;

    // Diagnostic frame #1: with overlay still visible. The red rect should
    // align exactly with the purple Template box. If it doesn't, our
    // overlay-relative → screen-absolute conversion is wrong.
    try {
      await invoke<string>("debug_save_full_with_marker", {
        x: screenX,
        y: screenY,
        width: r.width,
        height: r.height,
        suffix: "with_overlay",
      });
    } catch (e) {
      console.warn("marker with_overlay failed", e);
    }

    const mainWasVisible = mainWin ? await mainWin.isVisible() : false;

    await overlayWin.hide();
    if (mainWin && mainWasVisible) await mainWin.hide();
    // Short sleep — long enough for macOS to recompose the screen without
    // the overlay, short enough that the SHAKE prompt doesn't dismiss in
    // the gap. 60ms = ~4 frames at 60Hz.
    await new Promise((res) => setTimeout(res, 60));

    // Diagnostic frame #2: overlay hidden — the red rect should land on the
    // SHAKE button itself.
    try {
      await invoke<string>("debug_save_full_with_marker", {
        x: screenX,
        y: screenY,
        width: r.width,
        height: r.height,
        suffix: "no_overlay",
      });
    } catch (e) {
      console.warn("marker no_overlay failed", e);
    }

    let ok = false;
    let err = "";
    try {
      await invoke<number>("capture_shake_template", {
        x: screenX,
        y: screenY,
        width: r.width,
        height: r.height,
      });
      ok = true;
    } catch (e) {
      err = String(e);
    }

    await overlayWin.show();
    await overlayWin.setFocus();
    if (mainWin && mainWasVisible) await mainWin.show();

    const diag = `outer=(${rawPos.x},${rawPos.y})phys dpr=${dpr} → offset=(${offset.x},${offset.y})log | box=(${r.x},${r.y}) ${r.width}×${r.height} → screen=(${screenX},${screenY})`;
    templateStatus = ok
      ? `Template captured. ${diag}. See Desktop/fisch-macro-debug/template_marker_*.png — red rect should align with purple box (with_overlay) and SHAKE button (no_overlay).`
      : `Capture failed: ${err} | ${diag}`;
  }

  async function saveAndClose() {
    // Convert overlay-relative coords → screen-absolute for persistence.
    const offset = await currentOriginLogical();
    const toAbs = (r: Region | null): Region | null =>
      r ? { ...r, x: r.x + offset.x, y: r.y + offset.y } : null;

    const cfg: RegionsConfig = {
      screen_width: screenW,
      screen_height: screenH,
      shake: toAbs(regions.shake),
      fish_bar: toAbs(regions.fish_bar),
      shake_template: toAbs(regions.shake_template),
    };
    await saveRegions(cfg);
    await getCurrentWindow().hide();
  }

  function deleteRegion(key: RegionKey) {
    regions[key] = null;
    if (selected === key) selected = null;
  }

  async function cancel() {
    // reload from disk to discard unsaved edits
    const cfg = await loadRegions();
    for (const k of KEYS) regions[k] = cfg[k] ?? null;
    await getCurrentWindow().hide();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") cancel();
    if (e.key === "Enter") saveAndClose();
  }
</script>

<svelte:window on:pointermove={onPointerMove} on:pointerup={endDrag} on:keydown={onKey} />

<div class="overlay" onpointerdown={() => (selected = null)}>
  {#each KEYS as key}
    {@const r = regions[key]}
    {#if r}
      {@const css = toCss(r)}
      <div
        class="region"
        class:selected={selected === key}
        style:left="{css.left}px"
        style:top="{css.top}px"
        style:width="{css.width}px"
        style:height="{css.height}px"
        style:--color={REGION_META[key].color}
        onpointerdown={(e) => startDrag(e, key, "move")}
      >
        <div class="label">{REGION_META[key].label} — {r.width}×{r.height} @ ({r.x},{r.y})</div>
        {#each ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as h (h)}
          <div
            class="handle h-{h}"
            onpointerdown={(e) => startDrag(e, key, h as Handle)}
          ></div>
        {/each}
      </div>
    {/if}
  {/each}

  <div class="toolbar" onpointerdown={(e) => e.stopPropagation()}>
    <div class="title">Region picker</div>
    {#each KEYS as key}
      {@const r = regions[key]}
      <div class="row">
        <span class="dot" style:background={REGION_META[key].color}></span>
        <span class="name">{REGION_META[key].label}</span>
        {#if r}
          <button onclick={() => (selected = key)}>Select</button>
          <button onclick={() => deleteRegion(key)}>Clear</button>
        {:else}
          <button onclick={() => { ensureRegion(key); selected = key; }}>Add</button>
        {/if}
      </div>
    {/each}
    <div class="actions">
      <button class="primary" onclick={saveAndClose}>Save & close (Enter)</button>
      <button onclick={cancel}>Cancel (Esc)</button>
    </div>
    {#if regions.shake_template}
      <div class="actions">
        <button onclick={captureTemplateFromOverlay}>Capture template now</button>
      </div>
    {/if}
    {#if templateStatus}
      <div class="status-line">{templateStatus}</div>
    {/if}
    <div class="hint">Drag inside a region to move; corners/edges resize.</div>
  </div>
</div>

<style>
  :global(html, body) {
    margin: 0;
    padding: 0;
    background: transparent !important;
    overflow: hidden;
    width: 100vw;
    height: 100vh;
    user-select: none;
  }
  .overlay {
    position: fixed;
    inset: 0;
    background: transparent;
    cursor: default;
  }
  .region {
    position: absolute;
    border: 2px solid var(--color);
    background: color-mix(in srgb, var(--color) 12%, transparent);
    box-sizing: border-box;
    cursor: move;
  }
  .region.selected {
    box-shadow: 0 0 0 2px white inset;
  }
  .label {
    position: absolute;
    top: -22px;
    left: 0;
    background: var(--color);
    color: white;
    font: 12px/1 system-ui, sans-serif;
    padding: 3px 6px;
    border-radius: 4px 4px 0 0;
    white-space: nowrap;
  }
  .handle {
    position: absolute;
    width: 12px;
    height: 12px;
    background: white;
    border: 1px solid var(--color);
    box-sizing: border-box;
  }
  .h-nw { left: -6px;  top: -6px;    cursor: nwse-resize; }
  .h-n  { left: 50%;   top: -6px;    transform: translateX(-50%); cursor: ns-resize; }
  .h-ne { right: -6px; top: -6px;    cursor: nesw-resize; }
  .h-e  { right: -6px; top: 50%;     transform: translateY(-50%); cursor: ew-resize; }
  .h-se { right: -6px; bottom: -6px; cursor: nwse-resize; }
  .h-s  { left: 50%;   bottom: -6px; transform: translateX(-50%); cursor: ns-resize; }
  .h-sw { left: -6px;  bottom: -6px; cursor: nesw-resize; }
  .h-w  { left: -6px;  top: 50%;     transform: translateY(-50%); cursor: ew-resize; }

  .toolbar {
    position: fixed;
    top: 16px;
    right: 16px;
    background: rgba(20, 20, 20, 0.92);
    color: #eee;
    padding: 12px 14px;
    border-radius: 10px;
    font: 13px system-ui, sans-serif;
    min-width: 260px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  }
  .title {
    font-weight: 600;
    margin-bottom: 8px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 4px 0;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    display: inline-block;
  }
  .name {
    flex: 1;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
  }
  button {
    background: #333;
    color: #eee;
    border: 1px solid #555;
    border-radius: 6px;
    padding: 4px 10px;
    cursor: pointer;
    font: inherit;
  }
  button:hover { background: #444; }
  button.primary { background: #3b82f6; border-color: #3b82f6; }
  button.primary:hover { background: #2563eb; }
  .hint {
    margin-top: 8px;
    color: #888;
    font-size: 11px;
  }
  .status-line {
    margin-top: 8px;
    color: #4ade80;
    font-size: 11px;
    font-family: ui-monospace, monospace;
  }
</style>
