<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, size, exact, percent, timeLeft, running, resumable, applyTheme, type Download, type Settings } from "../lib/api";

  let { id }: { id: string } = $props();
  let d: Download | null = $state(null);
  let details = $state(true);
  let limitOn = $state(false);
  let limitKb = $state(500);
  const win = getCurrentWindow();

  onMount(() => {
    call<Settings>("get_settings").then((s) => applyTheme(s.theme));
    call<Download | null>("get_download", { id }).then((v) => { d = v; if (v) { limitOn = v.speedLimit > 0; if (v.speedLimit) limitKb = Math.round(v.speedLimit / 1024); } });
    const subs = [
      listen<Download>("download", ({ payload }) => { if (payload.id === id) d = payload; }),
      listen<string[]>("removed", ({ payload }) => { if (payload.includes(id)) win.close(); }),
    ];
    return () => subs.forEach((s) => s.then((f) => f()));
  });
  $effect(() => { if (d) win.setTitle(`${Math.floor(percent(d))}% ${d.name}`); });

  const total = $derived(d?.total ?? 0);
  // Segment map: each part drawn at its position in the file, filled as far as it has arrived.
  const parts = $derived(d && total ? d.segments.map((s) => ({ left: (s.start / total) * 100, width: (((s.end ?? total - 1) - s.start + 1) / total) * 100, fill: s.end != null ? Math.min(100, (s.done / (s.end - s.start + 1)) * 100) : 100 })) : []);
  function applyLimit() { if (d) call("set_speed_limit", { id, bytesPerSecond: limitOn ? Math.max(1, limitKb) * 1024 : 0 }); }
  const toggle = () => d && (running(d.status) || d.status === "Queued" ? call("pause_downloads", { ids: [id] }) : call("resume_downloads", { ids: [id] }));
  async function cancel() { if (d && running(d.status)) await call("pause_downloads", { ids: [id] }); win.close(); }
</script>

<div class="win">
  <div class="win-body">
    {#if !d}
      <p class="muted">This download is no longer in the list.</p>
    {:else}
      <div class="head">
        <div class="url" title={d.url}>{d.url}</div>
      </div>
      <dl class="kv">
        <dt>Status</dt><dd class:bad={d.status === "Failed"}>{d.status === "Downloading" ? "Receiving data…" : d.status}{d.error && d.status !== "Downloading" ? ` · ${d.error}` : ""}</dd>
        <dt>File size</dt><dd>{d.total != null ? (d.estimated ? "≈ " : "") + size(d.total) : "Unknown"}</dd>
        <dt>Downloaded</dt><dd>{size(d.written, "0 B")} ({percent(d).toFixed(2)}%)</dd>
        <dt>Transfer rate</dt><dd>{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : "—"}</dd>
        <dt>Time left</dt><dd>{timeLeft(d)}</dd>
        <dt>Resume capability</dt>
        {#if d.kind === "video"}<dd class="good">Yes: video parts are kept while paused</dd>
        {:else}<dd class={d.resumable ? "good" : "warn"}>{d.total == null && !d.resumable ? "Unknown until the server answers" : d.resumable ? "Yes" : "No: this server does not allow resuming"}</dd>{/if}
      </dl>
      <div class="track" aria-label="Progress"><i style:width="{percent(d)}%"></i></div>
      {#if d.phase}
        <div class="phase"><span>{d.phase}…</span>{#if d.phaseFraction != null}<span class="muted">{Math.round(d.phaseFraction * 100)}%</span>{/if}</div>
        <div class="track second" class:busy={d.phaseFraction == null} aria-label={d.phase}><i style:width="{(d.phaseFraction ?? 1) * 100}%"></i></div>
      {/if}
      {#if d.note}<p class="note">{d.note}</p>{/if}

      {#if details && d.kind !== "video"}
        <div class="map" aria-label="Parts of the file">
          {#each parts as p}<span class="part" style:left="{p.left}%" style:width="{p.width}%"><i style:width="{p.fill}%"></i></span>{/each}
        </div>
        <div class="conns">
          <div class="conn head"><span>N.</span><span>Downloaded</span><span>Info</span></div>
          {#each d.segments as s, i}
            <div class="conn"><span>{i + 1}</span><span>{size(s.done, "0 B")}</span><span class="muted">{s.end != null && s.done >= s.end - s.start + 1 ? "Complete" : running(d.status) ? "Receiving data…" : "Waiting"}</span></div>
          {/each}
        </div>
        <div class="limit row">
          <label class="check"><input type="checkbox" bind:checked={limitOn} onchange={applyLimit} /> Speed limiter</label>
          <input class="input num" type="number" min="1" bind:value={limitKb} disabled={!limitOn} onchange={applyLimit} aria-label="Maximum speed in KB/s" /> <span class="muted">KB/s</span>
          <span class="muted bytes">{exact(d.written)}</span>
        </div>
      {/if}
    {/if}
  </div>
  <div class="win-actions">
    <div class="left"><button class="btn" onclick={() => (details = !details)}>{details ? "« Hide details" : "Show details »"}</button></div>
    <button class="btn primary" onclick={toggle} disabled={!d || d.status === "Completed"}>{d && (running(d.status) || d.status === "Queued") ? "Pause" : d && resumable(d.status) ? "Resume" : "Pause"}</button>
    <button class="btn" onclick={cancel}>{d && running(d.status) ? "Cancel" : "Close"}</button>
  </div>
</div>

<style>
  .head { display: flex; align-items: center; gap: 8px; }
  .url { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--accent); }
  .bad { color: var(--bad); } .good { color: var(--good); } .warn { color: var(--warn); }
  .map { position: relative; height: 14px; background: var(--bg); border: 1px solid var(--line); border-radius: 3px; overflow: hidden; }
  .part { position: absolute; top: 0; bottom: 0; border-right: 1px solid var(--surface); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .part i { display: block; height: 100%; background: var(--accent); }
  .conns { border: 1px solid var(--line); border-radius: 4px; max-height: 130px; overflow: auto; font-variant-numeric: tabular-nums; }
  .conn { display: grid; grid-template-columns: 40px 110px 1fr; gap: 8px; padding: 3px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .conn.head { color: var(--muted); background: var(--chrome); position: sticky; top: 0; }
  .phase { display: flex; justify-content: space-between; }
  .second.busy i { opacity: 0.45; animation: pulse 1.2s ease-in-out infinite alternate; }
  @keyframes pulse { to { opacity: 0.9; } }
  .num { width: 90px; flex: none !important; }
  .bytes { margin-left: auto; font-variant-numeric: tabular-nums; }
</style>
