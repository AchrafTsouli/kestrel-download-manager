<script lang="ts" module>
  // One request per file type (or per finished program, which has its own logo), shared by every row.
  const cache = new Map<string, Promise<string | null>>();
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import { call, type Download } from "./api";

  let { d, size = 16 }: { d: Download; size?: number } = $props();
  const own = /\.(exe|msi|ico|lnk|scr|cpl)$/i;
  const key = $derived(d.status === "Completed" && d.exists && own.test(d.name) ? `file:${d.id}` : `type:${(d.name.match(/\.[^.\\/]+$/)?.[0] ?? "").toLowerCase()}`);
  let url: string | null = $state(null);
  $effect(() => {
    const k = key;
    if (!cache.has(k)) cache.set(k, call<string | null>("file_icon", { id: d.id }).catch(() => null));
    cache.get(k)!.then((u) => { if (k === key) url = u; });
  });
</script>

{#if url}<img src={url} alt="" width={size} height={size} draggable="false" />{:else}<Icon name={d.category} size={size - 1} />{/if}

<style>
  img { flex: none; image-rendering: auto; }
</style>
