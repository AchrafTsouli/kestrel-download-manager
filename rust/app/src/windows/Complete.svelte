<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { startDrag } from "@crabnebula/tauri-plugin-drag";
  import { call, size, exact, applyTheme, fitHeight, type Download, type Settings } from "../lib/api";

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
  $effect(() => { void d; void error; fitHeight(); });
  async function close() {
    if (dontShow && settings) await call("save_settings", { settings: { ...settings, showCompleteDialog: false } });
    win.close();
  }
  async function open() { try { await call("open_file", { id }); await close(); } catch (e) { error = String(e); } }
  /** Drag the file itself into Explorer, a mail or a chat. */
  async function dragOut(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    try { const item = await call<{ path: string; icon: string }>("drag_item", { id }); await startDrag({ item: [item.path], icon: item.icon }); }
    catch (err) { error = String(err); }
  }
  async function openWith() { try { await call("open_with", { id }); await close(); } catch (e) { error = String(e); } }
  async function folder() { try { await call("show_in_folder", { id }); await close(); } catch (e) { error = String(e); } }
</script>

<div class="win">
  <div class="win-body">
    {#if d}
      <div class="top">
        <!-- The finished file: drag it to the desktop, a folder or a chat. -->
        <button class="drag" onmousedown={dragOut} title="Drag the file to the desktop, a folder or a chat" aria-label="Drag the file out">
          <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
            <path d="M3,8 L13,8 L16,11 L33,11 C34,11 35,12 35,13 L35,29 C35,30 34,31 33,31 L3,31 C2,31 1,30 1,29 L1,10 C1,9 2,8 3,8 Z" fill="#E5A93B" stroke="#B57F20" />
            <path d="M10,4 L26,4 L26,18 L10,18 Z" fill="#FFFFFF" stroke="#C0C0C0" stroke-width="0.8" />
            <path d="M1,14 L35,14 L32,31 L1,31 Z" fill="#F5C04A" stroke="#C88E20" stroke-width="0.8" />
          </svg>
        </button>
        <div>
          <div class="title">Download complete</div>
          <div class="muted">Downloaded {size(d.total)} ({exact(d.total ?? 0)})</div>
        </div>
      </div>
      <label class="field"><span class="muted">Address</span><input class="input" readonly value={d.url} /></label>
      <label class="field"><span class="muted">The file saved as</span><input class="input" readonly value={d.path} /></label>
      {#if error}<p class="error-text">{error}</p>{/if}
      <div class="buttons">
        <button class="btn" onclick={open}>Open</button>
        <button class="btn" onclick={openWith}>Open with...</button>
        <button class="btn" onclick={folder}>Open folder</button>
        <div class="grow"></div>
        <button class="btn" onclick={close}>Close</button>
      </div>
      <label class="check foot"><input type="checkbox" bind:checked={dontShow} /> Don't show this dialog again</label>
    {/if}
  </div>
</div>

<style>
  .win-body { display: flex; flex-direction: column; gap: 8px; padding: 14px 16px 12px; }
  .top { display: flex; align-items: center; gap: 12px; margin-bottom: 4px; }
  .drag { width: 44px; height: 44px; display: grid; place-items: center; border: 1px dashed transparent; border-radius: 6px; background: transparent; cursor: grab; padding: 0; }
  .drag:hover { border-color: var(--line); }
  .title { font-weight: 600; font-size: 14px; }
  .field { display: flex; flex-direction: column; gap: 3px; }
  .field span { font-size: 11px; }
  .buttons { display: flex; gap: 6px; margin-top: 6px; }
  .grow { flex: 1; }
  .foot { color: var(--muted); font-size: 11px; margin-top: 4px; }
</style>
