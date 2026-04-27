<script lang="ts">
  // Always-on-top status overlay. The main macro window emits a `macro-status`
  // event on every tick (and on state transitions); we listen and render.
  // Designed to live in a screen corner while the user plays.

  import { listen } from "@tauri-apps/api/event";
  import { onMount, onDestroy } from "svelte";

  type Status = {
    running: boolean;
    stage: string; // "Idle" / "Casting" / "Reeling" / "SHAKE"
    mode: "HOLD" | "RELEASE" | "CLICK" | "—";
    err: number;
    player: number | null;
    target: number | null;
    cycles: number;
    capture_ms: number;
    keybind_start_stop: string;
    keybind_overlay: string;
  };

  let s = $state<Status>({
    running: false,
    stage: "Idle",
    mode: "—",
    err: 0,
    player: null,
    target: null,
    cycles: 0,
    capture_ms: 0,
    keybind_start_stop: "F3",
    keybind_overlay: "F1",
  });

  let elapsed = $state("0m 0s");
  let startedAt = 0;
  let elapsedTimer: number | null = null;

  function startElapsed() {
    if (elapsedTimer != null) return;
    startedAt = Date.now();
    elapsed = "0m 0s";
    elapsedTimer = window.setInterval(() => {
      const total = Math.floor((Date.now() - startedAt) / 1000);
      const m = Math.floor(total / 60);
      const sec = total % 60;
      elapsed = `${m}m ${sec.toString().padStart(2, "0")}s`;
    }, 1000);
  }

  function stopElapsed() {
    if (elapsedTimer != null) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
    }
    startedAt = 0;
  }

  let unlistenFn: (() => void) | null = null;

  onMount(async () => {
    unlistenFn = await listen<Status>("macro-status", (e) => {
      const next = e.payload;
      const wasRunning = s.running;
      s = next;
      if (next.running && !wasRunning) startElapsed();
      else if (!next.running && wasRunning) stopElapsed();
    });
  });

  onDestroy(() => {
    if (unlistenFn) unlistenFn();
    stopElapsed();
  });

  function modeColor(m: string) {
    if (m === "HOLD") return "#3b82f6";
    if (m === "RELEASE") return "#f59e0b";
    if (m === "CLICK") return "#34d399";
    return "#666";
  }
</script>

<div class="overlay" data-tauri-drag-region>
  <div class="header" data-tauri-drag-region>
    <span class="brand">FISCH MACRO</span>
    <span class="elapsed">{elapsed}</span>
  </div>

  <div class="row">
    <span class="dot" class:on={s.running}></span>
    <span class="stage">{s.stage}</span>
  </div>

  <div class="row">
    <span class="label">Mode</span>
    <span class="value" style:color={modeColor(s.mode)}>{s.mode}</span>
  </div>

  <div class="row">
    <span class="label">Err</span>
    <span class="value">{s.err}px</span>
  </div>

  <div class="row">
    <span class="label">Cycles</span>
    <span class="value">{s.cycles}</span>
  </div>

  <div class="row">
    <span class="label">Cap</span>
    <span class="value">{s.capture_ms}ms</span>
  </div>

  <div class="footer">
    {s.keybind_start_stop} start/stop · {s.keybind_overlay} regions
  </div>
</div>

<style>
  :global(html, body) {
    margin: 0;
    padding: 0;
    background: transparent;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    user-select: none;
    -webkit-user-select: none;
    overflow: hidden;
  }

  .overlay {
    background: rgba(15, 15, 15, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    padding: 0.55rem 0.7rem;
    color: #e5e5e5;
    font-size: 12px;
    -webkit-backdrop-filter: blur(8px);
    backdrop-filter: blur(8px);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.3);
  }

  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding-bottom: 0.35rem;
    margin-bottom: 0.45rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    cursor: move;
  }

  .brand {
    font-size: 0.65rem;
    font-weight: 700;
    letter-spacing: 0.1em;
    color: #34d399;
  }

  .elapsed {
    font-family: ui-monospace, "SF Mono", Monaco, monospace;
    font-size: 0.7rem;
    color: #888;
  }

  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.16rem 0;
    font-size: 0.78rem;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #444;
    margin-right: 0.45rem;
    transition: background 0.2s, box-shadow 0.2s;
  }

  .dot.on {
    background: #34d399;
    box-shadow: 0 0 6px #34d399;
  }

  .stage {
    color: #d4d4d4;
    font-weight: 500;
  }

  .label {
    color: #888;
    font-size: 0.74rem;
  }

  .value {
    font-family: ui-monospace, "SF Mono", Monaco, monospace;
    color: #d4d4d4;
    font-size: 0.75rem;
    font-weight: 500;
  }

  .footer {
    margin-top: 0.5rem;
    padding-top: 0.4rem;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    font-size: 0.66rem;
    color: #666;
    letter-spacing: 0.04em;
    text-align: center;
  }
</style>
