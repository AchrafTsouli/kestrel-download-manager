<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { save as pickPath } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import Icon from "../lib/Icon.svelte";
  import { call, categories, size, applyTheme, type Probe, type Settings } from "../lib/api";

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
  // "Save As" is the whole path, as in Flux 1.x; folder and name are kept apart behind it.
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
  async function cancel() { await call("cancel_pending", { id }); win.close(); }
</script>

<div class="win">
  <div class="win-body">
    <div class="grid">
      <label for="url">URL</label>
      <input id="url" class="input wide" readonly value={pending?.request.url ?? ""} />

      <label for="cat">Category</label>
      <select id="cat" class="input" bind:value={category} onchange={changeCategory}>{#each categories as c}<option value={c}>{c}</option>{/each}</select>
      <div class="side">
        <Icon name={category} size={36} />
        <b title={total != null ? `${total.toLocaleString()} bytes` : ""}>{checking ? "Checking…" : total != null ? size(total) : "Unknown"}</b>
      </div>

      <label for="saveas">Save As</label>
      <div class="row"><input id="saveas" class="input" value={savePath} oninput={(e) => editPath((e.currentTarget as HTMLInputElement).value)} spellcheck="false" /><button class="btn dots" onclick={browse} title="Browse destination file or folder">...</button></div>

      <span></span>
      <div class="remember">
        <label class="check"><input type="checkbox" bind:checked={remember} /> Remember this path for "{category}" category</label>
        <input class="input dim" readonly value={folder} />
      </div>

      <label for="desc">Description</label>
      <input id="desc" class="input wide" bind:value={description} />
    </div>
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<p class="error-text">{error}</p>{/if}
    <div class="queue"><label for="q">Queue</label><select id="q" class="input" bind:value={queue}>{#each settings?.queues ?? [] as q}<option value={q.name}>{q.name}</option>{/each}</select></div>
    <div class="buttons">
      <button class="btn" onclick={() => start("queue")} disabled={!pending} title="Add it to the chosen queue; it starts with the queue">Download Later</button>
      <button class="btn go" onclick={() => start("now")} disabled={!pending || !fileName.trim()}>Start Download</button>
      <button class="btn" onclick={cancel}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .grid { display: grid; grid-template-columns: 76px 1fr 96px; gap: 6px 8px; align-items: center; }
  .grid > label { text-align: end; color: var(--muted); }
  .wide { grid-column: 2 / 4; }
  .side { grid-column: 3; grid-row: 2 / span 3; display: flex; flex-direction: column; align-items: center; gap: 6px; }
  .side b { font-weight: 600; }
  .dots { min-width: 0; width: 30px; padding: 0; }
  .remember { border: 1px solid var(--line); border-radius: 3px; padding: 5px 8px 6px; background: var(--chrome); display: flex; flex-direction: column; gap: 6px; }
  .dim { color: var(--muted); }
  .queue { display: flex; justify-content: center; align-items: center; gap: 10px; margin-top: 10px; }
  .queue select { width: 230px; flex: none; }
  .buttons { display: flex; justify-content: center; gap: 12px; margin-top: 6px; }
  .go { background: #1b4934; border-color: #288057; color: #fff; font-weight: 600; }
  .go:hover:not(:disabled) { background: #22593f; border-color: #33a06c; }
</style>
