<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, applyTheme, type Settings } from "../lib/api";

  const win = getCurrentWindow();
  let url = $state("");
  let types = $state("zip rar 7z pdf mp3 mp4 mkv iso exe msi");
  let found: string[] = $state([]);
  let chosen: Set<string> = $state(new Set());
  let busy = $state(false);
  let searched = $state(false);
  let error = $state("");
  let message = $state("");

  onMount(() => {
    call<Settings>("get_settings").then((s) => applyTheme(s.theme));
    window.addEventListener("keydown", (e) => { if (e.key === "Escape") win.close(); });
  });

  async function search(event: SubmitEvent) {
    event.preventDefault();
    busy = true; error = ""; message = ""; found = []; searched = false;
    try {
      found = await call<string[]>("grab_site", { url, extensions: types });
      chosen = new Set(found);
      searched = true;
    } catch (e) { error = String(e); }
    busy = false;
  }
  const name = (link: string) => decodeURIComponent(new URL(link).pathname.split("/").pop() || link);
  function toggle(link: string) {
    const next = new Set(chosen);
    next.has(link) ? next.delete(link) : next.add(link);
    chosen = next;
  }
  async function add(start: "now" | "later") {
    const results = await call<({ Ok: string } | { Err: string })[]>("add_downloads", { urls: [...chosen], start: start === "now" ? "queue" : "later" });
    const failed = results.filter((r) => "Err" in r).length;
    if (start === "now") await call("run_waiting", { name: "Main download queue" });
    message = `Added ${results.length - failed} file${results.length - failed === 1 ? "" : "s"}${failed ? `, ${failed} could not be added` : ""}.`;
    if (!failed) setTimeout(() => win.close(), 900);
  }
</script>

<div class="win">
  <form class="win-body" onsubmit={search}>
    <div class="form">
      <label for="site">Site</label>
      <input id="site" class="input" bind:value={url} placeholder="https://example.com/downloads/" spellcheck="false" required />
      <label for="types">File types</label>
      <input id="types" class="input" bind:value={types} spellcheck="false" />
    </div>
    <div class="row">
      <span class="muted">Follows up to 20 pages of this site and lists links to these types.</span>
      <div class="left"></div>
      <button class="btn primary" type="submit" disabled={busy || !url.trim()}>{busy ? "Searching…" : "Find files"}</button>
    </div>
    {#if searched}
      <div class="row">
        <b>{found.length} file{found.length === 1 ? "" : "s"} found</b>
        {#if found.length}
          <button class="btn small" type="button" onclick={() => (chosen = new Set(found))}>Select all</button>
          <button class="btn small" type="button" onclick={() => (chosen = new Set())}>Select none</button>
        {/if}
      </div>
      <div class="list">
        {#each found as link (link)}
          <label class="item" class:on={chosen.has(link)} title={link}>
            <input type="checkbox" checked={chosen.has(link)} onchange={() => toggle(link)} />
            <span class="label">{name(link)}</span>
            <span class="muted path">{new URL(link).pathname}</span>
          </label>
        {:else}
          <p class="note">No links to these file types were found on this site.</p>
        {/each}
      </div>
    {/if}
    {#if error}<p class="error-text">{error}</p>{/if}
    {#if message}<p class="note">{message}</p>{/if}
  </form>
  <div class="win-actions">
    <div class="left"></div>
    <button class="btn" onclick={() => add("later")} disabled={chosen.size === 0}>Download later</button>
    <button class="btn primary" onclick={() => add("now")} disabled={chosen.size === 0}>Download {chosen.size || ""}</button>
    <button class="btn" onclick={() => win.close()}>Close</button>
  </div>
</div>

<style>
  .win-body { display: flex; flex-direction: column; gap: 10px; min-height: 0; }
  .left { flex: 1; }
  .list { flex: 1; min-height: 120px; overflow: auto; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); }
  .item { display: flex; align-items: center; gap: 8px; padding: 5px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .item.on { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .label { flex: none; max-width: 45%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-align: right; }
  .small { min-width: 0; }
  .note { padding: 8px; }
</style>
