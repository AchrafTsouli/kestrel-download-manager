<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, applyTheme, size, type Settings, type Queue, type Download } from "../lib/api";

  let s: Settings | null = $state(null);
  let downloads: Download[] = $state([]);
  let runningQueues: string[] = $state([]);
  let current = $state(0);
  let newName = $state("");
  let error = $state("");
  const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const win = getCurrentWindow();

  onMount(() => {
    call<Settings>("get_settings").then((v) => { s = v; applyTheme(v.theme); });
    call<Download[]>("list_downloads").then((v) => (downloads = v));
    call<string[]>("running_queues").then((v) => (runningQueues = v));
    const subs = [
      listen<string[]>("queues", ({ payload }) => (runningQueues = payload)),
      listen<Download>("download", ({ payload }) => { const i = downloads.findIndex((d) => d.id === payload.id); if (i < 0) downloads.push(payload); else downloads[i] = payload; }),
    ];
    return () => subs.forEach((x) => x.then((f) => f()));
  });
  const queue = $derived(s?.queues[current] as Queue | undefined);
  const files = $derived(queue ? downloads.filter((d) => d.queue === queue.name && d.status !== "Completed") : []);
  function addQueue() {
    const name = newName.trim();
    if (!s || !name || s.queues.some((q) => q.name === name)) return;
    s.queues.push({ name, maxActive: 1, scheduleEnabled: false, startTime: null, stopTime: null, days: [], after: "nothing", retries: 3 });
    current = s.queues.length - 1; newName = "";
  }
  function removeQueue() { if (s && current > 0) { s.queues.splice(current, 1); current = 0; } }
  function toggleDay(i: number) { if (queue) queue.days = queue.days.includes(i) ? queue.days.filter((x) => x !== i) : [...queue.days, i].sort(); }
  async function save(close = false) {
    if (!s) return;
    try { await call("save_settings", { settings: s }); error = ""; if (close) win.close(); } catch (e) { error = String(e); }
  }
  async function startStop() {
    if (!queue) return;
    await save();
    await call(runningQueues.includes(queue.name) ? "stop_queue" : "start_queue", { name: queue.name });
  }
</script>

<div class="win">
  {#if s && queue}
    <div class="layout">
      <aside class="queues">
        <div class="muted title">Queues</div>
        {#each s.queues as q, i}
          <button class="q" class:on={current === i} onclick={() => (current = i)}>{q.name}{#if runningQueues.includes(q.name)}<span class="live" aria-label="running"></span>{/if}</button>
        {/each}
        <div class="row add"><input class="input" bind:value={newName} placeholder="New queue" /><button class="btn small" onclick={addQueue} disabled={!newName.trim()}>Add</button></div>
        <button class="btn small" onclick={removeQueue} disabled={current === 0}>Delete queue</button>
      </aside>
      <div class="win-body">
        <h2>{queue.name}</h2>
        <fieldset class="group">
          <legend>Schedule</legend>
          <label class="check"><input type="checkbox" bind:checked={queue.scheduleEnabled} /> Start and stop this queue at set times</label>
          <div class="form">
            <label for="st">Start at</label><input id="st" class="input short" type="time" bind:value={queue.startTime} disabled={!queue.scheduleEnabled} />
            <label for="sp">Stop at</label><input id="sp" class="input short" type="time" bind:value={queue.stopTime} disabled={!queue.scheduleEnabled} />
            <span class="muted">Days</span>
            <div class="days">{#each days as day, i}<button class="day" class:on={queue.days.includes(i)} disabled={!queue.scheduleEnabled} onclick={() => toggleDay(i)}>{day}</button>{/each}<span class="note">{queue.days.length ? "" : "No day chosen: every day"}</span></div>
          </div>
        </fieldset>
        <div class="form">
          <label for="max">Download at once</label><input id="max" class="input short" type="number" min="1" max="16" bind:value={queue.maxActive} />
          <label for="after">When done</label>
          <select id="after" class="input short2" bind:value={queue.after}>
            <option value="nothing">Do nothing</option><option value="exit">Exit Kestrel</option><option value="sleep">Sleep</option><option value="hibernate">Hibernate</option><option value="shutdown">Shut down the computer</option>
          </select>
        </div>
        <fieldset class="group">
          <legend>Files in this queue ({files.length})</legend>
          {#if files.length === 0}<p class="note">Add files from the start dialog (Download later) or with right-click → Move to queue.</p>{/if}
          <div class="files">{#each files as f}<div class="file"><span>{f.name}</span><span class="muted">{size(f.total)}</span><span class="muted">{f.status}</span></div>{/each}</div>
        </fieldset>
        {#if error}<p class="error-text">{error}</p>{/if}
      </div>
    </div>
  {/if}
  <div class="win-actions">
    <div class="left"><button class="btn" onclick={startStop}>{queue && runningQueues.includes(queue.name) ? "Stop now" : "Start now"}</button></div>
    <button class="btn primary" onclick={() => save(true)}>OK</button>
    <button class="btn" onclick={() => save(false)}>Apply</button>
    <button class="btn" onclick={() => win.close()}>Close</button>
  </div>
</div>

<style>
  .layout { display: grid; grid-template-columns: 190px 1fr; min-height: 0; }
  .queues { display: flex; flex-direction: column; gap: 2px; padding: 10px 8px; background: var(--chrome); border-right: 1px solid var(--line); }
  .title { padding: 0 6px 6px; }
  .q { display: flex; align-items: center; gap: 6px; text-align: left; height: 26px; padding: 0 8px; border: 0; border-radius: 4px; background: transparent; cursor: default; }
  .q:hover { background: var(--raised); }
  .q.on { background: var(--selection); color: var(--selection-text); }
  .live { width: 7px; height: 7px; border-radius: 50%; background: var(--good); margin-left: auto; }
  .add { margin-top: auto; }
  .small { min-width: 0; height: 26px; }
  h2 { margin: 0; font-size: 14px; }
  .short { width: 120px; flex: none !important; }
  .short2 { width: 220px; }
  .days { display: flex; gap: 4px; flex-wrap: wrap; align-items: center; }
  .day { width: 40px; height: 24px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); cursor: default; }
  .day.on { background: var(--selection); border-color: var(--accent); }
  .day:disabled { opacity: 0.4; }
  .files { max-height: 140px; overflow: auto; display: grid; }
  .file { display: grid; grid-template-columns: 1fr 80px 80px; gap: 8px; padding: 3px 0; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .file span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
