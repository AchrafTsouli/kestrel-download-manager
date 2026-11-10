<script lang="ts">
  // Kestrel's own line icons (32-unit grid), drawn for Kestrel: toolbar actions, folders and file types.
  let { name, size = 32 }: { name: string; size?: number } = $props();
  const colors: Record<string, string> = { Compressed: "#a78bfa", Documents: "#60a5fa", Music: "#f472b6", Programs: "#38bdf8", Video: "#2dd4bf", Images: "#4ade80", Other: "#94a3b8" };
</script>

<svg width={size} height={size} viewBox="0 0 32 32" fill="none" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  {#if name === "add"}
    <g stroke="#2dd4bf"><path d="M16 5v22M5 16h22" /></g>
  {:else if name === "resume"}
    <g stroke="currentColor"><circle cx="16" cy="16" r="11" /><path d="M16 9.5v12M11 16.5l5 5 5-5" /></g>
  {:else if name === "stop"}
    <g stroke="currentColor"><circle cx="16" cy="16" r="11" /><rect x="11.5" y="11.5" width="9" height="9" rx="1.5" /></g>
  {:else if name === "stopAll"}
    <g stroke="#cbd5e1"><path d="M11.4 4h9.2L28 11.4v9.2L20.6 28h-9.2L4 20.6v-9.2Z" /><path d="M13 12v8M19 12v8" /></g>
  {:else if name === "delete"}
    <g stroke="#f87171"><path d="M6 9h20M12.5 9V6h7v3M8.5 9l1.4 17h12.2l1.4-17M13.5 13.5v8.5M18.5 13.5v8.5" /></g>
  {:else if name === "deleteDone"}
    <g stroke="#f472b6"><path d="M11 6h10v4H11z" /><path d="M9 8H7v19h18V8h-2" /><path d="M11.5 18l3 3 6-6.5" /></g>
  {:else if name === "options"}
    <g stroke="#a78bfa"><circle cx="16" cy="16" r="4" /><path d="M16 3.5v4M16 24.5v4M3.5 16h4M24.5 16h4M7.2 7.2l2.8 2.8M22 22l2.8 2.8M7.2 24.8 10 22M22 10l2.8-2.8" /><circle cx="16" cy="16" r="9" /></g>
  {:else if name === "scheduler"}
    <g stroke="#fb7185"><circle cx="16" cy="17.5" r="9.5" /><path d="M16 12v6l4 2.5M5.5 8.5 9 5M26.5 8.5 23 5M9.5 26l-2 2.5M22.5 26l2 2.5" /></g>
  {:else if name === "startQueue"}
    <g stroke="#38bdf8"><path d="M5 18v8h22v-8M5 18h6l2 3h6l2-3h6" /><path d="M16 4v11M11.5 10.5 16 15l4.5-4.5" /></g>
  {:else if name === "stopQueue"}
    <g stroke="#818cf8"><rect x="5" y="5" width="16" height="16" rx="2" /><rect x="11" y="11" width="16" height="16" rx="2" /></g>
  {:else if name === "share"}
    <g stroke="#c084fc"><circle cx="24" cy="8" r="3.5" /><circle cx="8" cy="16" r="3.5" /><circle cx="24" cy="24" r="3.5" /><path d="m11.2 14.3 9.6-4.6M11.2 17.7l9.6 4.6" /></g>
  {:else if name === "folder" || name === "folderOpen"}
    <path d="M3 9.5a2 2 0 0 1 2-2h7l2.5 3H27a2 2 0 0 1 2 2V25a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" fill="#e8b33c" stroke="#b9862a" stroke-width="1.2" />
    <path d="M3 14h26" stroke="#f6cc5c" stroke-width="1.6" />
  {:else if name === "queue"}
    <path d="M3 9.5a2 2 0 0 1 2-2h7l2.5 3H27a2 2 0 0 1 2 2V25a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" fill="#7dd3a8" stroke="#3f9e72" stroke-width="1.2" />
  {:else if name === "failed"}
    <g stroke="#f87171" stroke-width="2.4"><path d="M9 9l14 14M23 9 9 23" /></g>
  {:else}
    <!-- File-type badge: a page with the category's colour. -->
    <path d="M8 3.5h11l6 6V28.5H8Z" fill={colors[name] ?? "#94a3b8"} stroke="none" />
    <path d="M19 3.5v6h6" fill="#ffffff55" stroke="none" />
    <path d="M11.5 16h10M11.5 20h10M11.5 24h6" stroke="#ffffffcc" stroke-width="1.6" />
  {/if}
</svg>
