<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, applyTheme, type Settings } from "../lib/api";

  let text = $state("");
  let pattern = $state("");
  let from = $state(1);
  let to = $state(10);
  let width = $state(1);
  let result = $state("");
  const win = getCurrentWindow();
  onMount(() => { call<Settings>("get_settings").then((s) => applyTheme(s.theme)); });

  // Links typed or pasted (one per line), plus an optional numbered series: https://site/file_*.zip
  const links = $derived.by(() => {
    const list = text.split(/\s+/).filter((l) => /^https?:\/\//i.test(l));
    if (pattern.includes("*") && /^https?:\/\//i.test(pattern) && to >= from && to - from < 5000)
      for (let n = from; n <= to; n++) list.push(pattern.replace("*", String(n).padStart(width, "0")));
    return [...new Set(list)];
  });
  async function add(start: string) {
    const results = await call<{ Ok?: string; Err?: string }[]>("add_downloads", { urls: links, start });
    const failed = results.filter((r) => "Err" in r).length;
    result = `${results.length - failed} added${failed ? `, ${failed} skipped (not valid links)` : ""}.`;
    if (!failed) win.close();
  }
</script>

<div class="win">
  <div class="win-body">
    <label class="muted" for="links">Links, one per line</label>
    <textarea id="links" class="input" rows="8" bind:value={text} placeholder="https://example.com/part1.zip&#10;https://example.com/part2.zip" spellcheck="false"></textarea>
    <fieldset class="group">
      <legend>Numbered series (optional)</legend>
      <div class="form">
        <label for="pat">Address with *</label><input id="pat" class="input" bind:value={pattern} placeholder="https://example.com/photo_*.jpg" spellcheck="false" />
        <label for="from">From / to</label><div class="row"><input id="from" class="input" type="number" bind:value={from} /><input class="input" type="number" bind:value={to} aria-label="To" /><input class="input" type="number" min="1" max="6" bind:value={width} aria-label="Digits" title="Digits (1 = 1, 2 = 01…)" /></div>
      </div>
    </fieldset>
    <p class="note">{links.length} link{links.length === 1 ? "" : "s"} ready. {result}</p>
  </div>
  <div class="win-actions">
    <button class="btn" disabled={!links.length} onclick={() => add("queue")}>Add to queue</button>
    <div class="left" style="flex:1"></div>
    <button class="btn primary" disabled={!links.length} onclick={() => add("now")}>Download now</button>
    <button class="btn" onclick={() => win.close()}>Cancel</button>
  </div>
</div>
