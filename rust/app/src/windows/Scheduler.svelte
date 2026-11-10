<script lang="ts">
  // The Scheduler, laid out like IDM's: the queues on the left; on the right the chosen queue's
  // "Schedule" and "Files in the queue" tabs; Start now / Stop / Apply / Close below.
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { call, applyTheme, size, timeLeft, statusText, type Settings, type Queue, type Download } from "../lib/api";

  let s: Settings | null = $state(null);
  let downloads: Download[] = $state([]);
  let runningQueues: string[] = $state([]);
  let current = $state(0);
  let tab = $state<"schedule" | "files">("schedule");
  let naming = $state(false);
  let newName = $state("");
  let picked: string | null = $state(null);
  let dirty = $state(false);
  let error = $state("");
  const dayNames = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  // IDM's three columns: Sun Mon Tue | Wed Thu Fri | Sat
  const dayColumns = [[0, 1, 2], [3, 4, 5], [6]];
  const win = getCurrentWindow();

  onMount(() => {
    call<Settings>("get_settings").then((v) => { s = v; applyTheme(v.theme); });
    call<Download[]>("list_downloads").then((v) => (downloads = v));
    call<string[]>("running_queues").then((v) => (runningQueues = v));
    const subs = [
      listen<string[]>("queues", ({ payload }) => (runningQueues = payload)),
      listen<Download>("download", ({ payload }) => { const i = downloads.findIndex((d) => d.id === payload.id); if (i < 0) downloads.push(payload); else downloads[i] = payload; }),
      listen<string[]>("removed", ({ payload }) => (downloads = downloads.filter((d) => !payload.includes(d.id)))),
    ];
    return () => subs.forEach((x) => x.then((f) => f()));
  });
  const queue = $derived(s?.queues[current] as Queue | undefined);
  const files = $derived(queue ? downloads.filter((d) => d.queue === queue.name && d.status !== "Completed").sort((a, b) => a.queueOrder - b.queueOrder) : []);
  const isRunning = $derived(!!queue && runningQueues.includes(queue.name));
  const changed = () => (dirty = true);

  // Start and stop times are each optional; the queue's schedule is on when either is set.
  const startOn = $derived(!!queue?.startTime);
  const stopOn = $derived(!!queue?.stopTime);
  function setStart(on: boolean) { if (!queue) return; queue.startTime = on ? queue.startTime || "22:00" : null; queue.scheduleEnabled = !!queue.startTime || !!queue.stopTime; changed(); }
  function setStop(on: boolean) { if (!queue) return; queue.stopTime = on ? queue.stopTime || "06:30" : null; queue.scheduleEnabled = !!queue.startTime || !!queue.stopTime; changed(); }
  const daily = $derived((queue?.days.length ?? 0) > 0);
  function setDaily(on: boolean) { if (queue) { queue.days = on ? [0, 1, 2, 3, 4, 5, 6] : []; changed(); } }
  function toggleDay(i: number) { if (!queue) return; queue.days = queue.days.includes(i) ? queue.days.filter((x) => x !== i) : [...queue.days, i].sort(); if (!queue.days.length) queue.days = [i]; changed(); }

  // "Turn off computer when done" with its kind, or "Exit Kestrel when done": one of them.
  const turnOff = $derived(!!queue && ["shutdown", "sleep", "hibernate"].includes(queue.after));
  let powerKind = $state<"shutdown" | "sleep" | "hibernate">("shutdown");
  $effect(() => { if (queue && turnOff) powerKind = queue.after as typeof powerKind; });
  function setTurnOff(on: boolean) { if (queue) { queue.after = on ? powerKind : "nothing"; changed(); } }
  function setExit(on: boolean) { if (queue) { queue.after = on ? "exit" : "nothing"; changed(); } }
  function setKind() { if (queue && turnOff) { queue.after = powerKind; changed(); } }

  async function browseOpen() {
    const path = await pickFile({ title: "Open the following file when done" });
    if (typeof path === "string" && queue) { queue.openWhenDone = path; changed(); }
  }
  function addQueue(e: SubmitEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!s || !name) return;
    if (s.queues.some((q) => q.name.toLowerCase() === name.toLowerCase())) { error = "A queue with this name already exists."; return; }
    s.queues.push({ name, maxActive: 1, scheduleEnabled: false, startTime: null, stopTime: null, days: [], after: "nothing", retries: 10, startOnLaunch: false, openWhenDone: "" });
    current = s.queues.length - 1; newName = ""; naming = false; error = ""; changed();
  }
  function removeQueue() { if (s && current > 0) { s.queues.splice(current, 1); current = 0; changed(); } }
  async function save() {
    if (!s) return true;
    try { await call("save_settings", { settings: s }); error = ""; dirty = false; return true; } catch (e) { error = String(e); return false; }
  }
  async function startNow() { if (queue && (await save())) await call("start_queue", { name: queue.name }); }
  async function stopNow() { if (queue) await call("stop_queue", { name: queue.name }); }
  const move = (delta: number) => picked && call("move_in_queue", { id: picked, delta });
  async function removeFromQueue() {
    if (!picked) return;
    // Back to the main queue; from the main queue the file only stops waiting.
    if (queue && queue.name !== "Main download queue") await call("set_queue", { ids: [picked], queue: "Main download queue" });
    else await call("pause_downloads", { ids: [picked] });
    picked = null;
  }
  async function close() { if (dirty) await save(); win.close(); }
</script>

<div class="sched">
  {#if s && queue}
    <aside class="left">
      <div class="left-title">Queues</div>
      <div class="queue-list" role="listbox" aria-label="Queues">
        {#each s.queues as q, i}
          <button role="option" aria-selected={current === i} class="q" class:on={current === i} onclick={() => { current = i; picked = null; }}>
            <svg width="18" height="16" viewBox="0 0 18 16" aria-hidden="true"><rect x="1" y="4" width="12" height="10" rx="1.5" fill="var(--q-back)" /><rect x="4" y="1.5" width="12" height="10" rx="1.5" fill="var(--q-front)" stroke="var(--q-edge)" /></svg>
            <span>{q.name}</span>
            {#if runningQueues.includes(q.name)}<i class="live" title="Running"></i>{/if}
          </button>
        {/each}
      </div>
      {#if naming}
        <form class="naming" onsubmit={addQueue}>
          <!-- svelte-ignore a11y_autofocus -->
          <input class="input" bind:value={newName} placeholder="Queue name" autofocus onkeydown={(e) => e.key === "Escape" && (naming = false)} />
          <button class="btn small" disabled={!newName.trim()}>Add</button>
        </form>
      {/if}
      <div class="left-buttons">
        <button class="btn" onclick={() => { naming = true; newName = ""; }}>New queue</button>
        <button class="btn" onclick={removeQueue} disabled={current === 0}>Delete</button>
      </div>
    </aside>

    <section class="right">
      <div class="queue-name">{queue.name}</div>
      <div class="tabs" role="tablist">
        <button role="tab" class="tab" class:on={tab === "schedule"} aria-selected={tab === "schedule"} onclick={() => (tab = "schedule")}>Schedule</button>
        <button role="tab" class="tab" class:on={tab === "files"} aria-selected={tab === "files"} onclick={() => (tab = "files")}>Files in the queue</button>
      </div>
      <div class="page">
        {#if tab === "schedule"}
          <label class="check"><input type="checkbox" bind:checked={queue.startOnLaunch} onchange={changed} /> Start download on Kestrel startup</label>
          <hr />
          <div class="line">
            <label class="check"><input type="checkbox" checked={startOn} onchange={(e) => setStart(e.currentTarget.checked)} /> Start download at</label>
            <input class="input time" type="time" bind:value={queue.startTime} disabled={!startOn} onchange={changed} />
          </div>
          <div class="when" class:off={!startOn && !stopOn}>
            <label class="radio"><input type="radio" name="when" checked={!daily} disabled={!startOn && !stopOn} onchange={() => setDaily(false)} /> Once (the next time it comes)</label>
            <div class="daily">
              <label class="radio"><input type="radio" name="when" checked={daily} disabled={!startOn && !stopOn} onchange={() => setDaily(true)} /> Daily</label>
              <div class="days">
                {#each dayColumns as column}
                  <div class="col">{#each column as d}<label class="check" class:off={!daily}><input type="checkbox" checked={queue.days.includes(d)} disabled={!daily || (!startOn && !stopOn)} onchange={() => toggleDay(d)} /> {dayNames[d]}</label>{/each}</div>
                {/each}
              </div>
            </div>
          </div>
          <hr />
          <div class="line">
            <label class="check"><input type="checkbox" checked={stopOn} onchange={(e) => setStop(e.currentTarget.checked)} /> Stop download at</label>
            <input class="input time" type="time" bind:value={queue.stopTime} disabled={!stopOn} onchange={changed} />
          </div>
          <div class="line">
            <span>Number of retries for each file if downloading failed:</span>
            <input class="input num" type="number" min="0" max="20" bind:value={queue.retries} onchange={changed} />
          </div>
          <hr />
          <label class="check"><input type="checkbox" checked={!!queue.openWhenDone} onchange={(e) => { if (!e.currentTarget.checked && queue) { queue.openWhenDone = ""; changed(); } else browseOpen(); }} /> Open the following file when done:</label>
          <div class="line"><input class="input" readonly value={queue.openWhenDone} placeholder="" /><button class="btn dots" onclick={browseOpen} title="Choose a file">...</button></div>
          <label class="check"><input type="checkbox" checked={queue.after === "exit"} onchange={(e) => setExit(e.currentTarget.checked)} /> Exit Kestrel when done</label>
          <div class="line">
            <label class="check"><input type="checkbox" checked={turnOff} onchange={(e) => setTurnOff(e.currentTarget.checked)} /> Turn off computer when done</label>
            <select class="input kind" bind:value={powerKind} disabled={!turnOff} onchange={setKind}>
              <option value="shutdown">Shut down</option><option value="sleep">Sleep</option><option value="hibernate">Hibernate</option>
            </select>
          </div>
        {:else}
          <div class="line">
            <span>Download</span>
            <input class="input num" type="number" min="1" max="16" bind:value={queue.maxActive} onchange={changed} />
            <span>files at the same time</span>
          </div>
          <div class="table" role="grid" aria-label="Files in the queue">
            <div class="tr head" role="row"><span>File Name</span><span>Size</span><span>Status</span><span>Time left</span></div>
            {#each files as f (f.id)}
              <div class="tr" role="row" class:sel={picked === f.id} onmousedown={() => (picked = f.id)} tabindex="-1">
                <span title={f.name}>{f.name}</span><span>{size(f.total)}</span><span>{statusText(f)}</span><span>{timeLeft(f)}</span>
              </div>
            {:else}
              <p class="empty">No files are waiting in this queue. Use "Download Later" in the start dialog, or right-click a download → Move to queue.</p>
            {/each}
          </div>
          <div class="file-buttons">
            <button class="icon-btn" onclick={() => move(1)} disabled={!picked} title="Move down" aria-label="Move down"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2v10M3.5 7.5 8 12l4.5-4.5" /></svg></button>
            <button class="icon-btn" onclick={() => move(-1)} disabled={!picked} title="Move up" aria-label="Move up"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 14V4M3.5 8.5 8 4l4.5 4.5" /></svg></button>
            <button class="icon-btn bad" onclick={removeFromQueue} disabled={!picked} title="Remove from this queue" aria-label="Remove from this queue"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg></button>
          </div>
        {/if}
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
    </section>

    <div class="bottom">
      <button class="btn" onclick={startNow} disabled={isRunning}>Start now</button>
      <button class="btn" onclick={stopNow} disabled={!isRunning}>Stop</button>
      <div class="grow"></div>
      <button class="btn" onclick={save} disabled={!dirty}>Apply</button>
      <button class="btn" onclick={close}>Close</button>
    </div>
  {/if}
</div>

<style>
  .sched { display: grid; grid-template-columns: 200px 1fr; grid-template-rows: 1fr auto; height: 100%; gap: 0 12px; padding: 10px 12px; box-sizing: border-box; --q-back: #3fae5a; --q-front: #f2c94c; --q-edge: #b8902b; }
  .left { display: flex; flex-direction: column; gap: 6px; min-height: 0; }
  .left-title { text-align: center; font-weight: 600; color: var(--muted); }
  .queue-list { flex: 1; min-height: 0; overflow: auto; border: 1px solid var(--line); border-radius: 3px; background: var(--surface); padding: 3px; }
  .q { display: flex; align-items: center; gap: 6px; width: 100%; height: 24px; padding: 0 6px; border: 0; border-radius: 3px; background: transparent; text-align: left; cursor: default; }
  .q span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .q:hover { background: var(--raised); }
  .q.on { background: var(--selection); color: var(--selection-text); }
  .live { width: 7px; height: 7px; border-radius: 50%; background: var(--good); margin-left: auto; flex: none; }
  .naming { display: flex; gap: 4px; }
  .naming .input { min-width: 0; }
  .left-buttons { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
  .left-buttons .btn { min-width: 0; }
  .small { min-width: 0; padding: 0 10px; }
  .right { display: flex; flex-direction: column; min-height: 0; border-left: 1px solid var(--line); padding-left: 12px; }
  .queue-name { text-align: center; color: var(--muted); margin-bottom: 4px; }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--line); }
  .tab { padding: 4px 12px; border: 1px solid var(--line); border-bottom: 0; border-radius: 3px 3px 0 0; background: var(--raised); color: var(--muted); cursor: default; margin-bottom: -1px; }
  .tab.on { background: var(--surface); color: var(--text); }
  .page { flex: 1; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; background: var(--surface); border: 1px solid var(--line); border-top: 0; }
  hr { width: 100%; border: 0; border-top: 1px solid var(--line); margin: 2px 0; }
  .line { display: flex; align-items: center; gap: 8px; }
  .line .input:not(.time):not(.num):not(.kind) { flex: 1; min-width: 0; }
  .time { width: 120px; flex: none; }
  .num { width: 64px; flex: none; }
  .kind { width: 140px; flex: none; margin-left: auto; }
  .dots { min-width: 0; width: 30px; padding: 0; }
  .when { display: grid; gap: 4px; padding-left: 22px; }
  .when.off { opacity: 0.55; }
  .daily { display: flex; gap: 14px; align-items: flex-start; }
  .days { display: grid; grid-template-columns: repeat(3, auto); gap: 0 18px; }
  .col { display: grid; gap: 2px; }
  .check.off { opacity: 0.55; }
  .radio { display: flex; align-items: center; gap: 6px; }
  .table { flex: 1; min-height: 160px; overflow: auto; border: 1px solid var(--line); border-radius: 2px; background: var(--bg); }
  .tr { display: grid; grid-template-columns: 1fr 80px 110px 80px; gap: 8px; padding: 3px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); cursor: default; }
  .tr span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tr.head { position: sticky; top: 0; background: var(--chrome); color: var(--muted); }
  .tr.sel { background: var(--selection); color: var(--selection-text); }
  .empty { margin: 0; padding: 12px; color: var(--muted); }
  .file-buttons { display: flex; gap: 6px; }
  .icon-btn { width: 30px; height: 28px; display: grid; place-items: center; border: 1px solid var(--line); border-radius: 4px; background: var(--raised); color: var(--warn); cursor: default; }
  .icon-btn.bad { color: var(--bad); }
  .icon-btn:disabled { opacity: 0.4; }
  .icon-btn svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2.2; stroke-linecap: round; stroke-linejoin: round; }
  .bottom { grid-column: 1 / 3; display: flex; gap: 8px; padding-top: 10px; }
  .grow { flex: 1; }
</style>
