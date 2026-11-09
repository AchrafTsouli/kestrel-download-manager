<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import Icon from "./lib/Icon.svelte";
  import { call, categories, running, resumable, size, percent, timeLeft, stamp, statusText, statusClass, applyTheme, type Download, type Settings, type AfterQueue } from "./lib/api";

  let downloads: Download[] = $state([]);
  let settings: Settings | null = $state(null);
  let runningQueues: string[] = $state([]);
  let selected: Set<string> = $state(new Set());
  let anchor: string | null = $state(null);
  let view = $state("all");
  let search = $state("");
  let menu: string | null = $state(null);
  let context: { x: number; y: number } | null = $state(null);
  let queueMenu = $state(false);
  let adding = $state(false);
  let address = $state("");
  let addError = $state("");
  let confirmDelete: string[] | null = $state(null);
  let deleteFiles = $state(false);
  let renaming: { id: string; name: string } | null = $state(null);
  let about = $state(false);
  let version = $state("");
  let countdown: { queue: string; action: AfterQueue; seconds: number } | null = $state(null);
  let showCategories = $state(true);
  let showToolbar = $state(true);
  let message = $state("Ready.");

  const queues = $derived(settings?.queues ?? []);
  function inView(d: Download, key: string) {
    if (key === "all") return true;
    if (key === "unfinished") return d.status !== "Completed";
    if (key === "finished") return d.status === "Completed";
    if (key === "failed") return d.status === "Failed";
    if (key.startsWith("type:")) return d.category === key.slice(5);
    if (key.startsWith("queue:")) return d.queue === key.slice(6) && d.status !== "Completed";
    return false;
  }
  const visible = $derived(downloads.filter((d) => inView(d, view) && (!search || (d.name + " " + d.description + " " + d.url).toLowerCase().includes(search.toLowerCase()))));
  const chosen = $derived(downloads.filter((d) => selected.has(d.id)));
  const canResume = $derived(chosen.some((d) => resumable(d.status)));
  const canStop = $derived(chosen.some((d) => running(d.status) || d.status === "Queued"));
  const active = $derived(downloads.filter((d) => running(d.status)));
  const totalSpeed = $derived(downloads.reduce((s, d) => s + d.bytesPerSecond, 0));
  const viewTitle = $derived(view === "all" ? "All Downloads" : view === "unfinished" ? "Unfinished" : view === "finished" ? "Finished" : view === "failed" ? "Failed" : view.replace(/^(type|queue):/, ""));
  const count = (key: string) => downloads.filter((d) => inView(d, key)).length;
  const currentQueue = $derived(view.startsWith("queue:") ? view.slice(6) : "Main download queue");

  onMount(() => {
    call<Download[]>("list_downloads").then((list) => (downloads = list));
    call<Settings>("get_settings").then((s) => { settings = s; applyTheme(s.theme); });
    call<string[]>("running_queues").then((q) => (runningQueues = q));
    call<string>("app_version").then((v) => (version = v));
    const subscriptions = [
      listen<Download>("download", ({ payload }) => {
        const i = downloads.findIndex((d) => d.id === payload.id);
        if (i < 0) downloads.push(payload); else downloads[i] = payload;
      }),
      listen<string[]>("removed", ({ payload }) => {
        downloads = downloads.filter((d) => !payload.includes(d.id));
        selected = new Set([...selected].filter((id) => !payload.includes(id)));
      }),
      listen<Settings>("settings", ({ payload }) => { settings = payload; applyTheme(payload.theme); }),
      listen<string[]>("queues", ({ payload }) => (runningQueues = payload)),
      listen<string>("message", ({ payload }) => (message = payload)),
      listen<string>("completed", ({ payload }) => { const d = downloads.find((x) => x.id === payload); if (d) message = "Saved " + d.path; }),
      listen<[string, string]>("failed", ({ payload }) => { const d = downloads.find((x) => x.id === payload[0]); message = (d ? d.name + ": " : "") + payload[1]; }),
      listen("open-add", () => openAdd()),
      listen<[string, AfterQueue]>("queue-finished", ({ payload }) => startCountdown(payload[0], payload[1])),
    ];
    const close = (e: MouseEvent) => { if (!(e.target as HTMLElement).closest(".menubar, .context")) { menu = null; context = null; queueMenu = false; } };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", keys);
    return () => { subscriptions.forEach((s) => s.then((f) => f())); window.removeEventListener("mousedown", close); window.removeEventListener("keydown", keys); };
  });

  function keys(e: KeyboardEvent) {
    const dialog = adding || confirmDelete || about || renaming || countdown;
    if (dialog) { if (e.key === "Escape") { adding = false; confirmDelete = null; about = false; renaming = null; } return; }
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    if (e.key === "Delete" && selected.size) askDelete([...selected]);
    else if (e.key === "Enter" && chosen.length === 1) activate(chosen[0]);
    else if (e.key === "a" && e.ctrlKey) { e.preventDefault(); selected = new Set(visible.map((d) => d.id)); }
    else if (e.key === "n" && e.ctrlKey) { e.preventDefault(); openAdd(); }
    else if (e.key === "Escape") { menu = null; context = null; }
  }

  function select(d: Download, e: MouseEvent) {
    if (e.shiftKey && anchor) {
      const ids = visible.map((x) => x.id);
      const [a, b] = [ids.indexOf(anchor), ids.indexOf(d.id)].sort((x, y) => x - y);
      selected = new Set(ids.slice(a, b + 1));
    } else if (e.ctrlKey) {
      const next = new Set(selected); next.has(d.id) ? next.delete(d.id) : next.add(d.id); selected = next; anchor = d.id;
    } else { selected = new Set([d.id]); anchor = d.id; }
  }
  function rowMenu(d: Download, e: MouseEvent) {
    e.preventDefault();
    if (!selected.has(d.id)) { selected = new Set([d.id]); anchor = d.id; }
    context = { x: Math.min(e.clientX, innerWidth - 230), y: Math.min(e.clientY, innerHeight - 330) };
    queueMenu = false;
  }
  // Double-click: a finished file opens; anything else shows its progress window.
  const activate = (d: Download) => d.status === "Completed" ? openFile(d) : call("show_progress", { id: d.id });

  function openAdd() {
    adding = true; addError = ""; address = ""; menu = null;
    navigator.clipboard?.readText?.().then((t) => {
      t = t.trim();
      if (!/^(https?:\/\/|magnet:\?)\S+$/i.test(t) || address) return;
      address = t;
      requestAnimationFrame(() => (document.getElementById("address") as HTMLInputElement | null)?.select());
    }).catch(() => {});
  }
  async function add(event: SubmitEvent) {
    event.preventDefault();
    const url = address.trim();
    const existing = downloads.find((d) => d.url === url);
    if (existing && existing.status !== "Failed") { addError = `This address is already in the list as ${existing.name}.`; selected = new Set([existing.id]); return; }
    try { await call("offer_url", { url }); adding = false; } catch (error) { addError = String(error); }
  }
  async function openTorrentFile() {
    menu = null;
    const path = await pickFile({ title: "Open torrent file", filters: [{ name: "Torrent files", extensions: ["torrent"] }] });
    if (typeof path === "string") call("open_torrent_file", { path }).catch((e) => (message = String(e)));
  }
  const ids = () => [...selected];
  const resume = () => call("resume_downloads", { ids: ids() });
  const stop = () => call("pause_downloads", { ids: ids() });
  function askDelete(list: string[]) { if (list.length) { confirmDelete = list; deleteFiles = false; menu = null; context = null; } }
  async function doDelete() { if (confirmDelete) await call("remove_downloads", { ids: confirmDelete, deleteFiles }); confirmDelete = null; }
  async function deleteCompleted() { const removed = await call<string[]>("remove_completed"); message = `Removed ${removed.length} completed download${removed.length === 1 ? "" : "s"} from the list.`; menu = null; }
  async function openFile(d: Download) { try { await call("open_file", { id: d.id }); } catch (e) { message = String(e); } }
  async function showFolder(d: Download) { try { await call("show_in_folder", { id: d.id }); } catch (e) { message = String(e); } }
  async function doRename(e: SubmitEvent) { e.preventDefault(); if (!renaming) return; try { await call("rename_download", renaming); renaming = null; } catch (err) { message = String(err); renaming = null; } }
  const showWindow = (name: string) => { call("show_window", { name }); menu = null; };
  async function toggleQueue(name: string) {
    if (runningQueues.includes(name)) { await call("stop_queue", { name }); message = `${name} stopped.`; }
    else { await call("start_queue", { name }); message = `${name} started.`; }
  }
  function startCountdown(queue: string, action: AfterQueue) {
    if (action === "nothing") return;
    countdown = { queue, action, seconds: 60 };
    const timer = setInterval(() => {
      if (!countdown) { clearInterval(timer); return; }
      countdown.seconds -= 1;
      if (countdown.seconds <= 0) { clearInterval(timer); const a = countdown.action; countdown = null; call("power_action", { action: a }); }
    }, 1000);
  }
  const actionName: Record<string, string> = { exit: "Flux will close", sleep: "The computer will sleep", hibernate: "The computer will hibernate", shutdown: "The computer will shut down" };

  const menus: Record<string, { label: string; run?: () => void; disabled?: () => boolean; check?: () => boolean; key?: string; sep?: boolean }[]> = {
    Tasks: [
      { label: "Add new download", key: "Ctrl+N", run: openAdd },
      { label: "Add batch download", run: () => showWindow("batch") },
      { label: "Open torrent file…", run: openTorrentFile },
      { label: "Exit", sep: true, run: () => call("exit_app") },
    ],
    File: [
      { label: "Open", key: "Enter", run: () => chosen[0] && openFile(chosen[0]), disabled: () => chosen.length !== 1 || chosen[0].status !== "Completed" },
      { label: "Open folder", run: () => chosen[0] && showFolder(chosen[0]), disabled: () => chosen.length !== 1 },
      { label: "Show progress", run: () => chosen[0] && call("show_progress", { id: chosen[0].id }), disabled: () => chosen.length !== 1 },
      { label: "Stop download", sep: true, run: stop, disabled: () => !canStop },
      { label: "Download now / Resume", run: resume, disabled: () => !canResume },
      { label: "Redownload", run: () => chosen[0] && call("redownload", { id: chosen[0].id }), disabled: () => chosen.length !== 1 },
      { label: "Rename", run: () => chosen[0] && (renaming = { id: chosen[0].id, name: chosen[0].name }), disabled: () => chosen.length !== 1 || running(chosen[0].status) },
      { label: "Remove", key: "Del", sep: true, run: () => askDelete(ids()), disabled: () => selected.size === 0 },
    ],
    Downloads: [
      { label: "Pause all", run: () => call("pause_all") },
      { label: "Resume all", run: () => call("resume_all") },
      { label: "Delete all completed", run: deleteCompleted },
      { label: "Start main queue", sep: true, run: () => toggleQueue("Main download queue"), disabled: () => runningQueues.includes("Main download queue") },
      { label: "Stop main queue", run: () => toggleQueue("Main download queue"), disabled: () => !runningQueues.includes("Main download queue") },
      { label: "Scheduler", sep: true, run: () => showWindow("scheduler") },
      { label: "Options", run: () => showWindow("options") },
    ],
    View: [
      { label: "Categories", check: () => showCategories, run: () => (showCategories = !showCategories) },
      { label: "Toolbar", check: () => showToolbar, run: () => (showToolbar = !showToolbar) },
      { label: "Dark mode", check: () => settings?.theme !== "light", run: () => { if (settings) call("save_settings", { settings: { ...settings, theme: settings.theme === "light" ? "dark" : "light" } }); } },
    ],
    Help: [
      { label: "Flux on GitHub", run: () => call("open_folder", { path: "https://github.com/AchrafTsouli/flux-downloader" }) },
      { label: "About Flux", run: () => { about = true; menu = null; } },
    ],
  };
</script>

<div class="window">
  <div class="menubar" role="menubar">
    {#each Object.keys(menus) as name}
      <div class="menu-root">
        <button class="menu-title" class:open={menu === name} onclick={() => (menu = menu === name ? null : name)} onmouseenter={() => menu && (menu = name)}>{name}</button>
        {#if menu === name}
          <div class="dropdown" role="menu">
            {#each menus[name] as item}
              {#if item.sep}<div class="sep"></div>{/if}
              <button role="menuitem" class="menu-item" disabled={item.disabled?.()} onclick={() => { item.run?.(); if (name !== "View") menu = null; }}>
                <span class="tick">{item.check?.() ? "✓" : ""}</span>{item.label}<span class="key">{item.key ?? ""}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    <div class="brand"><img src="/logo.png" alt="" width="18" height="18" /><span>FLUX</span></div>
  </div>

  {#if showToolbar}
    <div class="toolbar" role="toolbar">
      <button class="tool" onclick={openAdd}><Icon name="add" /><span>Add URL</span></button>
      <button class="tool" onclick={resume} disabled={!canResume}><Icon name="resume" /><span>Resume</span></button>
      <button class="tool" onclick={stop} disabled={!canStop}><Icon name="stop" /><span>Stop</span></button>
      <button class="tool" onclick={() => call("pause_all")} disabled={active.length === 0 && !downloads.some((d) => d.status === "Queued")}><Icon name="stopAll" /><span>Stop All</span></button>
      <button class="tool" onclick={() => askDelete(ids())} disabled={selected.size === 0}><Icon name="delete" /><span>Delete</span></button>
      <button class="tool" onclick={deleteCompleted} disabled={!downloads.some((d) => d.status === "Completed")}><Icon name="deleteDone" /><span>Delete Co…</span></button>
      <button class="tool" onclick={() => showWindow("options")}><Icon name="options" /><span>Options</span></button>
      <button class="tool" onclick={() => showWindow("scheduler")}><Icon name="scheduler" /><span>Scheduler</span></button>
      <button class="tool" onclick={() => toggleQueue(currentQueue)} disabled={runningQueues.includes(currentQueue)} title={"Start " + currentQueue}><Icon name="startQueue" /><span>Start Qu…</span></button>
      <button class="tool" onclick={() => toggleQueue(currentQueue)} disabled={!runningQueues.includes(currentQueue)} title={"Stop " + currentQueue}><Icon name="stopQueue" /><span>Stop Qu…</span></button>
      <button class="tool" onclick={() => { navigator.clipboard?.writeText("Flux, a free download manager: https://github.com/AchrafTsouli/flux-downloader"); message = "Link to Flux copied. Paste it to a friend."; }}><Icon name="share" /><span>Tell a Friend</span></button>
    </div>
  {/if}

  <div class="main" style:grid-template-columns={showCategories ? "220px 1fr" : "1fr"}>
    {#if showCategories}
      <nav class="tree" aria-label="Categories">
        <div class="pane-title">Categories <button class="pane-close" onclick={() => (showCategories = false)} aria-label="Hide categories">×</button></div>
        <button class="node" class:on={view === "all"} onclick={() => (view = "all")}><span class="twisty">▾</span><Icon name="folder" size={16} />All Downloads<small>{count("all") || ""}</small></button>
        {#each categories as type}
          <button class="node child" class:on={view === "type:" + type} onclick={() => (view = "type:" + type)}><Icon name={type} size={15} />{type}<small>{count("type:" + type) || ""}</small></button>
        {/each}
        <button class="node" class:on={view === "unfinished"} onclick={() => (view = "unfinished")}><span class="twisty"></span><Icon name="folder" size={16} />Unfinished<small>{count("unfinished") || ""}</small></button>
        <button class="node" class:on={view === "failed"} onclick={() => (view = "failed")}><span class="twisty"></span><Icon name="failed" size={16} />Failed<small>{count("failed") || ""}</small></button>
        <button class="node" class:on={view === "finished"} onclick={() => (view = "finished")}><span class="twisty"></span><Icon name="folder" size={16} />Finished<small>{count("finished") || ""}</small></button>
        <div class="node static"><span class="twisty">▾</span><Icon name="folder" size={16} />Queues</div>
        {#each queues as q}
          <button class="node child" class:on={view === "queue:" + q.name} onclick={() => (view = "queue:" + q.name)} ondblclick={() => showWindow("scheduler")} title={runningQueues.includes(q.name) ? "Running" : "Stopped"}>
            <Icon name="queue" size={15} />{q.name}{#if runningQueues.includes(q.name)}<span class="live" aria-label="running"></span>{/if}<small>{count("queue:" + q.name) || ""}</small>
          </button>
        {/each}
      </nav>
    {/if}

    <section class="listpane">
      <div class="list-head">
        <h1>{viewTitle}</h1>
        <input class="search" type="search" placeholder="Search downloads…" bind:value={search} aria-label="Search downloads" />
      </div>
      <div class="grid-wrap">
        <table class="grid">
          <colgroup><col /><col style="width:26px" /><col style="width:84px" /><col style="width:96px" /><col style="width:80px" /><col style="width:96px" /><col style="width:100px" /><col style="width:150px" /><col style="width:150px" /></colgroup>
          <thead><tr><th>File Name</th><th class="c">Q</th><th class="r">Size</th><th>Status</th><th class="r">Time left</th><th class="r">Transfer rate</th><th>Last Try…</th><th>Description</th><th>Progress</th></tr></thead>
          <tbody>
            {#each visible as d (d.id)}
              <tr class:sel={selected.has(d.id)} onmousedown={(e) => e.button === 0 && select(d, e)} ondblclick={() => activate(d)} oncontextmenu={(e) => rowMenu(d, e)} title={d.error ?? d.url}>
                <td class="name"><Icon name={d.category} size={15} /><span>{d.name}</span></td>
                <td class="c">{d.status === "Queued" ? "⏱" : ""}</td>
                <td class="r">{size(d.total)}</td>
                <td><span class="status {statusClass(d)}">{statusText(d)}</span></td>
                <td class="r">{timeLeft(d)}</td>
                <td class="r">{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : "—"}</td>
                <td>{stamp(d.lastTryMs)}</td>
                <td class="desc">{d.description}</td>
                <td><div class="progress"><div class="bar"><i style:width="{percent(d)}%"></i></div><span>{Math.floor(percent(d))}%</span></div></td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if visible.length === 0}
          <div class="empty">{downloads.length === 0 ? "No downloads yet. Click Add URL, or press Ctrl+N, and paste a link." : "Nothing in this view."}</div>
        {/if}
      </div>
    </section>
  </div>

  <footer class="statusbar">
    <span class="msg">{message}</span>
    <span class="totals">Total: <b>{downloads.length}</b>&nbsp;&nbsp; Active: <b>{active.length}</b>{totalSpeed > 0 ? ` · ${size(totalSpeed)}/s` : ""}&nbsp;&nbsp; Finished: <b>{downloads.filter((d) => d.status === "Completed").length}</b></span>
  </footer>
</div>

{#if context}
  <div class="context dropdown" style:left="{context.x}px" style:top="{context.y}px" role="menu">
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status !== "Completed"} onclick={() => { openFile(chosen[0]); context = null; }}><span class="tick"></span>Open</button>
    <button class="menu-item" disabled={chosen.length !== 1} onclick={() => { showFolder(chosen[0]); context = null; }}><span class="tick"></span>Open folder</button>
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status === "Completed"} onclick={() => { call("show_progress", { id: chosen[0].id }); context = null; }}><span class="tick"></span>Show progress</button>
    <div class="sep"></div>
    <button class="menu-item" disabled={!canResume} onclick={() => { resume(); context = null; }}><span class="tick"></span>Resume</button>
    <button class="menu-item" disabled={!canStop} onclick={() => { stop(); context = null; }}><span class="tick"></span>Stop</button>
    <button class="menu-item" disabled={!canResume} onclick={() => { call("enqueue", { ids: ids() }); context = null; }}><span class="tick"></span>Download later (queue)</button>
    <button class="menu-item submenu" onmouseenter={() => (queueMenu = true)} onclick={() => (queueMenu = !queueMenu)}><span class="tick"></span>Move to queue<span class="key">▸</span></button>
    {#if queueMenu}
      <div class="dropdown sub" role="menu">
        {#each queues as q}<button class="menu-item" onclick={() => { call("set_queue", { ids: ids(), queue: q.name }); context = null; }}><span class="tick">{chosen.every((d) => d.queue === q.name) ? "✓" : ""}</span>{q.name}</button>{/each}
      </div>
    {/if}
    <div class="sep"></div>
    <button class="menu-item" disabled={chosen.length !== 1 || running(chosen[0].status)} onclick={() => { renaming = { id: chosen[0].id, name: chosen[0].name }; context = null; }}><span class="tick"></span>Rename</button>
    <button class="menu-item" disabled={chosen.length !== 1} onclick={() => { call("redownload", { id: chosen[0].id }); context = null; }}><span class="tick"></span>Redownload</button>
    <button class="menu-item" onclick={() => { navigator.clipboard?.writeText(chosen.map((d) => d.url).join("\n")); context = null; message = "Address copied."; }}><span class="tick"></span>Copy address</button>
    <button class="menu-item danger" onclick={() => askDelete(ids())}><span class="tick"></span>Delete<span class="key">Del</span></button>
  </div>
{/if}

{#if adding}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (adding = false)}>
    <form class="dialog" onsubmit={add} aria-labelledby="add-title">
      <div class="dialog-title" id="add-title">Enter new address to download</div>
      <div class="dialog-body">
        <div class="field"><label for="address">Address</label>
          <!-- svelte-ignore a11y_autofocus -->
          <input id="address" bind:value={address} autofocus autocomplete="off" spellcheck="false" placeholder="https://… or magnet:?…" />
        </div>
        {#if addError}<p class="error" role="alert">{addError}</p>{/if}
      </div>
      <div class="dialog-actions">
        <button type="submit" class="btn primary" disabled={!address.trim()}>OK</button>
        <button type="button" class="btn" onclick={() => (adding = false)}>Cancel</button>
      </div>
    </form>
  </div>
{/if}

{#if renaming}
  <div class="scrim" role="presentation">
    <form class="dialog small" onsubmit={doRename} aria-labelledby="ren-title">
      <div class="dialog-title" id="ren-title">Rename</div>
      <div class="dialog-body"><div class="field"><label for="newname">Name</label>
        <!-- svelte-ignore a11y_autofocus -->
        <input id="newname" bind:value={renaming.name} autofocus spellcheck="false" /></div></div>
      <div class="dialog-actions"><button type="submit" class="btn primary">Rename</button><button type="button" class="btn" onclick={() => (renaming = null)}>Cancel</button></div>
    </form>
  </div>
{/if}

{#if confirmDelete}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="alertdialog" aria-labelledby="del-title">
      <div class="dialog-title" id="del-title">Delete {confirmDelete.length === 1 ? "this download" : `${confirmDelete.length} downloads`}?</div>
      <div class="dialog-body">
        <p>{confirmDelete.length === 1 ? downloads.find((d) => d.id === confirmDelete![0])?.name : "The selected downloads"} will be removed from the list.</p>
        <label class="checkbox"><input type="checkbox" bind:checked={deleteFiles} /> Also delete the files from disk</label>
      </div>
      <div class="dialog-actions"><button class="btn danger" onclick={doDelete}>Delete</button><button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button></div>
    </div>
  </div>
{/if}

{#if countdown}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="alertdialog" aria-labelledby="cd-title">
      <div class="dialog-title" id="cd-title">{countdown.queue} has finished</div>
      <div class="dialog-body"><p>{actionName[countdown.action]} in <b>{countdown.seconds}</b> seconds.</p></div>
      <div class="dialog-actions">
        <button class="btn primary" onclick={() => { const a = countdown!.action; countdown = null; call("power_action", { action: a }); }}>Now</button>
        <button class="btn" onclick={() => (countdown = null)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if about}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (about = false)}>
    <div class="dialog small" role="dialog" aria-labelledby="about-title">
      <div class="dialog-title" id="about-title">About Flux</div>
      <div class="dialog-body about"><img src="/logo.png" alt="" width="48" height="48" /><div><b>Flux {version}</b><p>Rust engine · Tauri interface · MIT licence</p><p class="muted">github.com/AchrafTsouli/flux-downloader</p></div></div>
      <div class="dialog-actions"><button class="btn primary" onclick={() => (about = false)}>OK</button></div>
    </div>
  </div>
{/if}

<style>
  .window { display: grid; grid-template-rows: auto auto 1fr auto; height: 100%; }
  .menubar { display: flex; align-items: center; height: 26px; padding: 0 6px; background: var(--chrome); }
  .menu-root { position: relative; }
  .menu-title { height: 22px; padding: 0 9px; border: 0; border-radius: 3px; background: transparent; cursor: default; }
  .menu-title:hover, .menu-title.open { background: var(--raised); }
  .dropdown { position: absolute; top: 24px; left: 0; z-index: 30; min-width: 230px; padding: 4px; background: var(--menu); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 10px 28px rgb(0 0 0 / 0.35); display: grid; }
  .dropdown.sub { top: auto; left: 100%; margin-top: -60px; }
  .menu-item { display: grid; grid-template-columns: 18px 1fr auto; align-items: center; gap: 4px; height: 26px; padding: 0 10px 0 4px; border: 0; border-radius: 4px; background: transparent; text-align: left; cursor: default; }
  .menu-item:hover:not(:disabled) { background: var(--selection); color: var(--selection-text); }
  .menu-item:disabled { color: var(--muted); opacity: 0.55; }
  .menu-item.danger:hover { background: color-mix(in srgb, var(--bad) 28%, transparent); }
  .tick { text-align: center; color: var(--accent); }
  .key { color: var(--muted); font-size: 11px; padding-left: 18px; }
  .sep { height: 1px; margin: 4px 6px; background: var(--line); }
  .brand { margin-left: auto; margin-right: 6px; display: flex; align-items: center; gap: 6px; font-weight: 700; letter-spacing: 0.06em; color: var(--accent); }
  .brand img { border-radius: 4px; }
  .toolbar { display: flex; gap: 2px; padding: 4px 8px 5px; background: var(--chrome); border-bottom: 1px solid var(--line); overflow: hidden; }
  .tool { display: grid; justify-items: center; align-content: center; gap: 3px; width: 70px; height: 62px; padding: 4px 2px; border: 1px solid transparent; border-radius: 6px; background: transparent; color: var(--icon); cursor: default; flex: none; }
  .tool span { font-size: 11px; color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
  .tool:hover:not(:disabled) { background: var(--raised); border-color: var(--line); }
  .tool:active:not(:disabled) { background: var(--selection); }
  .tool:disabled { opacity: 0.35; }
  .main { display: grid; min-height: 0; }
  .tree { display: flex; flex-direction: column; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; padding-bottom: 8px; }
  .pane-title { display: flex; justify-content: space-between; align-items: center; height: 24px; padding: 0 8px; font-weight: 600; color: var(--muted); border-bottom: 1px solid var(--line); }
  .pane-close { border: 0; background: none; color: var(--muted); font-size: 14px; line-height: 1; padding: 0 2px; }
  .node { display: flex; align-items: center; gap: 6px; height: 21px; padding: 0 8px 0 6px; border: 0; background: transparent; text-align: left; white-space: nowrap; cursor: default; color: var(--text); flex: none; }
  .node.child { padding-left: 34px; }
  .node:not(.static):hover { background: var(--raised); }
  .node.on { background: var(--selection); color: var(--selection-text); }
  .node small { margin-left: auto; color: var(--muted); font-size: 10.5px; }
  .node.on small { color: inherit; }
  .twisty { width: 10px; font-size: 9px; color: var(--muted); }
  .live { width: 7px; height: 7px; border-radius: 50%; background: var(--good); }
  .listpane { display: grid; grid-template-rows: auto 1fr; min-width: 0; min-height: 0; background: var(--bg); }
  .list-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; height: 30px; padding: 0 8px 0 10px; background: var(--surface); border-bottom: 1px solid var(--line); }
  .list-head h1 { margin: 0; font-size: 12px; font-weight: 600; }
  .search { width: 200px; height: 22px; padding: 0 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .grid-wrap { overflow: auto; position: relative; }
  .grid { width: 100%; min-width: 900px; border-collapse: collapse; table-layout: fixed; font-variant-numeric: tabular-nums; }
  .grid th { position: sticky; top: 0; z-index: 1; height: 24px; padding: 0 8px; text-align: left; font-weight: 400; color: var(--muted); background: var(--surface); border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .grid td { height: 21px; padding: 0 8px; border-right: 1px solid color-mix(in srgb, var(--line) 55%, transparent); border-bottom: 1px solid color-mix(in srgb, var(--line) 55%, transparent); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .grid tbody tr:nth-child(even) { background: var(--row-alt); }
  .grid tbody tr:hover { background: color-mix(in srgb, var(--selection) 45%, transparent); }
  .grid tbody tr.sel { background: var(--selection); color: var(--selection-text); }
  .grid .r { text-align: right; }
  .grid .c { text-align: center; color: var(--accent); }
  .name { display: flex; align-items: center; gap: 5px; }
  .name span { overflow: hidden; text-overflow: ellipsis; }
  .desc { color: var(--muted); }
  tr.sel .desc { color: inherit; }
  .status.good { color: var(--good); } .status.bad { color: var(--bad); } .status.warn { color: var(--warn); } .status.run { color: var(--accent); }
  .progress { display: flex; align-items: center; gap: 6px; }
  .bar { flex: 1; height: 5px; border-radius: 3px; background: var(--raised); overflow: hidden; }
  .bar i { display: block; height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); transition: width 0.25s linear; }
  .progress span { width: 30px; text-align: right; font-size: 10.5px; color: var(--muted); }
  .empty { position: absolute; inset: 40px 0 auto; text-align: center; color: var(--muted); }
  .statusbar { display: flex; justify-content: space-between; gap: 16px; height: 24px; align-items: center; padding: 0 10px; background: var(--chrome); border-top: 1px solid var(--line); color: var(--muted); }
  .msg { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .totals { white-space: nowrap; }
  .totals b { color: var(--text); font-weight: 600; }
  .context { position: fixed; top: 0; left: 0; }
  .submenu { position: relative; }
  .scrim { position: fixed; inset: 0; z-index: 40; display: grid; place-items: center; background: rgb(0 0 0 / 0.35); }
  .dialog { width: min(540px, 92vw); background: var(--surface); border: 1px solid var(--line); border-radius: 8px; box-shadow: 0 18px 50px rgb(0 0 0 / 0.45); overflow: hidden; }
  .dialog.small { width: min(420px, 92vw); }
  .dialog-title { padding: 10px 14px; font-weight: 600; background: var(--chrome); border-bottom: 1px solid var(--line); }
  .dialog-body { padding: 14px; display: grid; gap: 10px; }
  .dialog-body p { margin: 0; }
  .field { display: grid; grid-template-columns: 64px 1fr; align-items: center; gap: 8px; }
  .field label { color: var(--muted); }
  .field input { height: 28px; padding: 0 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .field input:focus { border-color: var(--accent); outline: none; }
  .error { color: var(--bad); }
  .checkbox { display: flex; align-items: center; gap: 6px; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; padding: 0 14px 14px; }
  .about { grid-template-columns: auto 1fr; align-items: center; gap: 14px; }
  .about img { border-radius: 10px; }
  .muted { color: var(--muted); }
</style>
