<script lang="ts">
  import { onMount } from "svelte";
  import { Window, getCurrentWindow } from "@tauri-apps/api/window";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
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

  // Frozen-screen background. When the overlay opens we capture a snapshot
  // of the desktop and render it as a static <img> behind the region UI.
  // Solves the "GUIs disappear too fast / I have to tab around" problem
  // the user described — the picker always shows the screen state at the
  // moment F1 was pressed, even if underlying apps re-render.
  let freezeUrl = $state<string | null>(null);
  let freezeImg = $state<HTMLImageElement | null>(null);
  // Magnifier state — follows cursor, samples the freeze image at higher
  // zoom for pixel-precise region picking. Always on whenever a freeze
  // exists; user explicitly asked to remove the toggle.
  let magX = $state(0);
  let magY = $state(0);
  let magCanvas: HTMLCanvasElement | null = $state(null);
  // Magnifier display size + zoom factor. 8× zoom on a 180px window means
  // each rendered "pixel" is 8 actual pixels — same magnification class
  // as Hydra's example screenshot.
  const MAG_SIZE = 180;
  const MAG_ZOOM = 8;
  // Hex sampled under the cursor — shown as a label on the magnifier so
  // the user can read the exact pixel color for manual hex entry without
  // having to save a PNG and use a separate eyedropper.
  let magHex = $state<string>("");

  // While this overlay is open, region coordinates are stored as
  // OVERLAY-RELATIVE PHYSICAL pixels (so display = stored * scale, no
  // offsets needed). Persistent storage on disk uses SCREEN-ABSOLUTE
  // PHYSICAL coords; we convert in/out via the overlay's outer position
  // on load/save/capture.

  onMount(() => {
    // Re-capture every time the overlay window is shown again. Without
    // this, the overlay window — which is created hidden at app launch
    // and reused on every F1 press — keeps reusing the launch-time
    // screenshot. That produced the "freeze is from a minute ago and
    // doesn't line up" report from the user.
    const unlistenP = listen("overlay-shown", async () => {
      await refreshFreeze();
    });

    (async () => {
      await refreshScale();
      await refreshDebug();
      window.addEventListener("resize", async () => {
        await refreshScale();
        await refreshDebug();
      });
      // Capture the freeze BEFORE drawing existing regions so the picker
      // appears with the static snapshot immediately on first show.
      await refreshFreeze();

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
    })();

    return () => {
      unlistenP.then((u) => u()).catch(() => {});
    };
  });

  async function refreshFreeze() {
    try {
      // Use a base64 data URL — sidesteps Tauri 2's asset-protocol
      // requirement (which would otherwise block file:// loads from the
      // webview). Data URLs also work cleanly inside drawImage.
      const dataUrl = await invoke<string>("capture_full_screen_data_url");
      // Pre-load into an <Image> so the magnifier canvas can draw it
      // synchronously on every cursor move. Without this we'd have to
      // wait for the browser to load it on first draw, which causes the
      // "magnifier is empty" symptom from the user's report.
      const img = new Image();
      await new Promise<void>((resolve, reject) => {
        img.onload = () => resolve();
        img.onerror = (e) => reject(e);
        img.src = dataUrl;
      });
      freezeImg = img;
      freezeUrl = dataUrl;
    } catch (e) {
      console.warn("freeze capture failed", e);
      freezeUrl = null;
      freezeImg = null;
    }
  }

  // Canvas-based magnifier. Drawing the zoomed pixels via drawImage with
  // imageSmoothingEnabled=false is far more reliable than CSS background
  // tricks (which silently fail on URL quoting / size calc edge cases —
  // the symptom from the user's last test). Recomputed on every cursor
  // move via $effect.
  function paintMagnifier() {
    if (!magCanvas || !freezeImg) return;
    const ctx = magCanvas.getContext("2d");
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;
    // Source rect: a small window centered on the cursor's pixel,
    // sized so that when scaled to MAG_SIZE we get MAG_ZOOM× zoom.
    const halfSrc = MAG_SIZE / (2 * MAG_ZOOM);
    // Convert cursor CSS pixels → freeze-image pixels. The freeze image
    // is at logical screen size, the overlay viewport is also at logical
    // screen size (window = full screen), so the conversion is identity.
    const srcX = magX - halfSrc;
    const srcY = magY - halfSrc;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, MAG_SIZE, MAG_SIZE);
    ctx.drawImage(
      freezeImg,
      srcX,
      srcY,
      halfSrc * 2,
      halfSrc * 2,
      0,
      0,
      MAG_SIZE,
      MAG_SIZE
    );
    // Sample the pixel under the cursor for the hex label. Use a tiny
    // 1×1 readback at the center of the canvas (= cursor pixel after
    // the zoom).
    try {
      const px = ctx.getImageData(MAG_SIZE / 2, MAG_SIZE / 2, 1, 1).data;
      magHex = `#${px[0].toString(16).padStart(2, "0")}${px[1]
        .toString(16)
        .padStart(2, "0")}${px[2].toString(16).padStart(2, "0")}`;
    } catch {
      magHex = "";
    }
  }

  $effect(() => {
    // Re-paint whenever the cursor moves, the freeze loads, or the canvas
    // becomes available (canvas is rendered conditionally inside
    // {#if freezeImg}, so its `bind:this` fires *after* freezeImg flips
    // from null to an image — a previous bug where the first paint
    // missed because the canvas wasn't bound yet).
    magX;
    magY;
    freezeImg;
    magCanvas;
    paintMagnifier();
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
    // Update magnifier position on every move (cheap — just two reactive
    // assignments), regardless of whether we're dragging a region.
    magX = e.clientX;
    magY = e.clientY;
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

  // Manual hex assignment from the magnifier. The user hovers their
  // cursor over a target pixel (e.g., the fish indicator line they want
  // to track) and clicks one of these buttons — that pixel's hex gets
  // written directly to the corresponding fish-color setting in main
  // localStorage. Bypasses auto-cal entirely. Solves the long-running
  // "auto-cal keeps picking the wrong target color" problem since auto-
  // cal can't tell the fish indicator from track end-cap markers.
  let pickStatus = $state<string>("");
  function setMagAsFishColor(role: "target" | "arrow" | "left" | "right") {
    if (!magHex || magHex.length < 7) {
      pickStatus = "Move cursor over a pixel first.";
      return;
    }
    const hex = magHex.replace(/^#/, "").toLowerCase();
    // Sanity-check the role/hex combo. The Left/Right Bar are the white
    // player-bar gradient; if the user accidentally hovers over the
    // divider (dark navy ~#171734) and clicks "Set Left Bar", their
    // bar detection breaks completely (this is the bug from log
    // 1777257958 — both bar colors set to #171734, tot[L=0 R=0] forever).
    const r = parseInt(hex.slice(0, 2), 16);
    const g = parseInt(hex.slice(2, 4), 16);
    const b = parseInt(hex.slice(4, 6), 16);
    const minCh = Math.min(r, g, b);
    if ((role === "left" || role === "right") && minCh < 150) {
      pickStatus =
        `Refused: #${hex} is too dark for the player bar (which should be near-white). ` +
        `Hover over the WHITE part of the bar, not the dark divider.`;
      return;
    }
    if (role === "arrow" && (minCh > 200 || minCh < 30)) {
      pickStatus =
        `Refused: #${hex} doesn't look like the arrow icon (mid-grey expected).`;
      return;
    }
    const keyMap: Record<string, string> = {
      target: "fishTargetHex",
      arrow: "fishArrowHex",
      left: "fishLeftHex",
      right: "fishRightHex",
    };
    const k = keyMap[role];
    try {
      // Write to BOTH the live setting AND the saved-default snapshot so
      // that Reset will return to this color and a future auto-cal Apply
      // won't immediately overwrite it.
      localStorage.setItem(`fm:${k}`, JSON.stringify(hex));
      localStorage.setItem(`fm:default:${k}`, JSON.stringify(hex));
      // Also widen the matching tol a bit so anti-aliased neighbors of
      // this pixel still match — manually-picked single pixels are often
      // the "ideal" color but actual rendered pixels vary by ±5-10.
      const tolKey = `fm:${k.replace("Hex", "Tol")}`;
      localStorage.setItem(tolKey, JSON.stringify(8));
      pickStatus = `Set ${role} = #${hex} (tol 8). Close overlay & test.`;
    } catch (e) {
      pickStatus = `error: ${e}`;
    }
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
    if (e.key === "F1") {
      e.preventDefault();
      refreshFreeze();
    }
  }

  // Position the magnifier to the side of the cursor so it doesn't hide
  // what we're aiming at. Right side normally; flip to left if we're near
  // the right edge. Vertical: clamp to the viewport.
  function magPos(): { left: number; top: number } {
    let left = magX + 24;
    if (typeof window !== "undefined") {
      if (left + MAG_SIZE > window.innerWidth - 12)
        left = magX - MAG_SIZE - 24;
    }
    let top = magY - MAG_SIZE / 2;
    if (top < 12) top = 12;
    if (typeof window !== "undefined") {
      if (top + MAG_SIZE > window.innerHeight - 12)
        top = window.innerHeight - MAG_SIZE - 12;
    }
    return { left, top };
  }
</script>

<svelte:window on:pointermove={onPointerMove} on:pointerup={endDrag} on:keydown={onKey} />

<div class="overlay" onpointerdown={() => (selected = null)}>
  <!--
    Freeze image is intentionally NOT rendered. It used to be drawn as a
    full-screen <img> behind the region UI so the user could click on a
    static snapshot. But on macOS the overlay window covers the work area
    only (not the menu bar), while the screenshot is full-screen — when
    the image stretches into the smaller window the menu bar pixels get
    squished into the top, producing a ghosted "double menu bar" right
    below the live one. Symptom from logs/screenshots before this fix.

    Behavior change: user now sees the LIVE game underneath while
    drawing region rectangles. Region picking still works (clicks on
    rectangles, dragging edges) — they were always HTML overlays, not
    drawn into the freeze. Pixel-precise selection on a moving game is
    slightly less precise but most regions are big enough that this
    doesn't matter. The freeze IMAGE is still loaded into memory
    (freezeImg) so the magnifier canvas can sample from it for color
    picking — that part is unaffected by removing the on-screen <img>.
  -->

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

  {#if freezeImg}
    {@const mp = magPos()}
    <div
      class="magnifier-wrap"
      style:left="{mp.left}px"
      style:top="{mp.top}px"
    >
      <canvas
        bind:this={magCanvas}
        class="magnifier"
        width={MAG_SIZE}
        height={MAG_SIZE}
      ></canvas>
      <div class="mag-crosshair-h"></div>
      <div class="mag-crosshair-v"></div>
      {#if magHex}
        <div class="mag-hex">{magHex}</div>
      {/if}
    </div>
  {/if}

  <div class="toolbar" onpointerdown={(e) => e.stopPropagation()}>
    <div class="title">Region picker</div>
    <div class="hint">F1 = re-snapshot · Enter = save · Esc = cancel</div>
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

    <div class="picker-panel">
      <div class="picker-title">Color picker</div>
      <div class="picker-hex">
        Cursor pixel: <code>{magHex || "—"}</code>
      </div>
      <div class="picker-row">
        <button class="picker-btn" onclick={() => setMagAsFishColor("target")}>Set Target Line</button>
        <button class="picker-btn" onclick={() => setMagAsFishColor("arrow")}>Set Arrow</button>
      </div>
      <div class="picker-row">
        <button class="picker-btn" onclick={() => setMagAsFishColor("left")}>Set Left Bar</button>
        <button class="picker-btn" onclick={() => setMagAsFishColor("right")}>Set Right Bar</button>
      </div>
      {#if pickStatus}
        <div class="status-line">{pickStatus}</div>
      {/if}
      <div class="hint" style="margin-top:4px;">
        Most users only need <strong>Set Target Line</strong> — hover the
        fish indicator line and click it. Don't touch the Bar buttons unless
        the bar isn't being detected at all (rod has very dim bar).
      </div>
    </div>
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
    cursor: crosshair;
  }
  .freeze {
    position: fixed;
    inset: 0;
    width: 100vw;
    height: 100vh;
    object-fit: fill;
    pointer-events: none;
    user-select: none;
    z-index: 0;
    /* No filter — show the freeze at faithful colors. Earlier we applied
       brightness(0.85) to make region rectangles pop, but that darkened
       the pixels visually for the user, making calibration of regions
       harder (you're aligning to a dimmed scene that doesn't match the
       live game). The magnifier reads the original image data, not the
       darkened render, so its hex values are correct — but the user's
       perception of where the bar is in the freeze was off. */
  }
  .magnifier-wrap {
    position: fixed;
    pointer-events: none;
    z-index: 9999;
    border: 2px solid white;
    box-shadow: 0 0 0 1px black, 0 6px 18px rgba(0, 0, 0, 0.6);
    border-radius: 50%;
    overflow: hidden;
    width: 180px;
    height: 180px;
  }
  .magnifier {
    display: block;
    image-rendering: pixelated;
  }
  .mag-crosshair-h,
  .mag-crosshair-v {
    position: absolute;
    background: rgba(255, 0, 0, 0.9);
    pointer-events: none;
  }
  .mag-crosshair-h {
    left: 0;
    right: 0;
    top: 50%;
    height: 1px;
    transform: translateY(-0.5px);
  }
  .mag-crosshair-v {
    top: 0;
    bottom: 0;
    left: 50%;
    width: 1px;
    transform: translateX(-0.5px);
  }
  .mag-hex {
    position: absolute;
    bottom: 6px;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(0, 0, 0, 0.78);
    color: white;
    padding: 2px 8px;
    border-radius: 3px;
    font: 11px ui-monospace, "SF Mono", Menlo, monospace;
    pointer-events: none;
  }
  .region {
    z-index: 5;
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
    z-index: 10;
  }
  .hint {
    color: #888;
    font-size: 11px;
    margin-bottom: 6px;
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
  .picker-panel {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid #2a2a2a;
  }
  .picker-title {
    font-weight: 600;
    margin-bottom: 4px;
    font-size: 12px;
  }
  .picker-hex {
    margin-bottom: 6px;
    font-size: 12px;
    color: #ccc;
  }
  .picker-hex code {
    font-family: ui-monospace, "SF Mono", Menlo, monospace;
    color: #fff;
  }
  .picker-row {
    display: flex;
    gap: 4px;
    margin-bottom: 4px;
  }
  .picker-btn {
    flex: 1;
    padding: 4px 6px;
    font-size: 11px;
    background: #1f3a5f;
    border: 1px solid #2a4a72;
    color: white;
    border-radius: 4px;
    cursor: pointer;
  }
  .picker-btn:hover {
    background: #2a4a72;
  }
</style>
