<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Download = {
    id: string; name: string; url: string; path: string; status: string;
    written: number; total: number | null; bytesPerSecond: number; connections: number; error: string | null;
  };

  let downloads: Download[] = $state([]);
  let selected: string | null = $state(null);
  let adding = $state(false);
  let address = $state("");
  let addError = $state("");
  let filter = $state("All");
  let search = $state("");

  const categories = ["All", "Downloading", "Completed", "Paused", "Failed"];
  const visible = $derived(downloads.filter((d) =>
    (filter === "All" || (filter === "Downloading" ? !["Completed", "Paused", "Failed"].includes(d.status) : d.status === filter)) &&
    (!search || d.name.toLowerCase().includes(search.toLowerCase()))));
  const current = $derived(downloads.find((d) => d.id === selected) ?? null);
  const active = $derived(downloads.filter((d) => d.bytesPerSecond > 0));

  onMount(() => {
    invoke<Download[]>("list_downloads").then((list) => (downloads = list));
    const stop = listen<Download>("download", ({ payload }) => {
      const index = downloads.findIndex((d) => d.id === payload.id);
      if (index < 0) downloads = [...downloads, payload];
      else downloads[index] = payload;
    });
    return () => { stop.then((f) => f()); };
  });

  function size(bytes: number | null) {
    if (bytes == null) return "—";
    if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(2) + " GB";
    if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " MB";
    if (bytes >= 1024) return Math.round(bytes / 1024) + " KB";
    return bytes + " B";
  }
  const percent = (d: Download) => (d.total ? Math.min(100, (d.written / d.total) * 100) : d.status === "Completed" ? 100 : 0);
  function remaining(d: Download) {
    if (!d.total || d.bytesPerSecond <= 0) return "—";
    const s = Math.round((d.total - d.written) / d.bytesPerSecond);
    return s >= 3600 ? `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }
  const statusClass = (s: string) => (s === "Completed" ? "good" : s === "Failed" ? "bad" : s === "Paused" ? "warn" : "run");

  async function add(event: SubmitEvent) {
    event.preventDefault();
    addError = "";
    try {
      selected = await invoke<string>("add_download", { url: address });
      adding = false;
      address = "";
    } catch (error) {
      addError = String(error);
    }
  }
  const pause = () => current && invoke("pause_download", { id: current.id });
  const resume = () => current && invoke("resume_download", { id: current.id });
</script>

<div class="shell">
  <header class="toolbar" aria-label="Toolbar">
    <button class="tool primary" onclick={() => { adding = true; addError = ""; }}><span class="glyph">＋</span>Add URL</button>
    <button class="tool" onclick={resume} disabled={!current || !["Paused", "Failed"].includes(current.status)}><span class="glyph">▶</span>Resume</button>
    <button class="tool" onclick={pause} disabled={!current || ["Completed", "Paused", "Failed"].includes(current.status)}><span class="glyph">❚❚</span>Pause</button>
    <div class="spacer"></div>
    <input class="search" type="search" placeholder="Search downloads" bind:value={search} aria-label="Search downloads" />
    <div class="brand"><span class="logo"></span>FLUX</div>
  </header>

  <div class="body">
    <nav class="sidebar" aria-label="Categories">
      <div class="heading">Downloads</div>
      {#each categories as category}
        <button class="category" class:on={filter === category} onclick={() => (filter = category)}>
          {category}
          <small>{category === "All" ? downloads.length : downloads.filter((d) => (category === "Downloading" ? !["Completed", "Paused", "Failed"].includes(d.status) : d.status === category)).length}</small>
        </button>
      {/each}
    </nav>

    <main class="list">
      {#if visible.length === 0}
        <div class="empty">
          <p class="empty-title">{downloads.length === 0 ? "No downloads yet" : "Nothing here"}</p>
          <p>{downloads.length === 0 ? "Click Add URL and paste a link to start." : "Choose another category or clear the search."}</p>
        </div>
      {:else}
        <table>
          <thead><tr><th>File name</th><th class="num">Size</th><th>Status</th><th class="num">Time left</th><th class="num">Speed</th><th class="bar-col">Progress</th></tr></thead>
          <tbody>
            {#each visible as d (d.id)}
              <tr class:sel={selected === d.id} onclick={() => (selected = d.id)} title={d.error ?? d.url}>
                <td class="name">{d.name}</td>
                <td class="num">{size(d.total)}</td>
                <td><span class="status {statusClass(d.status)}">{d.status}</span></td>
                <td class="num">{remaining(d)}</td>
                <td class="num">{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : "—"}</td>
                <td class="bar-col"><div class="bar"><i style="width:{percent(d)}%"></i></div><span class="pct">{Math.floor(percent(d))}%</span></td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </main>
  </div>

  <footer class="status-bar">
    <span>{current?.error ?? (current ? current.path : "Ready.")}</span>
    <span class="totals">Total {downloads.length} · Active {active.length} · {size(active.reduce((s, d) => s + d.bytesPerSecond, 0))}/s</span>
  </footer>
</div>

{#if adding}
  <div class="scrim" role="presentation" onclick={(e) => { if (e.target === e.currentTarget) adding = false; }}>
    <form class="dialog" onsubmit={add} aria-labelledby="add-title">
      <h2 id="add-title">Add a download</h2>
      <label for="address">Address</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input id="address" bind:value={address} placeholder="https://example.com/file.zip" autofocus autocomplete="off" spellcheck="false" />
      {#if addError}<p class="error" role="alert">{addError}</p>{/if}
      <div class="actions">
        <button type="button" class="tool" onclick={() => (adding = false)}>Cancel</button>
        <button type="submit" class="tool primary" disabled={!address.trim()}>Download</button>
      </div>
    </form>
  </div>
{/if}

<style>
  .shell { display: grid; grid-template-rows: auto 1fr auto; height: 100%; }
  .toolbar { display: flex; align-items: center; gap: 6px; padding: 8px 12px; background: var(--surface); border-bottom: 1px solid var(--line); }
  .tool { display: inline-flex; align-items: center; gap: 7px; padding: 7px 14px; border: 1px solid var(--line); border-radius: 6px; background: var(--raised); cursor: pointer; }
  .tool:hover:not(:disabled) { border-color: var(--accent); }
  .tool:disabled { opacity: 0.45; cursor: default; }
  .tool.primary { background: var(--accent); border-color: var(--accent); color: #04121c; font-weight: 600; }
  .glyph { font-size: 11px; width: 12px; text-align: center; }
  .spacer { flex: 1; }
  .search { width: 220px; padding: 7px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .brand { display: flex; align-items: center; gap: 7px; margin-left: 10px; font-weight: 700; letter-spacing: 0.08em; color: var(--accent); }
  .logo { width: 18px; height: 18px; border-radius: 5px; background: linear-gradient(135deg, var(--accent), var(--accent-2)); }
  .body { display: grid; grid-template-columns: 190px 1fr; min-height: 0; }
  .sidebar { background: var(--surface); border-right: 1px solid var(--line); padding: 10px 8px; display: flex; flex-direction: column; gap: 2px; }
  .heading { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); padding: 4px 10px 6px; }
  .category { display: flex; justify-content: space-between; padding: 6px 10px; border: 0; border-radius: 6px; background: transparent; text-align: left; cursor: pointer; color: var(--muted); }
  .category:hover { background: var(--raised); color: var(--text); }
  .category.on { background: var(--selection); color: var(--text); }
  .category small { font-family: var(--mono); }
  .list { overflow: auto; min-width: 0; }
  table { width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }
  th { position: sticky; top: 0; background: var(--surface); text-align: left; font-weight: 600; color: var(--muted); padding: 7px 10px; border-bottom: 1px solid var(--line); }
  td { padding: 7px 10px; border-bottom: 1px solid var(--line); white-space: nowrap; }
  tr { cursor: default; }
  tbody tr:hover { background: color-mix(in srgb, var(--raised) 60%, transparent); }
  tr.sel { background: var(--selection) !important; }
  .name { max-width: 360px; overflow: hidden; text-overflow: ellipsis; }
  .num { text-align: right; font-family: var(--mono); font-size: 12px; }
  .bar-col { width: 190px; }
  .bar { display: inline-block; width: 130px; height: 6px; border-radius: 3px; background: var(--raised); overflow: hidden; vertical-align: middle; }
  .bar i { display: block; height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); transition: width 0.25s; }
  .pct { margin-left: 8px; font-family: var(--mono); font-size: 11px; color: var(--muted); }
  .status.good { color: var(--good); } .status.bad { color: var(--bad); } .status.warn { color: var(--warn); } .status.run { color: var(--accent); }
  .empty { height: 100%; display: grid; place-content: center; text-align: center; color: var(--muted); gap: 4px; }
  .empty p { margin: 0; }
  .empty-title { font-size: 16px; color: var(--text); font-weight: 600; }
  .status-bar { display: flex; justify-content: space-between; gap: 12px; padding: 6px 12px; background: var(--surface); border-top: 1px solid var(--line); color: var(--muted); font-size: 12px; }
  .status-bar span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .totals { font-family: var(--mono); white-space: nowrap; }
  .scrim { position: fixed; inset: 0; background: rgb(0 0 0 / 0.45); display: grid; place-items: center; }
  .dialog { width: min(520px, 92vw); background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 18px 20px; display: grid; gap: 8px; box-shadow: 0 18px 50px rgb(0 0 0 / 0.4); }
  .dialog h2 { margin: 0 0 6px; font-size: 16px; }
  .dialog label { color: var(--muted); font-size: 12px; }
  .dialog input { padding: 9px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .error { margin: 0; color: var(--bad); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; }
</style>
