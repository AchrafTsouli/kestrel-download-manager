<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
  import { save as pickPath } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import Icon from "../lib/Icon.svelte";
  import { call, categories, size, applyTheme, fitHeight, type Probe, type Settings } from "../lib/api";

  let { id }: { id: string } = $props();
  type Pending = { id: string; request: { url: string; fileName: string | null; description: string | null }; probe: Probe | null };
  let pending: Pending | null = $state(null);
  let settings: Settings | null = $state(null);
  let fileName = $state("");
  let folder = $state("");
  let category = $state("Other");
  let description = $state("");
  let queue = $state("Main download queue");
  let total: number | null = $state(null);
  let checking = $state(true);
  let note = $state("");
  let error = $state("");
  let remember = $state(true);
  let nameEdited = false;
  // "Download Later": which queue; "Preview": the files inside a .zip on the server.
  let later = $state(false);
  type ZipEntry = { name: string; size: number; packed: number; modified: string; folder: boolean };
  let preview: ZipEntry[] | null = $state(null);
  let previewing = $state(false);
  // A program's own logo, read from the server before it is downloaded (like IDM).
  let logo: string | null = $state(null);
  let logoFor = "";
  $effect(() => {
    if (!pending || !/\.exe$/i.test(fileName) || logoFor === pending.request.url) return;
    logoFor = pending.request.url;
    call<string | null>("pending_icon", { id }).then((u) => (logo = u)).catch(() => {});
  });
  const isZip = $derived(/\.zip$/i.test(fileName));
  async function showPreview() {
    previewing = true; error = "";
    try { preview = await call<ZipEntry[]>("zip_preview", { id }); }
    catch (e) { error = String(e); }
    finally { previewing = false; }
  }
  // Earlier folders, for the "Save As" list.
  const recentFolders = $derived(settings ? [...new Set([settings.downloadDir, ...Object.values(settings.categoryDirs ?? {})].filter(Boolean))] : []);
  let folderEdited = false;
  const win = getCurrentWindow();

  function useProbe(p: Probe) {
    checking = false;
    total = p.total;
    note = p.error ?? (p.resumable ? "" : "This server does not allow resuming; a paused download restarts from the beginning.");
    if (!nameEdited) fileName = p.fileName;
    if (!nameEdited) category = p.category;
    if (!folderEdited) folder = p.folder;
  }
  onMount(() => {
    call<Settings>("get_settings").then((s) => { settings = s; applyTheme(s.theme); });
    call<Pending | null>("get_pending", { id }).then((p) => {
      pending = p;
      if (!p) { error = "This download was already added or cancelled."; checking = false; return; }
      fileName = p.request.fileName ?? decodeURIComponent(new URL(p.request.url).pathname.split("/").pop() || "download.bin");
      description = p.request.description ?? "";
      if (p.probe) useProbe(p.probe);
    });
    const sub = listen<[string, Probe]>("probe", ({ payload }) => { if (payload[0] === id) useProbe(payload[1]); });
    window.addEventListener("keydown", (e) => { if (e.key === "Escape") cancel(); });
    return () => sub.then((f) => f());
  });
  function changeCategory() {
    if (!settings) return;
    nameEdited = true;
    if (!folderEdited) folder = settings.categoryFolders && category !== "Other" ? `${settings.downloadDir}\\${category}` : settings.downloadDir;
  }
  // "Save As" is the whole path, as in Kestrel 1.x; folder and name are kept apart behind it.
  const savePath = $derived(folder ? folder.replace(/[\\/]+$/, "") + "\\" + fileName : fileName);
  function editPath(value: string) {
    const cut = Math.max(value.lastIndexOf("\\"), value.lastIndexOf("/"));
    if (cut >= 0) { folder = value.slice(0, cut); fileName = value.slice(cut + 1); folderEdited = true; } else fileName = value;
    nameEdited = true;
  }
  async function browse() {
    const chosen = await pickPath({ defaultPath: savePath || undefined, title: "Save As" });
    if (typeof chosen === "string") editPath(chosen);
  }
  async function start(mode: "now" | "later" | "queue") {
    error = "";
    try {
      await call("confirm_pending", { choice: { pendingId: id, fileName, folder, category, description, start: mode, queue, remember } });
      win.close();
    } catch (e) { error = String(e); }
  }
  // Fit the window to its content once it is filled, and when a note or an error appears.
  $effect(() => { void pending; void note; void error; void checking; void later; fitHeight(); });
  // The archive list needs room: the window grows while it is shown, and fits again after.
  $effect(() => { if (preview) win.setSize(new LogicalSize(window.innerWidth, Math.max(window.innerHeight, 440))); else fitHeight(); });
  async function cancel() { await call("cancel_pending", { id }); win.close(); }
</script>

<div class="win">
  <div class="win-body">
    <div class="grid">
      <label for="url">URL</label>
      <input id="url" class="input wide" readonly value={pending?.request.url ?? ""} />

      <label for="cat">Category</label>
      <div class="row cat">
        <select id="cat" class="input" bind:value={category} onchange={changeCategory}>{#each categories as c}<option value={c}>{c}</option>{/each}</select>
        <button class="btn plus" onclick={() => call("show_window", { name: "options" })} title="Categories and their folders (Options > Save to)" aria-label="Category settings">+</button>
      </div>
      <div class="side">
        {#if logo && /\.exe$/i.test(fileName)}<img src={logo} alt="" width="40" height="40" />{:else}<Icon name={category} size={40} />{/if}
        <b title={total != null ? `${total.toLocaleString()} bytes` : ""}>{checking ? "Checking…" : total != null ? size(total) : "Unknown"}</b>
        <button class="btn preview" onclick={showPreview} disabled={!isZip || checking || previewing} title={isZip ? "See the files inside this archive" : "Preview is for .zip archives"}>{previewing ? "Reading…" : "Preview"}</button>
      </div>

      <label for="saveas">Save As</label>
      <div class="row">
        <input id="saveas" class="input" list="folders" value={savePath} oninput={(e) => editPath((e.currentTarget as HTMLInputElement).value)} spellcheck="false" />
        <datalist id="folders">{#each recentFolders as f}<option value={f.replace(/[\\/]+$/, "") + "\\" + fileName}></option>{/each}</datalist>
        <button class="btn dots" onclick={browse} title="Browse destination file or folder">...</button>
      </div>

      <span></span>
      <fieldset class="remember">
        <legend><label class="check"><input type="checkbox" bind:checked={remember} /> Remember this path for "{category}" category</label></legend>
        <input class="input dim" readonly value={folder} disabled={!remember} />
      </fieldset>

      <label for="desc">Description</label>
      <input id="desc" class="input wide" bind:value={description} />
    </div>
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<p class="error-text">{error}</p>{/if}
    {#if later}
      <div class="later" role="group" aria-label="Download later">
        <label for="q">Add to queue</label>
        <select id="q" class="input" bind:value={queue}>{#each settings?.queues ?? [] as q}<option value={q.name}>{q.name}</option>{/each}</select>
        <button class="btn go" onclick={() => start("queue")}>OK</button>
        <button class="btn" onclick={() => (later = false)}>Back</button>
      </div>
    {:else}
      <div class="buttons">
        <button class="btn" onclick={() => (later = true)} disabled={!pending} title="Add it to a queue; it starts with the queue">Download Later</button>
        <button class="btn go" onclick={() => start("now")} disabled={!pending || !fileName.trim()}>Start Download</button>
        <button class="btn" onclick={cancel}>Cancel</button>
      </div>
    {/if}
  </div>
</div>

{#if preview}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (preview = null)}>
    <div class="zip" role="dialog" aria-label="Zip preview">
      <div class="zip-title">{fileName}, &nbsp;total {preview.filter((e) => !e.folder).length} file{preview.filter((e) => !e.folder).length === 1 ? "" : "s"}</div>
      <div class="zip-table">
        <div class="zr head"><span>File Name</span><span>Size</span><span>Packed</span><span>Modified</span></div>
        {#each preview.filter((e) => !e.folder) as e}
          <div class="zr"><span title={e.name}>{e.name}</span><span>{size(e.size)}</span><span>{size(e.packed)}</span><span>{e.modified}</span></div>
        {/each}
      </div>
      <div class="zip-ok"><button class="btn" onclick={() => (preview = null)}>OK</button></div>
    </div>
  </div>
{/if}

<style>
  .grid { display: grid; grid-template-columns: 76px 1fr 110px; gap: 6px 8px; align-items: center; }
  .cat select { flex: 1; min-width: 0; max-width: 260px; }
  .plus { min-width: 0; width: 26px; padding: 0; font-size: 15px; line-height: 1; }
  .preview { min-width: 0; width: 96px; margin-top: 4px; }
  .later { display: flex; justify-content: center; align-items: center; gap: 8px; margin-top: 10px; }
  .later select { width: 220px; flex: none; }
  .scrim { position: fixed; inset: 0; display: grid; place-items: center; background: rgb(0 0 0 / 0.45); z-index: 40; }
  .zip { width: calc(100vw - 30px); max-height: calc(100vh - 30px); display: flex; flex-direction: column; gap: 8px; padding: 10px; background: var(--surface); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 18px 50px rgb(0 0 0 / 0.45); }
  .zip-title { text-align: center; }
  .zip-table { flex: 1; min-height: 120px; overflow: auto; border: 1px solid var(--line); background: var(--bg); font-variant-numeric: tabular-nums; }
  .zr { display: grid; grid-template-columns: 1fr 86px 86px 120px; gap: 8px; padding: 3px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .zr span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .zr.head { position: sticky; top: 0; background: var(--chrome); color: var(--muted); }
  .zip-ok { display: flex; justify-content: center; }
  .grid > label { text-align: end; color: var(--muted); }
  .wide { grid-column: 2 / 4; }
  .side { grid-column: 3; grid-row: 2 / span 3; display: flex; flex-direction: column; align-items: center; gap: 4px; }
  .side b { font-weight: 600; }
  .dots { min-width: 0; width: 30px; padding: 0; }
  .remember { margin: 0; border: 1px solid var(--line); border-radius: 3px; padding: 2px 8px 8px; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .remember legend { padding: 0 4px; }
  .dim { color: var(--muted); }
  .buttons { display: flex; justify-content: center; gap: 12px; margin-top: 12px; }
  .buttons .btn { min-width: 116px; }
  .go { background: #1b4934; border-color: #288057; color: #fff; font-weight: 600; }
  .go:hover:not(:disabled) { background: #22593f; border-color: #33a06c; }
</style>
