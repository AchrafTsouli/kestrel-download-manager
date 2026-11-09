<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import Icon from "../lib/Icon.svelte";
  import { call, size, exact, applyTheme, type Download, type Settings } from "../lib/api";

  let { id }: { id: string } = $props();
  let d: Download | null = $state(null);
  let settings: Settings | null = $state(null);
  let dontShow = $state(false);
  let error = $state("");
  const win = getCurrentWindow();

  onMount(() => {
    call<Settings>("get_settings").then((s) => { settings = s; applyTheme(s.theme); });
    call<Download | null>("get_download", { id }).then((v) => (d = v));
  });
  async function close() {
    if (dontShow && settings) await call("save_settings", { settings: { ...settings, showCompleteDialog: false } });
    win.close();
  }
  async function open() { try { await call("open_file", { id }); await close(); } catch (e) { error = String(e); } }
  async function folder() { try { await call("show_in_folder", { id }); await close(); } catch (e) { error = String(e); } }
</script>

<div class="win">
  <div class="win-body">
    {#if d}
      <div class="top">
        <Icon name={d.category} size={40} />
        <div><b>Download complete</b><p class="muted">Downloaded {size(d.total)} ({exact(d.total ?? 0)})</p></div>
      </div>
      <div class="form">
        <label for="addr">Address</label><input id="addr" class="input" readonly value={d.url} />
        <label for="saved">The file saved as</label><input id="saved" class="input" readonly value={d.path} />
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
    {/if}
  </div>
  <div class="win-actions">
    <div class="left"><label class="check"><input type="checkbox" bind:checked={dontShow} /> Don't show this again</label></div>
    <button class="btn primary" onclick={open}>Open</button>
    <button class="btn" onclick={folder}>Open folder</button>
    <button class="btn" onclick={close}>Close</button>
  </div>
</div>

<style>
  .top { display: flex; align-items: center; gap: 12px; }
  .top p { margin: 2px 0 0; }
</style>
