<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { call, size, applyTheme, type Settings } from "../lib/api";

  type File = { index: number; path: string; length: number };
  type Info = { name: string; infoHash: string; files: File[]; totalBytes: number };
  type Pending = { request: { url: string } };

  let { id }: { id: string } = $props();
  const win = getCurrentWindow();
  let source = $state("");
  let info: Info | null = $state(null);
  let loading = $state(true);
  let error = $state("");
  let busy = $state(false);
  let chosen: Set<number> = $state(new Set());
  let folder = $state("");
  let seed = $state(false);
  let seedMinutes = $state(30);
  let filter = $state("");

  /** What the window shows for the source: the magnet's name, or the file's path. */
  const sourceText = $derived.by(() => {
    if (!source.toLowerCase().startsWith("magnet:")) return source;
    const name = new URLSearchParams(source.slice(8)).get("dn");
    return `Magnet link · ${name ?? "unnamed"}`;
  });

  async function load() {
    loading = true; error = ""; info = null;
    try {
      const offer = await call<{ info: Info; folder: string }>("load_torrent", { id });
      info = offer.info;
      if (!folder) folder = offer.folder;
      chosen = new Set(offer.info.files.map((f) => f.index));
      win.setTitle(`Download torrent · ${offer.info.name}`);
    } catch (e) { error = String(e); }
    loading = false;
  }
  onMount(() => {
    call<Settings>("get_settings").then((s) => applyTheme(s.theme));
    call<Pending | null>("get_pending", { id }).then((p) => { if (p) { source = p.request.url; load(); } else { loading = false; error = "This download was already added or cancelled."; } });
    window.addEventListener("keydown", (e) => { if (e.key === "Escape") cancel(); });
  });

  const shown = $derived(info ? info.files.filter((f) => !filter || f.path.toLowerCase().includes(filter.toLowerCase())) : []);
  const chosenBytes = $derived(info ? info.files.filter((f) => chosen.has(f.index)).reduce((sum, f) => sum + f.length, 0) : 0);
  const ready = $derived(!busy && !!info && chosen.size > 0 && folder.trim().length > 0);

  function toggle(index: number) {
    const next = new Set(chosen);
    next.has(index) ? next.delete(index) : next.add(index);
    chosen = next;
  }
  /** Ticks or unticks every file currently shown (all of them when nothing is searched). */
  function all(on: boolean) {
    const next = new Set(chosen);
    for (const f of shown) on ? next.add(f.index) : next.delete(f.index);
    chosen = next;
  }
  async function browse() {
    const picked = await pickFolder({ directory: true, defaultPath: folder || undefined, title: "Save the torrent in" });
    if (typeof picked === "string") folder = picked;
  }
  async function confirm(start: "now" | "later") {
    busy = true; error = "";
    try {
      await call("confirm_torrent", { choice: { pendingId: id, files: [...chosen], folder, seedMinutes: seed ? Math.min(1440, Math.max(1, seedMinutes)) : 0, start } });
      win.close();
    } catch (e) { error = String(e); busy = false; }
  }
  async function cancel() { await call("cancel_pending", { id }); win.close(); }
</script>

<div class="win">
  <div class="win-body">
    <div class="source" title={source}>{sourceText}</div>

    {#if loading}
      <div class="wait"><span class="spin" aria-hidden="true"></span>
        {source.toLowerCase().startsWith("magnet:") ? "Asking people who share this torrent for its file list… this can take up to 3 minutes." : "Reading the torrent file…"}
      </div>
    {:else if info}
      <div class="title">
        <b>{info.name}</b>
        <span class="muted">{info.files.length === 1 ? "One file" : `${info.files.length} files`} · {size(info.totalBytes)}</span>
      </div>
      {#if info.files.length > 1}
        <div class="row tools">
          <button class="btn small" onclick={() => all(true)}>Select all</button>
          <button class="btn small" onclick={() => all(false)}>Select none</button>
          <input class="input find" type="search" placeholder="Find a file…" bind:value={filter} aria-label="Find a file" />
        </div>
      {/if}
      <div class="list">
        {#each shown as f (f.index)}
          <label class="item" class:on={chosen.has(f.index)}>
            <input type="checkbox" checked={chosen.has(f.index)} onchange={() => toggle(f.index)} />
            <span class="label" title={f.path}>{f.path}</span>
            <span class="muted size">{size(f.length, "0 B")}</span>
          </label>
        {/each}
      </div>
      <p class="muted count">{chosen.size} of {info.files.length} files · {size(chosenBytes, "0 B")}. Only the chosen files are downloaded.</p>
      <div class="form">
        <label for="folder">Save in</label>
        <div class="row"><input id="folder" class="input" bind:value={folder} spellcheck="false" /><button class="btn small" onclick={browse}>Browse…</button></div>
      </div>
      <div class="row">
        <label class="check"><input type="checkbox" bind:checked={seed} /> Share the finished files with others for</label>
        <input class="input num" type="number" min="1" max="1440" bind:value={seedMinutes} disabled={!seed} aria-label="Minutes to share" /> <span class="muted">min</span>
      </div>
    {/if}
    {#if error}<p class="error-text">{error}</p>{/if}
  </div>
  <div class="win-actions">
    {#if error && !loading && !info}<button class="btn" onclick={load}>Try again</button>{/if}
    <div class="left"></div>
    <button class="btn" onclick={() => confirm("later")} disabled={!ready}>Download later</button>
    <button class="btn primary" onclick={() => confirm("now")} disabled={!ready}>Download</button>
    <button class="btn" onclick={cancel}>Cancel</button>
  </div>
</div>

<style>
  .win-body { display: flex; flex-direction: column; gap: 10px; min-height: 0; }
  .source { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--accent); }
  .title { display: flex; flex-direction: column; gap: 2px; }
  .title b { font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wait { display: flex; align-items: center; gap: 10px; padding: 24px 4px; color: var(--muted); }
  .spin { flex: none; width: 16px; height: 16px; border: 2px solid var(--line); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .list { flex: 1; min-height: 120px; overflow: auto; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); }
  .item { display: flex; align-items: center; gap: 8px; padding: 5px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .item.on { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .item .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item .size { font-variant-numeric: tabular-nums; white-space: nowrap; }
  .tools { gap: 6px; }
  .find { margin-left: auto; max-width: 220px; }
  .count { margin: 0; }
  .num { width: 80px; flex: none !important; }
  .small { min-width: 0; }
  .win-actions .left { flex: 1; }
</style>
