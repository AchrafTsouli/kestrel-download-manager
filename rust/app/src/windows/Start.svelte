<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
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
  async function browse() {
    const chosen = await pickFolder({ directory: true, defaultPath: folder || undefined, title: "Save in folder" });
    if (typeof chosen === "string") { folder = chosen; folderEdited = true; }
  }
  async function start(mode: "now" | "later" | "queue") {
    error = "";
    try {
      await call("confirm_pending", { choice: { pendingId: id, fileName, folder, category, description, start: mode, queue } });
      win.close();
    } catch (e) { error = String(e); }
  }
  async function cancel() { await call("cancel_pending", { id }); win.close(); }
</script>

<div class="win">
  <div class="win-body">
    <div class="form">
      <label for="url">URL</label>
      <input id="url" class="input" readonly value={pending?.request.url ?? ""} />
      <label for="cat">Category</label>
      <div class="row">
        <select id="cat" class="input" bind:value={category} onchange={changeCategory}>{#each categories as c}<option value={c}>{c}</option>{/each}</select>
        <Icon name={category} size={22} />
      </div>
      <label for="name">Save as</label>
      <input id="name" class="input" bind:value={fileName} oninput={() => (nameEdited = true)} spellcheck="false" />
      <label for="folder">In folder</label>
      <div class="row"><input id="folder" class="input" bind:value={folder} oninput={() => (folderEdited = true)} spellcheck="false" /><button class="btn small" onclick={browse}>Browse…</button></div>
      <label for="desc">Description</label>
      <input id="desc" class="input" bind:value={description} />
      <label for="q">Queue</label>
      <select id="q" class="input" bind:value={queue}>{#each settings?.queues ?? [] as q}<option value={q.name}>{q.name}</option>{/each}</select>
    </div>
    <div class="size">
      <span class="muted">File size</span>
      <b>{checking ? "Checking…" : total != null ? `${size(total)} (${total.toLocaleString()} bytes)` : "Unknown"}</b>
    </div>
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<p class="error-text">{error}</p>{/if}
  </div>
  <div class="win-actions">
    <button class="btn" onclick={() => start("queue")} disabled={!pending} title="Add it to the chosen queue; it starts with the queue">Download later</button>
    <div class="left"></div>
    <button class="btn primary" onclick={() => start("now")} disabled={!pending || !fileName.trim()}>Start download</button>
    <button class="btn" onclick={cancel}>Cancel</button>
  </div>
</div>

<style>
  .size { display: flex; gap: 10px; align-items: baseline; padding-left: 120px; }
  .small { min-width: 0; }
  .win-actions .left { flex: 1; }
</style>
