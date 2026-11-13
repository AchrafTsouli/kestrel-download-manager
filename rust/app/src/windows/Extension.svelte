<script lang="ts">
  // "Add the browser extension": until the extension is in the stores, it is added by hand with
  // the browser's "Load unpacked". Four steps, with buttons for the parts Kestrel can do.
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, applyTheme, type Settings } from "../lib/api";

  type Browser = { id: string; name: string; page: string };
  let browsers: Browser[] = $state([]);
  let folder: string | null = $state(null);
  let firefoxManifest: string | null = $state(null);
  let chosen = $state("");
  let note = $state("");
  let error = $state("");
  const win = getCurrentWindow();

  onMount(async () => {
    call<Settings>("get_settings").then((s) => applyTheme(s.theme));
    const setup = await call<{ browsers: Browser[]; folder: string | null; firefoxManifest: string | null }>("extension_setup");
    browsers = setup.browsers;
    folder = setup.folder;
    firefoxManifest = setup.firefoxManifest;
    chosen = browsers[0]?.id ?? "";
  });
  const browser = $derived(browsers.find((b) => b.id === chosen));

  async function openPage() {
    error = ""; note = "";
    try { await call("open_extensions_page", { browser: chosen }); note = `${browser?.name} opens its extensions page.`; }
    catch (e) { error = String(e); }
  }
  async function copy(text: string, what: string) {
    try { await navigator.clipboard.writeText(text); note = `${what} copied. Paste it with Ctrl+V.`; } catch { error = "Could not copy; select the text and copy it."; }
  }
  async function openFolder() { if (folder) try { await call("open_folder", { path: folder }); } catch (e) { error = String(e); } }
  async function check() {
    error = ""; note = "";
    try {
      await call("register_browsers");
      note = "Kestrel's bridge is registered. Click the Kestrel icon in the browser: it should say \"Connected\".";
    } catch (e) { error = String(e); }
  }
</script>

<div class="win">
  <div class="win-body">
    <p class="intro">The Kestrel extension sends your browser's downloads and videos to Kestrel. Until it is in the browser stores, add it once by hand: it takes a minute.</p>

    {#if browsers.length === 0}
      <p class="error-text">No Chrome, Edge, Brave, Opera, Vivaldi or Firefox was found on this computer.</p>
    {:else}
      {#if chosen === "firefox"}
      <ol class="steps">
        <li>
          <div class="step-title">Open Firefox's add-on debugging page</div>
          <div class="row">
            <select class="input" bind:value={chosen} aria-label="Browser">{#each browsers as b}<option value={b.id}>{b.name}</option>{/each}</select>
            <button class="btn primary" onclick={openPage}>Open the page</button>
          </div>
          {#if browser}<div class="muted small">Or type <code>{browser.page}</code> in the address bar. <button class="link" onclick={() => copy(browser!.page, "The address")}>Copy</button></div>{/if}
        </li>
        <li>
          <div class="step-title">Click "Load Temporary Add-on…" and choose this file</div>
          {#if firefoxManifest}
            <div class="row"><input class="input" readonly value={firefoxManifest} aria-label="Firefox manifest" /><button class="btn" onclick={() => copy(firefoxManifest!, "The file path")}>Copy path</button></div>
            <div class="muted small">In the file window, paste the path in the file name box and press Enter.</div>
          {:else}
            <p class="error-text">The Firefox build of the extension was not found next to Kestrel. Reinstall Kestrel to restore it.</p>
          {/if}
        </li>
        <li>
          <div class="step-title">Check the connection</div>
          <div class="row"><button class="btn" onclick={check}>Connect Firefox to Kestrel</button></div>
        </li>
      </ol>
      <p class="note">Firefox keeps a temporary add-on until it closes; add it again after restarting Firefox. A permanent add-on comes with Kestrel's listing on addons.mozilla.org.</p>
      {:else}
      <ol class="steps">
        <li>
          <div class="step-title">Open the extensions page</div>
          <div class="row">
            <select class="input" bind:value={chosen} aria-label="Browser">{#each browsers as b}<option value={b.id}>{b.name}</option>{/each}</select>
            <button class="btn primary" onclick={openPage}>Open extensions page</button>
          </div>
          {#if browser}<div class="muted small">Or type <code>{browser.page}</code> in the address bar. <button class="link" onclick={() => copy(browser!.page, "The address")}>Copy</button></div>{/if}
        </li>
        <li>
          <div class="step-title">Turn on "Developer mode"</div>
          <div class="muted">The switch is at the top right of the page (on the left in Edge).</div>
        </li>
        <li>
          <div class="step-title">Click "Load unpacked" and choose Kestrel's extension folder</div>
          {#if folder}
            <div class="row"><input class="input" readonly value={folder} aria-label="Extension folder" /><button class="btn" onclick={() => copy(folder!, "The folder path")}>Copy path</button><button class="btn" onclick={openFolder}>Open folder</button></div>
            <div class="muted small">In the folder window, paste the path in the address bar and press Enter, then click "Select Folder".</div>
          {:else}
            <p class="error-text">The extension folder was not found next to Kestrel. Reinstall Kestrel to restore it.</p>
          {/if}
        </li>
        <li>
          <div class="step-title">Pin the Kestrel icon</div>
          <div class="muted">Click the puzzle icon next to the address bar, then the pin next to "Kestrel Integration".</div>
          <div class="row"><button class="btn" onclick={check}>Check the connection</button></div>
        </li>
      </ol>
      {/if}
    {/if}
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<p class="error-text">{error}</p>{/if}
  </div>
  <div class="win-actions"><button class="btn" onclick={() => win.close()}>Close</button></div>
</div>

<style>
  .intro { margin: 0; }
  .steps { margin: 0; padding-inline-start: 22px; display: grid; gap: 14px; }
  .steps li::marker { font-weight: 700; color: var(--accent); }
  .step-title { font-weight: 600; margin-bottom: 5px; }
  .row { display: flex; gap: 8px; align-items: center; margin-top: 4px; }
  .row .input { flex: 1; min-width: 0; }
  .row select.input { flex: 0 1 220px; }
  .small { font-size: 11px; margin-top: 4px; }
  code { font-family: Consolas, monospace; background: var(--raised); padding: 0 4px; border-radius: 3px; }
  .link { border: 0; background: none; padding: 0; color: var(--accent); cursor: pointer; font: inherit; text-decoration: underline; }
</style>
