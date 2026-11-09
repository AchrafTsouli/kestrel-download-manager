<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import Icon from "./lib/Icon.svelte";

  type Download = {
    id: string; name: string; url: string; path: string; category: string; description: string; status: string;
    written: number; total: number | null; bytesPerSecond: number; connections: number; addedMs: number; lastTryMs: number | null; error: string | null;
  };

  const fileTypes = ["Compressed", "Documents", "Music", "Programs", "Video", "Images", "Other"];
  const finishedStates = ["Completed"];
  const idleStates = ["Completed", "Paused", "Failed"];

  let downloads: Download[] = $state([]);
  let selected: Set<string> = $state(new Set());
  let anchor: string | null = $state(null);
  let view = $state("all");
  let search = $state("");
  let menu: string | null = $state(null);
  let context: { x: number; y: number } | null = $state(null);
  let adding = $state(false);
  let address = $state("");
  let addError = $state("");
  let confirmDelete: string[] | null = $state(null);
  let deleteFiles = $state(false);
  let about = $state(false);
  let showCategories = $state(true);
  let showToolbar = $state(true);
  let theme = $state(localStorageGet("flux-theme") ?? "dark");
  let message = $state("Ready.");

  function localStorageGet(key: string) { try { return localStorage.getItem(key); } catch { return null; } }
  $effect(() => { document.documentElement.dataset.theme = theme; try { localStorage.setItem("flux-theme", theme); } catch {} });

  const running = (d: Download) => !idleStates.includes(d.status);
  function inView(d: Download, key: string) {
    if (key === "all") return true;
    if (key === "unfinished") return !finishedStates.includes(d.status);
    if (key === "finished") return finishedStates.includes(d.status);
    if (key === "failed") return d.status === "Failed";
    if (key.startsWith("type:")) return d.category === key.slice(5);
    return false;
  }
  const visible = $derived(downloads.filter((d) => inView(d, view) && (!search || (d.name + " " + d.description).toLowerCase().includes(search.toLowerCase()))));
  const chosen = $derived(downloads.filter((d) => selected.has(d.id)));
  const canResume = $derived(chosen.some((d) => ["Paused", "Failed"].includes(d.status)));
  const canStop = $derived(chosen.some(running));
  const active = $derived(downloads.filter(running));
  const totalSpeed = $derived(downloads.reduce((s, d) => s + d.bytesPerSecond, 0));
  const viewTitle = $derived(view === "all" ? "All Downloads" : view === "unfinished" ? "Unfinished" : view === "finished" ? "Finished" : view === "failed" ? "Failed" : view.slice(5));
  const count = (key: string) => downloads.filter((d) => inView(d, key)).length;

  onMount(() => {
    invoke<Download[]>("list_downloads").then((list) => (downloads = list));
    const unlisten = [
      listen<Download>("download", ({ payload }) => {
        const i = downloads.findIndex((d) => d.id === payload.id);
        if (i < 0) downloads.push(payload); else downloads[i] = payload;
        if (payload.status === "Completed" && i >= 0) message = "Saved " + payload.path;
        if (payload.status === "Failed" && payload.error) message = payload.name + ": " + payload.error;
      }),
      listen<string[]>("removed", ({ payload }) => {
        downloads = downloads.filter((d) => !payload.includes(d.id));
        selected = new Set([...selected].filter((id) => !payload.includes(id)));
      }),
    ];
    const close = (e: MouseEvent) => { if (!(e.target as HTMLElement).closest(".menubar, .context")) { menu = null; context = null; } };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", keys);
    return () => { unlisten.forEach((u) => u.then((f) => f())); window.removeEventListener("mousedown", close); window.removeEventListener("keydown", keys); };
  });

  function keys(e: KeyboardEvent) {
    if (adding || confirmDelete || about) { if (e.key === "Escape") { adding = false; confirmDelete = null; about = false; } return; }
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    if (e.key === "Delete" && selected.size) askDelete([...selected]);
    else if (e.key === "Enter" && chosen.length === 1) openFile(chosen[0]);
    else if (e.key === "a" && e.ctrlKey) { e.preventDefault(); selected = new Set(visible.map((d) => d.id)); }
    else if (e.key === "n" && e.ctrlKey) { e.preventDefault(); openAdd(); }
    else if (e.key === "Escape") { menu = null; context = null; }
  }

  function size(bytes: number | null) {
    if (bytes == null) return "";
    if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(2) + " GB";
    if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " MB";
    if (bytes >= 1024) return Math.round(bytes / 1024) + " KB";
    return bytes + " B";
  }
  const percent = (d: Download) => (d.total ? Math.min(100, (d.written / d.total) * 100) : d.status === "Completed" ? 100 : 0);
  function timeLeft(d: Download) {
    if (!d.total || d.bytesPerSecond <= 0 || !running(d)) return "—";
    const s = Math.round((d.total - d.written) / d.bytesPerSecond);
    const two = (n: number) => String(n).padStart(2, "0");
    return `${two(Math.floor(s / 3600))}:${two(Math.floor((s % 3600) / 60))}:${two(s % 60)}`;
  }
  const lastTry = (ms: number | null) => ms ? new Date(ms).toLocaleString(undefined, { month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }) : "";
  const statusText = (d: Download) => d.status === "Completed" ? "Complete" : d.status === "Downloading" ? `${percent(d).toFixed(2)}%` : d.status;
  const statusClass = (s: string) => s === "Completed" ? "good" : s === "Failed" ? "bad" : s === "Paused" ? "warn" : "run";

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
    context = { x: Math.min(e.clientX, innerWidth - 210), y: Math.min(e.clientY, innerHeight - 230) };
  }

  function openAdd() {
    adding = true; addError = ""; address = ""; menu = null;
    // Like IDM: a link on the clipboard is offered, selected, so typing replaces it.
    navigator.clipboard?.readText?.().then((t) => {
      t = t.trim();
      if (!/^https?:\/\/\S+$/i.test(t) || address) return;
      address = t;
      requestAnimationFrame(() => (document.getElementById("address") as HTMLInputElement | null)?.select());
    }).catch(() => {});
  }
  async function add(event: SubmitEvent) {
    event.preventDefault();
    const url = address.trim();
    const existing = downloads.find((d) => d.url === url || d.url === url + "/" );
    if (existing && existing.status !== "Failed") {
      addError = `This address is already in the list as ${existing.name}.`;
      selected = new Set([existing.id]); anchor = existing.id;
      return;
    }
    try { const id = await invoke<string>("add_download", { url }); selected = new Set([id]); anchor = id; adding = false; }
    catch (error) { addError = String(error); }
  }
  const ids = () => [...selected];
  const resume = () => invoke("resume_downloads", { ids: ids() });
  const stop = () => invoke("pause_downloads", { ids: ids() });
  const stopAll = () => invoke("pause_all");
  const resumeAll = () => invoke("resume_downloads", { ids: downloads.filter((d) => ["Paused", "Failed"].includes(d.status)).map((d) => d.id) });
  function askDelete(list: string[]) { if (list.length) { confirmDelete = list; deleteFiles = false; menu = null; context = null; } }
  async function doDelete() { if (confirmDelete) await invoke("remove_downloads", { ids: confirmDelete, deleteFiles }); confirmDelete = null; }
  async function deleteCompleted() { const removed = await invoke<string[]>("remove_completed"); message = `Removed ${removed.length} completed download${removed.length === 1 ? "" : "s"} from the list.`; menu = null; }
  async function openFile(d: Download) { if (d.status !== "Completed") return; try { await invoke("open_file", { id: d.id }); } catch (e) { message = String(e); } }
  async function showFolder(d: Download) { try { await invoke("show_in_folder", { id: d.id }); } catch (e) { message = String(e); } }
  const soon = (what: string) => { message = `${what} arrives in a later Flux 2.0 build.`; menu = null; };

  const menus: Record<string, { label: string; run?: () => void; disabled?: () => boolean; check?: () => boolean; key?: string }[]> = {
    Tasks: [
      { label: "Add new download", key: "Ctrl+N", run: openAdd },
      { label: "Add batch download", run: () => soon("Batch download") },
      { label: "Exit", run: () => window.close() },
    ],
    File: [
      { label: "Open", key: "Enter", run: () => chosen[0] && openFile(chosen[0]), disabled: () => chosen.length !== 1 || chosen[0].status !== "Completed" },
      { label: "Open folder", run: () => chosen[0] && showFolder(chosen[0]), disabled: () => chosen.length !== 1 },
      { label: "Stop download", run: stop, disabled: () => !canStop },
      { label: "Download now / Resume", run: resume, disabled: () => !canResume },
      { label: "Remove", key: "Del", run: () => askDelete(ids()), disabled: () => selected.size === 0 },
    ],
    Downloads: [
      { label: "Pause all", run: stopAll },
      { label: "Resume all", run: resumeAll },
      { label: "Delete all completed", run: deleteCompleted },
      { label: "Scheduler", run: () => soon("The scheduler") },
      { label: "Options", run: () => soon("Options") },
    ],
    View: [
      { label: "Categories", check: () => showCategories, run: () => (showCategories = !showCategories) },
      { label: "Toolbar", check: () => showToolbar, run: () => (showToolbar = !showToolbar) },
      { label: "Dark mode", check: () => theme === "dark", run: () => (theme = theme === "dark" ? "light" : "dark") },
    ],
    Help: [
      { label: "Check for updates", run: () => soon("Updates") },
      { label: "About Flux", run: () => { about = true; menu = null; } },
    ],
  };
</script>

<div class="window">
  <!-- Menu bar -->
  <div class="menubar" role="menubar">
    {#each Object.keys(menus) as name}
      <div class="menu-root">
        <button class="menu-title" class:open={menu === name} onclick={() => (menu = menu === name ? null : name)} onmouseenter={() => menu && (menu = name)}>{name}</button>
        {#if menu === name}
          <div class="dropdown" role="menu">
            {#each menus[name] as item}
              <button role="menuitem" class="menu-item" disabled={item.disabled?.()} onclick={() => { item.run?.(); if (name !== "View") menu = null; }}>
                <span class="check">{item.check?.() ? "✓" : ""}</span>{item.label}<span class="key">{item.key ?? ""}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    <div class="brand"><img src="/logo.png" alt="" width="18" height="18" /><span>FLUX</span></div>
  </div>

  <!-- Toolbar -->
  {#if showToolbar}
    <div class="toolbar" role="toolbar">
      <button class="tool" onclick={openAdd}><Icon name="add" /><span>Add URL</span></button>
      <button class="tool" onclick={resume} disabled={!canResume}><Icon name="resume" /><span>Resume</span></button>
      <button class="tool" onclick={stop} disabled={!canStop}><Icon name="stop" /><span>Stop</span></button>
      <button class="tool" onclick={stopAll} disabled={active.length === 0}><Icon name="stopAll" /><span>Stop All</span></button>
      <button class="tool" onclick={() => askDelete(ids())} disabled={selected.size === 0}><Icon name="delete" /><span>Delete</span></button>
      <button class="tool" onclick={deleteCompleted} disabled={!downloads.some((d) => d.status === "Completed")}><Icon name="deleteDone" /><span>Delete Co…</span></button>
      <button class="tool" onclick={() => soon("Options")}><Icon name="options" /><span>Options</span></button>
      <button class="tool" onclick={() => soon("The scheduler")}><Icon name="scheduler" /><span>Scheduler</span></button>
      <button class="tool" onclick={() => soon("Queues")}><Icon name="startQueue" /><span>Start Qu…</span></button>
      <button class="tool" onclick={() => soon("Queues")}><Icon name="stopQueue" /><span>Stop Qu…</span></button>
      <button class="tool" onclick={() => soon("Sharing")}><Icon name="share" /><span>Tell a Friend</span></button>
    </div>
  {/if}

  <div class="main" style:grid-template-columns={showCategories ? "220px 1fr" : "1fr"}>
    <!-- Category tree -->
    {#if showCategories}
      <nav class="tree" aria-label="Categories">
        <div class="pane-title">Categories <button class="pane-close" onclick={() => (showCategories = false)} aria-label="Hide categories">×</button></div>
        <button class="node" class:on={view === "all"} onclick={() => (view = "all")}><span class="twisty">▾</span><Icon name="folder" size={16} />All Downloads<small>{count("all")}</small></button>
        {#each fileTypes as type}
          <button class="node child" class:on={view === "type:" + type} onclick={() => (view = "type:" + type)}><Icon name={type} size={15} />{type}<small>{count("type:" + type) || ""}</small></button>
        {/each}
        <button class="node" class:on={view === "unfinished"} onclick={() => (view = "unfinished")}><span class="twisty"></span><Icon name="folder" size={16} />Unfinished<small>{count("unfinished") || ""}</small></button>
        <button class="node" class:on={view === "failed"} onclick={() => (view = "failed")}><span class="twisty"></span><Icon name="failed" size={16} />Failed<small>{count("failed") || ""}</small></button>
        <button class="node" class:on={view === "finished"} onclick={() => (view = "finished")}><span class="twisty"></span><Icon name="folder" size={16} />Finished<small>{count("finished") || ""}</small></button>
        <div class="node static"><span class="twisty">▾</span><Icon name="folder" size={16} />Queues</div>
        <div class="node child static"><Icon name="queue" size={15} />Main download queue</div>
        <div class="node child static"><Icon name="queue" size={15} />Synchronization queue</div>
      </nav>
    {/if}

    <!-- Download list -->
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
              <tr class:sel={selected.has(d.id)} onmousedown={(e) => e.button === 0 && select(d, e)} ondblclick={() => openFile(d)} oncontextmenu={(e) => rowMenu(d, e)} title={d.error ?? d.url}>
                <td class="name"><Icon name={d.category} size={15} /><span>{d.name}</span></td>
                <td class="c"></td>
                <td class="r">{size(d.total)}</td>
                <td><span class="status {statusClass(d.status)}">{statusText(d)}</span></td>
                <td class="r">{timeLeft(d)}</td>
                <td class="r">{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : "—"}</td>
                <td>{lastTry(d.lastTryMs)}</td>
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

  <!-- Status bar -->
  <footer class="statusbar">
    <span class="msg">{message}</span>
    <span class="totals">Total: <b>{downloads.length}</b>&nbsp;&nbsp; Active: <b>{active.length}</b>{totalSpeed > 0 ? ` · ${size(totalSpeed)}/s` : ""}&nbsp;&nbsp; Finished: <b>{downloads.filter((d) => d.status === "Completed").length}</b></span>
  </footer>
</div>

<!-- Row context menu -->
{#if context}
  <div class="context dropdown" style:left="{context.x}px" style:top="{context.y}px" role="menu">
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status !== "Completed"} onclick={() => { openFile(chosen[0]); context = null; }}><span class="check"></span>Open</button>
    <button class="menu-item" disabled={chosen.length !== 1} onclick={() => { showFolder(chosen[0]); context = null; }}><span class="check"></span>Open folder</button>
    <div class="sep"></div>
    <button class="menu-item" disabled={!canResume} onclick={() => { resume(); context = null; }}><span class="check"></span>Resume</button>
    <button class="menu-item" disabled={!canStop} onclick={() => { stop(); context = null; }}><span class="check"></span>Stop</button>
    <div class="sep"></div>
    <button class="menu-item" onclick={() => { navigator.clipboard?.writeText(chosen.map((d) => d.url).join("\n")); context = null; message = "Address copied."; }}><span class="check"></span>Copy address</button>
    <button class="menu-item danger" onclick={() => askDelete(ids())}><span class="check"></span>Delete<span class="key">Del</span></button>
  </div>
{/if}

<!-- Add URL dialog -->
{#if adding}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (adding = false)}>
    <form class="dialog" onsubmit={add} aria-labelledby="add-title">
      <div class="dialog-title" id="add-title">Enter new address to download</div>
      <div class="dialog-body">
        <div class="field"><label for="address">Address</label>
          <!-- svelte-ignore a11y_autofocus -->
          <input id="address" bind:value={address} autofocus autocomplete="off" spellcheck="false" placeholder="https://" />
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

<!-- Delete confirmation -->
{#if confirmDelete}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="alertdialog" aria-labelledby="del-title">
      <div class="dialog-title" id="del-title">Delete {confirmDelete.length === 1 ? "this download" : `${confirmDelete.length} downloads`}?</div>
      <div class="dialog-body">
        <p>{confirmDelete.length === 1 ? downloads.find((d) => d.id === confirmDelete![0])?.name : "The selected downloads"} will be removed from the list.</p>
        <label class="checkbox"><input type="checkbox" bind:checked={deleteFiles} /> Also delete the files from disk</label>
      </div>
      <div class="dialog-actions">
        <button class="btn danger" onclick={doDelete}>Delete</button>
        <button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if about}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (about = false)}>
    <div class="dialog small" role="dialog" aria-labelledby="about-title">
      <div class="dialog-title" id="about-title">About Flux</div>
      <div class="dialog-body about"><img src="/logo.png" alt="" width="48" height="48" /><div><b>Flux 2.0 (preview)</b><p>Rust engine · Tauri interface · MIT licence</p><p class="muted">github.com/AchrafTsouli/flux-downloader</p></div></div>
      <div class="dialog-actions"><button class="btn primary" onclick={() => (about = false)}>OK</button></div>
    </div>
  </div>
{/if}

<style>
  .window { display: grid; grid-template-rows: auto auto 1fr auto; height: 100%; }

  /* Menu bar */
  .menubar { display: flex; align-items: center; height: 26px; padding: 0 6px; background: var(--chrome); }
  .menu-root { position: relative; }
  .menu-title { height: 22px; padding: 0 9px; border: 0; border-radius: 3px; background: transparent; cursor: default; }
  .menu-title:hover, .menu-title.open { background: var(--raised); }
  .dropdown { position: absolute; top: 24px; left: 0; z-index: 30; min-width: 220px; padding: 4px; background: var(--menu); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 10px 28px rgb(0 0 0 / 0.35); display: grid; }
  .menu-item { display: grid; grid-template-columns: 18px 1fr auto; align-items: center; gap: 4px; height: 26px; padding: 0 10px 0 4px; border: 0; border-radius: 4px; background: transparent; text-align: left; cursor: default; }
  .menu-item:hover:not(:disabled) { background: var(--selection); color: var(--selection-text); }
  .menu-item:disabled { color: var(--muted); opacity: 0.55; }
  .menu-item.danger:hover { background: color-mix(in srgb, var(--bad) 28%, transparent); }
  .check { text-align: center; color: var(--accent); }
  .key { color: var(--muted); font-size: 11px; padding-left: 18px; }
  .sep { height: 1px; margin: 4px 6px; background: var(--line); }
  .brand { margin-left: auto; margin-right: 6px; display: flex; align-items: center; gap: 6px; font-weight: 700; letter-spacing: 0.06em; color: var(--accent); font-size: 12px; }
  .brand img { border-radius: 4px; }

  /* Toolbar */
  .toolbar { display: flex; gap: 2px; padding: 4px 8px 5px; background: var(--chrome); border-bottom: 1px solid var(--line); }
  .tool { display: grid; justify-items: center; align-content: center; gap: 3px; width: 70px; height: 62px; padding: 4px 2px; border: 1px solid transparent; border-radius: 6px; background: transparent; color: var(--icon); cursor: default; }
  .tool span { font-size: 11px; color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
  .tool:hover:not(:disabled) { background: var(--raised); border-color: var(--line); }
  .tool:active:not(:disabled) { background: var(--selection); }
  .tool:disabled { opacity: 0.35; }

  .main { display: grid; min-height: 0; }

  /* Category tree */
  .tree { display: flex; flex-direction: column; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; padding-bottom: 8px; }
  .pane-title { display: flex; justify-content: space-between; align-items: center; height: 24px; padding: 0 8px; font-weight: 600; color: var(--muted); border-bottom: 1px solid var(--line); }
  .pane-close { border: 0; background: none; color: var(--muted); font-size: 14px; line-height: 1; padding: 0 2px; }
  .node { display: flex; align-items: center; gap: 6px; height: 21px; padding: 0 8px 0 6px; border: 0; background: transparent; text-align: left; white-space: nowrap; cursor: default; color: var(--text); }
  .node.child { padding-left: 34px; }
  .node:not(.static):hover { background: var(--raised); }
  .node.on { background: var(--selection); color: var(--selection-text); }
  .node small { margin-left: auto; color: var(--muted); font-size: 10.5px; }
  .node.on small { color: inherit; }
  .twisty { width: 10px; font-size: 9px; color: var(--muted); }

  /* List */
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
  .grid .c { text-align: center; }
  .name { display: flex; align-items: center; gap: 5px; }
  .name span { overflow: hidden; text-overflow: ellipsis; }
  .desc { color: var(--muted); }
  tr.sel .desc { color: inherit; }
  .status.good { color: var(--good); } .status.bad { color: var(--bad); } .status.warn { color: var(--warn); } .status.run { color: var(--accent); }
  tr.sel .status { filter: brightness(1.15); }
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

  /* Dialogs */
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
  .checkbox { display: flex; align-items: center; gap: 6px; color: var(--text); }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; padding: 0 14px 14px; }
  .btn { min-width: 84px; height: 28px; padding: 0 14px; border: 1px solid var(--line); border-radius: 4px; background: var(--raised); cursor: default; }
  .btn:hover:not(:disabled) { border-color: var(--accent); }
  .btn:disabled { opacity: 0.45; }
  .btn.primary { background: var(--accent); border-color: var(--accent); color: #04121c; font-weight: 600; }
  .btn.danger { background: var(--bad); border-color: var(--bad); color: #fff; }
  .about { grid-template-columns: auto 1fr; align-items: center; gap: 14px; }
  .about img { border-radius: 10px; }
  .muted { color: var(--muted); }
</style>
