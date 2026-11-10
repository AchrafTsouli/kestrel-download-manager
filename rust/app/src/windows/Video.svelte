<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { call, size, clock, applyTheme, type Settings } from "../lib/api";

  type Format = { formatId: string; extension: string; label: string; totalBytes: number | null; approximate: boolean; audioOnly: boolean; noAudio: boolean };
  type Catalog = { title: string; formats: Format[]; site: string | null; durationSeconds: number | null };
  type Entry = { url: string; title: string; durationSeconds: number | null };
  type Found = { catalog: Catalog | null; playlist: { title: string; entries: Entry[] } | null };
  type Pending = { request: { url: string } };

  let { id }: { id: string } = $props();
  const win = getCurrentWindow();
  let url = $state("");
  let found: Found | null = $state(null);
  let loading = $state(true);
  let error = $state("");
  let busy = $state(false);
  // One video
  let formatId = $state("");
  let clipOn = $state(false);
  let clipFrom = $state("0:00");
  let clipTo = $state("");
  let exact = $state(false);
  let exactAvailable = $state(false);
  // A playlist
  let chosen: Set<string> = $state(new Set());
  let preset = $state("best");
  let subfolder = $state(true);
  // Subtitles, in the language Windows uses (saved as files beside the video).
  let subsOn = $state(false);
  let subsLangs = $state((navigator.language || "en").split("-")[0].toLowerCase());

  async function load() {
    loading = true; error = ""; found = null;
    try {
      const f = await call<Found>("probe_video", { id });
      found = f;
      if (f.catalog) {
        formatId = f.catalog.formats[0]?.formatId ?? "";
        if (f.catalog.durationSeconds) clipTo = clock(Math.round(f.catalog.durationSeconds)).replace(/^00:/, "");
        win.setTitle(`Download video · ${f.catalog.title}`);
      }
      if (f.playlist) { chosen = new Set(f.playlist.entries.map((e) => e.url)); win.setTitle(`Download playlist · ${f.playlist.title}`); }
    } catch (e) { error = String(e); }
    loading = false;
  }
  onMount(() => {
    call<Settings>("get_settings").then((s) => applyTheme(s.theme));
    call<Pending | null>("get_pending", { id }).then((p) => { if (p) { url = p.request.url; load(); } else { loading = false; error = "This download was already added or cancelled."; } });
    call<boolean>("video_exact_cut").then((v) => (exactAvailable = v));
    window.addEventListener("keydown", (e) => { if (e.key === "Escape") cancel(); });
  });

  /** "83", "1:23", "1:02:03" or "83.5" → seconds; null when empty or unreadable. */
  function seconds(text: string): number | null {
    const t = text.trim();
    if (!t) return null;
    const parts = t.split(":").map(Number);
    if (parts.length > 3 || parts.some((n) => !Number.isFinite(n) || n < 0)) return NaN;
    return parts.reduce((total, n) => total * 60 + n, 0);
  }
  const duration = $derived(found?.catalog?.durationSeconds ?? null);
  const clipStart = $derived(clipOn ? seconds(clipFrom) : null);
  const clipEnd = $derived(clipOn ? seconds(clipTo) : null);
  const clipError = $derived.by(() => {
    if (!clipOn) return "";
    if (Number.isNaN(clipStart) || Number.isNaN(clipEnd)) return "Write times as 1:23 or 1:02:03.";
    if (clipStart == null && clipEnd == null) return "Set where the part starts or ends.";
    if (clipStart != null && clipEnd != null && clipStart >= clipEnd) return "The part must end after it starts.";
    if (duration && clipEnd != null && clipEnd > duration + 1) return `The video is ${clock(Math.round(duration))} long.`;
    return "";
  });
  const selected = $derived(found?.catalog?.formats.find((f) => f.formatId === formatId));
  const audio = $derived(found?.catalog ? !!selected?.audioOnly : preset === "audio");
  const wantSubs = $derived(subsOn && !audio && !(found?.catalog && clipOn) && subsLangs.trim().length > 0);
  const subsError = $derived(subsOn && !/^[A-Za-z0-9*._-]+(,[A-Za-z0-9*._-]+)*$/.test(subsLangs.trim()) ? "Write languages like: en, fr, pt-BR." : "");
  const ready = $derived(!subsError && !busy && !loading && (found?.catalog ? !!selected && !clipError : chosen.size > 0));

  function toggle(entry: string) {
    const next = new Set(chosen);
    next.has(entry) ? next.delete(entry) : next.add(entry);
    chosen = next;
  }
  function all(on: boolean) { chosen = new Set(on ? found?.playlist?.entries.map((e) => e.url) : []); }

  async function confirm(start: "now" | "later") {
    busy = true; error = "";
    try {
      const choice = found?.catalog
        ? { pendingId: id, formatId, clipStart: clipOn ? clipStart ?? 0 : null, clipEnd: clipOn ? clipEnd : null, clipExact: clipOn && exact, subtitles: wantSubs ? subsLangs.trim() : null, start }
        : { pendingId: id, entries: [...chosen], maxHeight: /^\d+$/.test(preset) ? Number(preset) : null, audioOnly: preset === "audio", subfolder, subtitles: wantSubs ? subsLangs.trim() : null, start };
      await call("confirm_video", { choice });
      win.close();
    } catch (e) { error = String(e); busy = false; }
  }
  async function asFile() { try { await call("video_as_file", { id }); win.close(); } catch (e) { error = String(e); } }
  async function cancel() { await call("cancel_pending", { id }); win.close(); }
</script>

{#snippet subtitles()}
  <div class="row subs">
    <label class="check" title={audio ? "Audio has no subtitles" : ""}><input type="checkbox" bind:checked={subsOn} disabled={audio} /> Subtitles</label>
    <input class="input langs" bind:value={subsLangs} disabled={!subsOn || audio} aria-label="Subtitle languages" spellcheck="false" />
    <span class="muted">{found?.catalog && clipOn && subsOn ? "not with a part of the video" : "saved beside the video, e.g. en, fr"}</span>
  </div>
  {#if subsError}<p class="error-text">{subsError}</p>{/if}
{/snippet}

<div class="win">
  <div class="win-body">
    <div class="url" title={url}>{url}</div>

    {#if loading}
      <div class="wait"><span class="spin" aria-hidden="true"></span> Reading the video page… this can take a few seconds.</div>
    {:else if found?.catalog}
      {@const c = found.catalog}
      <div class="title">
        <b>{c.title}</b>
        <span class="muted">{[c.site, c.durationSeconds ? clock(Math.round(c.durationSeconds)) : null].filter(Boolean).join(" · ")}</span>
      </div>
      <div class="list" role="radiogroup" aria-label="Quality">
        {#each c.formats as f (f.formatId)}
          <label class="item" class:on={f.formatId === formatId}>
            <input type="radio" name="quality" value={f.formatId} bind:group={formatId} />
            <span class="label">{f.label}</span>
            <span class="muted size">{f.totalBytes ? (f.approximate ? "≈ " : "") + size(f.totalBytes) : ""}</span>
          </label>
        {/each}
      </div>
      {@render subtitles()}
      <fieldset class="clip">
        <label class="check"><input type="checkbox" bind:checked={clipOn} /> Download only a part</label>
        {#if clipOn}
          <div class="row times">
            <label>From <input class="input time" bind:value={clipFrom} placeholder="0:00" spellcheck="false" /></label>
            <label>To <input class="input time" bind:value={clipTo} placeholder="end" spellcheck="false" /></label>
          </div>
          <div class="row modes">
            <label class="check"><input type="radio" name="cut" value={false} bind:group={exact} /> Fast cut <span class="muted">· starts at the nearest keyframe, no re-encoding</span></label>
            <label class="check" title={exactAvailable ? "" : "Windows' video encoder is not available on this PC"}>
              <input type="radio" name="cut" value={true} bind:group={exact} disabled={!exactAvailable} /> Exact cut <span class="muted">· starts on the second, re-encodes the part</span>
            </label>
          </div>
          {#if clipError}<p class="error-text">{clipError}</p>{/if}
        {/if}
      </fieldset>
    {:else if found?.playlist}
      {@const p = found.playlist}
      <div class="title">
        <b>{p.title}</b>
        <span class="muted">{chosen.size} of {p.entries.length} videos</span>
      </div>
      <div class="row tools">
        <button class="btn small" onclick={() => all(true)}>Select all</button>
        <button class="btn small" onclick={() => all(false)}>Select none</button>
      </div>
      <div class="list">
        {#each p.entries as e, i (e.url)}
          <label class="item" class:on={chosen.has(e.url)}>
            <input type="checkbox" checked={chosen.has(e.url)} onchange={() => toggle(e.url)} />
            <span class="n muted">{i + 1}</span>
            <span class="label">{e.title}</span>
            <span class="muted size">{e.durationSeconds ? clock(Math.round(e.durationSeconds)) : ""}</span>
          </label>
        {/each}
      </div>
      <div class="form">
        <label for="preset">Quality</label>
        <select id="preset" class="input" bind:value={preset}>
          <option value="best">Best available · video + audio</option>
          {#each [2160, 1440, 1080, 720, 480, 360] as h}<option value={String(h)}>Up to {h}p · video + audio</option>{/each}
          <option value="audio">Audio only</option>
        </select>
      </div>
      <label class="check"><input type="checkbox" bind:checked={subfolder} /> Save in a folder named after the playlist</label>
      {@render subtitles()}
    {/if}
    {#if error}<p class="error-text">{error}</p>{/if}
  </div>
  <div class="win-actions">
    <button class="btn" onclick={asFile} title="Download the address itself, not its video">Download as file</button>
    {#if error && !loading && !found}<button class="btn" onclick={load}>Try again</button>{/if}
    <div class="left"></div>
    <button class="btn" onclick={() => confirm("later")} disabled={!ready}>Download later</button>
    <button class="btn primary" onclick={() => confirm("now")} disabled={!ready}>{found?.playlist ? `Download ${chosen.size}` : "Download"}</button>
    <button class="btn" onclick={cancel}>Cancel</button>
  </div>
</div>

<style>
  .win-body { display: flex; flex-direction: column; gap: 10px; min-height: 0; }
  .url { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--accent); }
  .title { display: flex; flex-direction: column; gap: 2px; }
  .title b { font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wait { display: flex; align-items: center; gap: 10px; padding: 24px 4px; color: var(--muted); }
  .spin { width: 16px; height: 16px; border: 2px solid var(--line); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .list { flex: 1; min-height: 120px; overflow: auto; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); }
  .item { display: flex; align-items: center; gap: 8px; padding: 5px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); cursor: default; }
  .item.on { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .item .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item .size { font-variant-numeric: tabular-nums; white-space: nowrap; }
  .item .n { width: 28px; text-align: right; font-variant-numeric: tabular-nums; }
  .clip { border: 1px solid var(--line); border-radius: 4px; padding: 8px 10px; margin: 0; display: flex; flex-direction: column; gap: 8px; }
  .times label { display: flex; align-items: center; gap: 6px; }
  .time { width: 90px; flex: none !important; font-variant-numeric: tabular-nums; }
  .modes { flex-direction: column; align-items: flex-start; gap: 4px; }
  .tools { gap: 6px; }
  .subs { gap: 8px; }
  .langs { width: 110px; flex: none !important; }
  .small { min-width: 0; }
  .win-actions .left { flex: 1; }
</style>
