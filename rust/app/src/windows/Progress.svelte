<script lang="ts">
  // The progress window, laid out like Kestrel 1.x's: three tabs (status, speed limiter, options on
  // completion), the progress bar, Hide details / Pause / Cancel, then the connections.
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, size, exact, percent, timeLeft, running, resumable, applyTheme, type AfterQueue, type Download, type Settings } from "../lib/api";

  let { id }: { id: string } = $props();
  let d: Download | null = $state(null);
  let settings: Settings | null = $state(null);
  let tab = $state("status");
  let details = $state(true);
  let limitOn = $state(false);
  let limitKb = $state(500);
  // Options on completion
  let turnOff = $state(false);
  let powerKind: AfterQueue = $state("shutdown");
  let exitWhenDone = $state(false);
  const win = getCurrentWindow();
  // Speed over the last minute, one sample a second.
  let history: number[] = $state([]);
  const SAMPLES = 60;

  onMount(() => {
    call<Settings>("get_settings").then((s) => { settings = s; applyTheme(s.theme); });
    call<Download | null>("get_download", { id }).then((v) => { d = v; if (v) { limitOn = v.speedLimit > 0; if (v.speedLimit) limitKb = Math.round(v.speedLimit / 1024); } });
    call<AfterQueue>("get_after_complete", { id }).then((a) => {
      if (a === "exit") exitWhenDone = true;
      else if (a !== "nothing") { turnOff = true; powerKind = a; }
    });
    const subs = [
      listen<Download>("download", ({ payload }) => { if (payload.id === id) d = payload; }),
      listen<Download[]>("downloads", ({ payload }) => { const mine = payload.find((x) => x.id === id); if (mine) d = mine; }),
      listen<string[]>("removed", ({ payload }) => { if (payload.includes(id)) win.close(); }),
      listen<Settings>("settings", ({ payload }) => (settings = payload)),
    ];
    const sampler = setInterval(() => { if (d) history = [...history.slice(-(SAMPLES - 1)), d.status === "Downloading" || d.status === "Connecting" ? d.bytesPerSecond : 0]; }, 1000);
    return () => { clearInterval(sampler); subs.forEach((s) => s.then((f) => f())); };
  });
  const peak = $derived(Math.max(1, ...history));
  const graph = $derived.by(() => {
    if (history.length < 2) return { line: "", area: "" };
    const x = (i: number) => ((i + SAMPLES - history.length) / (SAMPLES - 1)) * 100;
    const y = (v: number) => 38 - (v / peak) * 34;
    const points = history.map((v, i) => `${x(i).toFixed(2)},${y(v).toFixed(2)}`);
    return { line: points.join(" "), area: `${x(0).toFixed(2)},40 ${points.join(" ")} 100,40` };
  });
  const title = $derived(d ? `${Math.floor(percent(d))}% ${d.name}` : "");
  $effect(() => { if (title) win.setTitle(title); });

  const total = $derived(d?.total ?? 0);
  // Segment map: each part drawn at its position in the file, filled as far as it has arrived.
  const parts = $derived(d && total ? d.segments.map((s) => ({ left: (s.start / total) * 100, width: (((s.end ?? total - 1) - s.start + 1) / total) * 100, fill: s.end != null ? Math.min(100, (s.done / (s.end - s.start + 1)) * 100) : 100 })) : []);
  function applyLimit() { if (d) call("set_speed_limit", { id, bytesPerSecond: limitOn ? Math.max(1, limitKb) * 1024 : 0 }); }
  const toggle = () => d && (running(d.status) || d.status === "Queued" ? call("pause_downloads", { ids: [id] }) : call("resume_downloads", { ids: [id] }));
  async function cancel() { if (d && running(d.status)) await call("pause_downloads", { ids: [id] }); win.close(); }

  const showComplete = $derived(settings?.showCompleteDialog ?? true);
  function setShowComplete(on: boolean) { if (settings) call("save_settings", { settings: { ...settings, showCompleteDialog: on } }); }
  function applyAfter() { call("set_after_complete", { id, action: turnOff ? powerKind : exitWhenDone ? "exit" : "nothing" }); }
  const statusText = $derived(!d ? "" : d.status === "Downloading" ? "Receiving data…" : d.status === "Connecting" ? "Connecting…" : d.status + (d.error ? ` · ${d.error}` : ""));
</script>

<div class="frame">
  <header class="titlebar" data-tauri-drag-region>
    <img src="/logo.png" alt="" width="16" height="16" data-tauri-drag-region />
    <span class="caption" data-tauri-drag-region>{title || "Download progress"}</span>
    <button class="cap tray" title="Download in the background (it stays in the tray)" aria-label="Download in the background" onclick={() => call("to_background", { id })}>
      <img class="tray-dark" src="/to-tray-dark.png" alt="" width="20" height="20" /><img class="tray-light" src="/to-tray-light.png" alt="" width="20" height="20" />
    </button>
    <button class="cap" title="Minimize" aria-label="Minimize" onclick={() => win.minimize()}><svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" stroke="currentColor" /></svg></button>
    <button class="cap" title="Maximize" aria-label="Maximize" onclick={() => win.toggleMaximize()}><svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" /></svg></button>
    <button class="cap close" title="Close" aria-label="Close" onclick={() => win.close()}><svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg></button>
  </header>
<div class="win">
  <div class="win-body">
    {#if !d}
      <p class="muted">This download is no longer in the list.</p>
    {:else}
      <div class="tabbox">
        <div class="tabs" role="tablist">
          <button role="tab" class="tab" class:on={tab === "status"} onclick={() => (tab = "status")}>Download status</button>
          <button role="tab" class="tab" class:on={tab === "limiter"} onclick={() => (tab = "limiter")}>Speed Limiter</button>
          <button role="tab" class="tab" class:on={tab === "after"} onclick={() => (tab = "after")}>Options on completion</button>
        </div>
        <div class="page">
          {#if tab === "status"}
            <div class="url" title={d.url}>{d.url}</div>
            <dl class="kv">
              <dt>Status</dt><dd class="accent" class:bad={d.status === "Failed"}>{statusText}</dd>
              <dt>File size</dt><dd>{d.total != null ? (d.estimated ? "≈ " : "") + size(d.total) : "Unknown"}</dd>
              <dt>Downloaded</dt><dd>{size(d.written, "0 B")} ( {percent(d).toFixed(2)} % )</dd>
              <dt>Transfer rate</dt><dd class="strong">{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/sec" : "—"}</dd>
              <dt>Time left</dt><dd>{timeLeft(d)}</dd>
              <dt>Resume capability</dt>
              {#if d.kind === "video"}<dd class="good">Yes: video parts are kept while paused</dd>
              {:else if d.kind === "torrent"}<dd class="good">Yes: a torrent continues where it stopped</dd>
              {:else}<dd class={d.resumable ? "good" : "warn"}>{d.total == null && !d.resumable ? "Unknown until the server answers" : d.resumable ? "Yes" : "No: this server does not allow resuming"}</dd>{/if}
            </dl>
            {#if history.length > 1}
              <div class="speed-graph" title="Speed over the last minute">
                <svg viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true"><polygon points={graph.area} /><polyline points={graph.line} /></svg>
                <span class="peak">{size(peak)}/s</span>
              </div>
            {/if}
          {:else if tab === "limiter"}
            <div class="pane">
              <div class="line"><span class="muted label">Transfer rate</span><b>{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/sec" : "0 KB/sec"}</b></div>
              <label class="check"><input type="checkbox" bind:checked={limitOn} onchange={applyLimit} /> Use Speed Limiter</label>
              <div class="line indent"><span class="muted">Maximum download speed:</span><input class="input num" type="number" min="1" bind:value={limitKb} disabled={!limitOn} onchange={applyLimit} aria-label="Maximum speed in KB/s" /><span class="muted">KBytes/sec</span></div>
              <label class="check"><input type="checkbox" checked disabled /> Remember Speed Limiter settings for this file on download stop/resume</label>
              <div class="line end"><button class="btn" onclick={() => (tab = "status")}>Hide tab</button></div>
            </div>
          {:else}
            <div class="pane">
              <div class="path" title={d.path}>Save To: {d.path}</div>
              <label class="check"><input type="checkbox" checked={showComplete} onchange={(e) => setShowComplete((e.currentTarget as HTMLInputElement).checked)} /> Show download complete dialog</label>
              <p class="hint">These settings are unavailable when "Show download complete dialog" is turned on</p>
              <div class="line">
                <label class="check"><input type="checkbox" bind:checked={turnOff} disabled={showComplete} onchange={() => { if (turnOff) exitWhenDone = false; applyAfter(); }} /> Turn off computer when done</label>
                <select class="input kind" bind:value={powerKind} disabled={showComplete || !turnOff} onchange={applyAfter}>
                  <option value="shutdown">Shut down</option><option value="sleep">Sleep</option><option value="hibernate">Hibernate</option>
                </select>
              </div>
              <label class="check"><input type="checkbox" bind:checked={exitWhenDone} disabled={showComplete} onchange={() => { if (exitWhenDone) turnOff = false; applyAfter(); }} /> Exit Kestrel when done</label>
              <div class="line end"><button class="btn" onclick={() => (tab = "status")}>Hide tab</button></div>
            </div>
          {/if}
        </div>
      </div>

      <div class="track main" aria-label="Progress"><i style:width="{percent(d)}%"></i></div>
      {#if d.phase}
        <div class="phase"><span>{d.phase}…</span>{#if d.phaseFraction != null}<span class="muted">{Math.round(d.phaseFraction * 100)}%</span>{/if}</div>
        <div class="track thin" class:busy={d.phaseFraction == null} aria-label={d.phase}><i style:width="{(d.phaseFraction ?? 1) * 100}%"></i></div>
      {/if}
      {#if d.note}<p class="note">{d.note}</p>{/if}

      <div class="actions">
        <button class="btn" onclick={() => (details = !details)}>{details ? "<< Hide details" : "Show details >>"}</button>
        <div class="grow"></div>
        <button class="btn" onclick={toggle} disabled={d.status === "Completed"}>{running(d.status) || d.status === "Queued" ? "Pause" : resumable(d.status) ? "Start" : "Pause"}</button>
        <button class="btn" onclick={cancel}>{running(d.status) ? "Cancel" : "Close"}</button>
      </div>

      {#if details && d.kind === "file"}
        <div class="muted small">Start positions and download progress by connections</div>
        <div class="map" aria-label="Parts of the file">
          {#each parts as p}<span class="part" style:left="{p.left}%" style:width="{p.width}%"><i style:width="{p.fill}%"></i></span>{/each}
        </div>
        <div class="conns">
          <div class="conn head"><span>N.</span><span>Downloaded</span><span>Info</span></div>
          {#each d.segments as s, i}
            <div class="conn"><span>{i + 1}</span><span>{size(s.done, "0 B")}</span><span class="muted">{s.end != null && s.done >= s.end - s.start + 1 ? "Complete" : running(d.status) ? "Receiving data…" : "Disconnected"}</span></div>
          {/each}
        </div>
        <div class="muted bytes">{exact(d.written)}</div>
      {/if}
    {/if}
  </div>
</div>
</div>

<style>
  .frame { display: grid; grid-template-rows: 32px 1fr; height: 100%; border: 1px solid var(--line); box-sizing: border-box; }
  .titlebar { display: flex; align-items: center; gap: 7px; padding-inline-start: 10px; background: var(--chrome); border-bottom: 1px solid var(--line); }
  .caption { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cap { width: 46px; height: 31px; border: 0; background: transparent; color: var(--text); font-family: "Segoe Fluent Icons", "Segoe MDL2 Assets"; font-size: 10px; cursor: default; display: grid; place-items: center; }
  .cap:hover { background: var(--raised); }
  /* The same icon as the background downloads' tray icon, in the theme's version. */
  .tray-light { display: none; }
  :global([data-theme="light"]) .tray-dark { display: none; }
  :global([data-theme="light"]) .tray-light { display: block; }
  .cap.close:hover { background: #c42b1c; color: #fff; }
  .win { min-height: 0; }
  .win-body { display: flex; flex-direction: column; gap: 10px; }
  .tabbox { border: 1px solid var(--line); border-radius: 3px; background: var(--chrome); padding: 4px; }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--line); }
  .tab { padding: 5px 12px; border: 1px solid var(--line); border-bottom: 0; border-radius: 3px 3px 0 0; background: var(--raised); color: var(--muted); cursor: default; margin-bottom: -1px; }
  .tab.on { background: var(--surface); color: var(--text); }
  .page { background: var(--surface); border: 1px solid var(--line); border-top: 0; padding: 10px 10px 12px; min-height: 172px; }
  .url { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-bottom: 8px; }
  .kv { display: grid; grid-template-columns: 116px 1fr; row-gap: 6px; margin: 0; }
  .kv dt { color: var(--muted); }
  .kv dd { margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .accent { color: var(--accent); } .strong { font-weight: 600; }
  .bad { color: var(--bad); } .good { color: var(--good); } .warn { color: var(--warn); }
  .pane { display: flex; flex-direction: column; gap: 8px; }
  .line { display: flex; align-items: center; gap: 8px; }
  .label { width: 130px; }
  .indent { padding-inline-start: 18px; }
  .num { width: 70px; flex: none !important; }
  .kind { width: 120px; flex: none !important; }
  .hint { margin: 0; color: var(--muted); font-size: 11px; }
  .path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track { height: 16px; }
  /* Like IDM: a plain green bar for the file. */
  .track.main { height: 20px; border-radius: 2px; }
  .track.main i { background: linear-gradient(180deg, #2fbf5f, #178a3e); }
  .line.end { justify-content: flex-end; margin-top: auto; }
  .pane { min-height: 148px; }
  .track.thin { height: 6px; }
  .track.busy i { opacity: 0.45; animation: pulse 1.2s ease-in-out infinite alternate; }
  @keyframes pulse { to { opacity: 0.9; } }
  .phase { display: flex; justify-content: space-between; color: var(--muted); font-size: 11px; }
  .actions { display: flex; gap: 8px; }
  .grow { flex: 1; }
  .small { font-size: 11px; }
  .map { position: relative; height: 22px; background: var(--bg); border: 1px solid var(--line); border-radius: 2px; overflow: hidden; }
  .part { position: absolute; top: 0; bottom: 0; border-inline-end: 1px solid #c0392b; background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .part i { display: block; height: 100%; background: linear-gradient(180deg, var(--accent), color-mix(in srgb, var(--accent) 60%, #1e3a8a)); }
  .conns { border: 1px solid var(--line); border-radius: 2px; max-height: 150px; overflow: auto; font-variant-numeric: tabular-nums; background: var(--bg); }
  .conn { display: grid; grid-template-columns: 40px 112px 1fr; gap: 8px; padding: 3px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .conn.head { color: var(--muted); background: var(--chrome); position: sticky; top: 0; }
  .bytes { text-align: end; font-size: 11px; font-variant-numeric: tabular-nums; }
  .speed-graph { position: relative; height: 52px; margin-top: 6px; border: 1px solid var(--line); border-radius: 3px; background: var(--bg); overflow: hidden; }
  .speed-graph svg { width: 100%; height: 100%; display: block; }
  .speed-graph polygon { fill: color-mix(in srgb, var(--accent) 22%, transparent); }
  .speed-graph polyline { fill: none; stroke: var(--accent); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
  .speed-graph .peak { position: absolute; top: 2px; right: 6px; font-size: 10.5px; color: var(--muted); }
</style>
