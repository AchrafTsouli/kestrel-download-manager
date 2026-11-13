<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { open as pickFile, save as saveFile } from "@tauri-apps/plugin-dialog";
  import { startDrag } from "@crabnebula/tauri-plugin-drag";
  import { onMount } from "svelte";
  import Icon from "./lib/Icon.svelte";
  import FileIcon from "./lib/FileIcon.svelte";
  import { call, categories, running, resumable, size, percent, timeLeft, stamp, statusText, statusClass, applyTheme, type Download, type Settings, type AfterQueue } from "./lib/api";

  let downloads: Download[] = $state.raw([]);
  let settings: Settings | null = $state(null);
  let runningQueues: string[] = $state([]);
  let selected: Set<string> = $state(new Set());
  let anchor: string | null = $state(null);
  let view = $state("all");
  let search = $state("");
  let menu: string | null = $state(null);
  let context: { x: number; y: number } | null = $state(null);
  let queueMenu = $state(false);
  let adding = $state(false);
  let address = $state("");
  let useAuth = $state(false);
  let login = $state("");
  let password = $state("");
  let addError = $state("");
  let confirmDelete: string[] | null = $state(null);
  let deleteFiles = $state(false);
  let renaming: { id: string; name: string } | null = $state(null);
  // Refresh address (an expired link), checksum, rename with a pattern, statistics.
  let refreshing = $state<{ id: string; name: string; url: string; error: string; busy: boolean } | null>(null);
  let summing = $state<{ id: string; name: string; algorithm: string; value: string; expected: string; busy: boolean; error: string } | null>(null);
  let patterning = $state<{ ids: string[]; pattern: string; error: string } | null>(null);
  let stats = $state(false);
  let about = $state(false);
  let shortcuts = $state(false);
  let submenu = $state<string | null>(null);
  type UpdateInfo = { current: string; version: string; available: boolean; signed: boolean; url: string };
  let update: UpdateInfo | null = $state(null);
  let installing = $state(false);
  // A crash last time: offered once as a GitHub issue the user reads and sends.
  let crash: { id: string; text: string } | null = $state(null);
  async function checkUpdates() {
    menu = null; message = "Checking for updates…";
    try {
      const info = await call<UpdateInfo>("check_updates");
      if (info.available) { update = info; message = ""; } else message = `Kestrel ${info.current} is up to date.`;
    } catch (e) { message = "Update check: " + String(e); }
  }
  async function installUpdate() {
    if (!update) return;
    if (!update.signed) { call("open_folder", { path: update.url }); update = null; return; }
    installing = true;
    try { await call("install_update"); } catch (e) { message = "Update: " + String(e); installing = false; update = null; }
  }
  let version = $state("");
  let countdown: { queue: string; action: AfterQueue; seconds: number } | null = $state(null);
  let showCategories = $state(true);
  let showToolbar = $state(true);
  let message = $state("Ready.");

  const queues = $derived(settings?.queues ?? []);
  // The list's columns, like IDM's; each can be filtered from its header.
  type Column = { key: string; label: string; width?: number; align?: string; text: (d: Download) => string };
  const columns: Column[] = [
    { key: "name", label: "File Name", text: (d) => d.name },
    { key: "q", label: "Q", width: 24, align: "c", text: () => "" },
    { key: "size", label: "Size", width: 86, align: "r", text: (d) => size(d.total) },
    { key: "status", label: "Status", width: 90, text: (d) => statusText(d) },
    { key: "left", label: "Time left", width: 80, align: "r", text: (d) => timeLeft(d) },
    { key: "rate", label: "Transfer rate", width: 96, align: "r", text: (d) => (d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : "") },
    { key: "last", label: "Last Try…", width: 118, text: (d) => stamp(d.lastTryMs) },
    { key: "added", label: "Added", width: 118, text: (d) => stamp(d.addedMs) },
    { key: "desc", label: "Description", width: 110, text: (d) => d.description },
  ];
  let filters: Record<string, string> = $state({});
  // Column widths dragged from the header edges, kept between sessions.
  let widths: Record<string, number> = $state((() => { try { return JSON.parse(localStorage.getItem("kestrel.columns") || "{}"); } catch { return {}; } })());
  const widthOf = (c: Column) => widths[c.key] ?? c.width;
  function startResize(e: PointerEvent, c: Column) {
    e.preventDefault(); e.stopPropagation();
    const th = (e.currentTarget as HTMLElement).closest("th") as HTMLElement;
    const from = e.clientX, start = th.offsetWidth;
    const move = (m: PointerEvent) => { widths[c.key] = Math.max(c.key === "name" ? 80 : 34, Math.round(start + m.clientX - from)); };
    const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); try { localStorage.setItem("kestrel.columns", JSON.stringify(widths)); } catch {} };
    window.addEventListener("pointermove", move); window.addEventListener("pointerup", up);
  }
  const tableWidth = $derived(columns.reduce((sum, c) => sum + (widthOf(c) ?? 150), 0));
  let filterOpen: string | null = $state(null);
  const shown = $derived.by(() => {
    const active = columns.filter((c) => filters[c.key]?.trim());
    if (!active.length) return visible;
    return visible.filter((d) => active.every((c) => c.text(d).toLowerCase().includes(filters[c.key].trim().toLowerCase())));
  });
  let gridHeight = $state(0);
  const ROW = 22;
  const fillerRows = $derived(Math.max(0, Math.ceil((gridHeight - 25) / ROW) - shown.length));
  // Only the rows in sight (and a few around) are drawn: a list of thousands scrolls like a short one.
  let gridWrap: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  const firstRow = $derived(Math.max(0, Math.floor(scrollTop / ROW) - 10));
  const lastRow = $derived(Math.min(shown.length, Math.ceil((scrollTop + gridHeight) / ROW) + 10));
  const rows = $derived(shown.slice(firstRow, lastRow));
  /** Scrolls the list so this download's row is in sight. */
  function reveal(id: string) {
    const i = shown.findIndex((d) => d.id === id);
    if (i < 0 || !gridWrap) return;
    const top = i * ROW, bottom = top + ROW + 25;
    if (top < gridWrap.scrollTop) gridWrap.scrollTop = top;
    else if (bottom > gridWrap.scrollTop + gridWrap.clientHeight) gridWrap.scrollTop = bottom - gridWrap.clientHeight;
  }
  const recentAddresses = $derived([...new Set([...downloads].sort((a, b) => b.addedMs - a.addedMs).map((d) => d.url))].slice(0, 20));
  function inView(d: Download, key: string) {
    if (key === "all") return true;
    if (key === "unfinished") return d.status !== "Completed";
    if (key === "finished") return d.status === "Completed";
    if (key === "failed") return d.status === "Failed";
    if (key.startsWith("type:")) return d.category === key.slice(5);
    if (key.startsWith("queue:")) return d.queue === key.slice(6) && d.status !== "Completed";
    return false;
  }
  const visible = $derived(downloads.filter((d) => inView(d, view) && (!search || (d.name + " " + d.description + " " + d.url).toLowerCase().includes(search.toLowerCase()))));
  const chosen = $derived(downloads.filter((d) => selected.has(d.id)));
  const canResume = $derived(chosen.some((d) => resumable(d.status)));
  const canStop = $derived(chosen.some((d) => running(d.status) || d.status === "Queued"));
  const active = $derived(downloads.filter((d) => running(d.status)));
  const totalSpeed = $derived(downloads.reduce((s, d) => s + d.bytesPerSecond, 0));
  const viewTitle = $derived(view === "all" ? "All Downloads" : view === "unfinished" ? "Unfinished" : view === "finished" ? "Finished" : view === "failed" ? "Failed" : view.replace(/^(type|queue):/, ""));
  // Every count of the folder tree in one pass over the list (the same rules as inView).
  const counts = $derived.by(() => {
    const c = new Map<string, number>();
    const add = (key: string) => c.set(key, (c.get(key) ?? 0) + 1);
    for (const d of downloads) {
      add("all"); add("type:" + d.category);
      if (d.status === "Completed") add("finished"); else { add("unfinished"); add("queue:" + d.queue); }
      if (d.status === "Failed") add("failed");
    }
    return c;
  });
  const count = (key: string) => counts.get(key) ?? 0;
  const currentQueue = $derived(view.startsWith("queue:") ? view.slice(6) : "Main download queue");

  // Puts changed downloads in place (new ones at the end), in one update of the list.
  function merge(changed: Download[]) {
    const next = downloads.slice();
    const at = new Map(next.map((d, i) => [d.id, i]));
    for (const d of changed) { const i = at.get(d.id); if (i === undefined) { at.set(d.id, next.length); next.push(d); } else next[i] = d; }
    downloads = next;
  }

  onMount(() => {
    call<Download[]>("list_downloads").then((list) => (downloads = list));
    call<Settings>("get_settings").then((s) => { settings = s; applyTheme(s.theme); });
    call<string[]>("running_queues").then((q) => (runningQueues = q));
    call<string>("app_version").then((v) => (version = v));
    call<{ id: string; text: string }[]>("crash_reports").then((list) => (crash = list[0] ?? null));
    const subscriptions = [
      listen<Download>("download", ({ payload }) => merge([payload])),
      // Progress of everything running, together, 4 times a second.
      listen<Download[]>("downloads", ({ payload }) => merge(payload)),
      listen<string[]>("removed", ({ payload }) => {
        downloads = downloads.filter((d) => !payload.includes(d.id));
        selected = new Set([...selected].filter((id) => !payload.includes(id)));
      }),
      listen<Settings>("settings", ({ payload }) => { settings = payload; applyTheme(payload.theme); }),
      listen<string[]>("queues", ({ payload }) => (runningQueues = payload)),
      listen<string>("message", ({ payload }) => (message = payload)),
      listen<UpdateInfo>("update-available", ({ payload }) => (update = payload)),
      listen<string>("completed", ({ payload }) => { const d = downloads.find((x) => x.id === payload); if (d) message = "Saved " + d.path; }),
      listen<[string, string]>("failed", ({ payload }) => { const d = downloads.find((x) => x.id === payload[0]); message = (d ? d.name + ": " : "") + payload[1]; }),
      listen("open-add", () => openAdd()),
      listen<[string, AfterQueue]>("queue-finished", ({ payload }) => startCountdown(payload[0], payload[1])),
    ];
    const close = (e: MouseEvent) => { if (!(e.target as HTMLElement).closest(".filter-pop, .funnel")) filterOpen = null; if (!(e.target as HTMLElement).closest(".menubar, .context")) { menu = null; context = null; queueMenu = false; } };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", keys);
    return () => { subscriptions.forEach((s) => s.then((f) => f())); window.removeEventListener("mousedown", close); window.removeEventListener("keydown", keys); };
  });

  function keys(e: KeyboardEvent) {
    const dialog = adding || confirmDelete || about || shortcuts || renaming || countdown || refreshing || summing || patterning || stats;
    if (dialog) { if (e.key === "Escape") { adding = false; confirmDelete = null; about = false; shortcuts = false; renaming = null; refreshing = null; summing = null; patterning = null; stats = false; } return; }
    if (e.key === "f" && e.ctrlKey) { e.preventDefault(); findBox(); return; }
    if (e.key === "F3") { e.preventDefault(); findNext(); return; }
    if (e.ctrlKey && (e.key === "+" || e.key === "=" || e.key === "-" || e.key === "0")) { e.preventDefault(); zoom(e.key === "0" ? 0 : e.key === "-" ? -1 : 1); return; }
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    if (e.key === "Delete" && selected.size) askDelete([...selected]);
    else if (e.key === "Enter" && chosen.length === 1) activate(chosen[0]);
    else if (e.key === "a" && e.ctrlKey) { e.preventDefault(); selected = new Set(visible.map((d) => d.id)); }
    else if (e.key === "n" && e.ctrlKey) { e.preventDefault(); openAdd(); }
    else if (e.key === "Escape") { menu = null; context = null; }
  }

  function select(d: Download, e: MouseEvent) {
    if (e.shiftKey && anchor) {
      const ids = visible.map((x) => x.id);
      const [a, b] = [ids.indexOf(anchor), ids.indexOf(d.id)].sort((x, y) => x - y);
      selected = new Set(ids.slice(a, b + 1));
    } else if (e.ctrlKey) {
      const next = new Set(selected); next.has(d.id) ? next.delete(d.id) : next.add(d.id); selected = next; anchor = d.id;
    } else { selected = new Set([d.id]); anchor = d.id; }
  }
  function rowMenu(d: Download, e: MouseEvent) {
    e.preventDefault();
    if (!selected.has(d.id)) { selected = new Set([d.id]); anchor = d.id; }
    context = { x: Math.min(e.clientX, innerWidth - 230), y: Math.min(e.clientY, innerHeight - 330) };
    queueMenu = false;
  }
  // Double-click: a finished file opens; anything else shows its progress window.
  const activate = (d: Download) => d.status === "Completed" ? openFile(d) : call("show_progress", { id: d.id });

  function openAdd() {
    adding = true; addError = ""; address = ""; useAuth = false; login = ""; password = ""; menu = null;
    navigator.clipboard?.readText?.().then((t) => {
      t = t.trim();
      if (!/^(https?:\/\/|magnet:\?)\S+$/i.test(t) || address) return;
      address = t;
      requestAnimationFrame(() => (document.getElementById("address") as HTMLInputElement | null)?.select());
    }).catch(() => {});
  }
  async function add(event: SubmitEvent) {
    event.preventDefault();
    const url = address.trim();
    const existing = downloads.find((d) => d.url === url);
    if (existing && existing.status !== "Failed") { addError = `This address is already in the list as ${existing.name}.`; selected = new Set([existing.id]); return; }
    try { await call("offer_url", { url, login: useAuth ? login : null, password: useAuth ? password : null }); adding = false; } catch (error) { addError = String(error); }
  }
  async function openTorrentFile() {
    menu = null;
    const path = await pickFile({ title: "Open torrent file", filters: [{ name: "Torrent files", extensions: ["torrent"] }] });
    if (typeof path === "string") call("open_torrent_file", { path }).catch((e) => (message = String(e)));
  }
  // A finished row dragged a few pixels leaves Kestrel as the file itself (to Explorer, a mail…).
  let dragFrom: { d: Download; x: number; y: number } | null = null;
  function rowDown(d: Download, e: MouseEvent) {
    if (e.button !== 0) return;
    select(d, e);
    dragFrom = d.status === "Completed" && d.exists ? { d, x: e.clientX, y: e.clientY } : null;
  }
  async function rowMove(e: MouseEvent) {
    if (!dragFrom || !(e.buttons & 1) || Math.hypot(e.clientX - dragFrom.x, e.clientY - dragFrom.y) < 6) return;
    const d = dragFrom.d;
    dragFrom = null;
    try { const item = await call<{ path: string; icon: string }>("drag_item", { id: d.id }); await startDrag({ item: [item.path], icon: item.icon }); }
    catch (err) { message = String(err); }
  }
  const ids = () => [...selected];
  const resume = () => call("resume_downloads", { ids: ids() });
  const stop = () => call("pause_downloads", { ids: ids() });
  function askDelete(list: string[]) {
    if (!list.length) return;
    menu = null; context = null;
    // Options > General > "Confirm before deleting downloads" off: removed from the list at once (files kept).
    if (settings && !settings.confirmDelete) { call("remove_downloads", { ids: list, deleteFiles: false }); return; }
    confirmDelete = list; deleteFiles = false;
  }
  async function doDelete() { if (confirmDelete) await call("remove_downloads", { ids: confirmDelete, deleteFiles }); confirmDelete = null; }
  async function deleteCompleted() { const removed = await call<string[]>("remove_completed"); message = `Removed ${removed.length} completed download${removed.length === 1 ? "" : "s"} from the list.`; menu = null; }
  async function openFile(d: Download) { try { await call("open_file", { id: d.id }); } catch (e) { message = String(e); } }
  async function showFolder(d: Download) { try { await call("show_in_folder", { id: d.id }); } catch (e) { message = String(e); } }
  // ---- Refresh address, checksum, pattern rename, statistics
  const canRefresh = (d?: Download) => !!d && d.kind === "file" && d.status !== "Completed" && !running(d.status);
  function openRefresh(d: Download) { menu = null; context = null; refreshing = { id: d.id, name: d.name, url: d.url, error: "", busy: false }; }
  async function doRefresh(e: SubmitEvent) {
    e.preventDefault(); if (!refreshing) return;
    refreshing.busy = true; refreshing.error = "";
    try { await call("refresh_address", { id: refreshing.id, url: refreshing.url }); message = `New address for ${refreshing.name}. Resume it to continue.`; refreshing = null; }
    catch (err) { if (refreshing) { refreshing.error = String(err); refreshing.busy = false; } }
  }
  function openChecksum(d: Download) { menu = null; context = null; summing = { id: d.id, name: d.name, algorithm: "sha256", value: "", expected: "", busy: false, error: "" }; computeChecksum(); }
  async function computeChecksum() {
    if (!summing) return;
    const s = summing; s.busy = true; s.value = ""; s.error = "";
    try { s.value = await call<string>("file_checksum", { id: s.id, algorithm: s.algorithm }); } catch (err) { s.error = String(err); }
    s.busy = false;
  }
  const checksumVerdict = $derived(!summing || !summing.value || !summing.expected.trim() ? "" : summing.expected.trim().toLowerCase() === summing.value ? "match" : "differ");
  function openPattern() { menu = null; context = null; const list = chosen.filter((d) => !running(d.status) && d.kind !== "torrent").map((d) => d.id); if (list.length) patterning = { ids: list, pattern: "{name}", error: "" }; }
  async function doPattern(e: SubmitEvent) {
    e.preventDefault(); if (!patterning) return;
    const results = await call<({ Ok: string } | { Err: string })[]>("rename_with_pattern", { ids: patterning.ids, pattern: patterning.pattern });
    const failed = results.filter((r) => "Err" in r) as { Err: string }[];
    if (failed.length) { patterning.error = failed[0].Err; return; }
    message = `Renamed ${results.length} download${results.length === 1 ? "" : "s"}.`; patterning = null;
  }
  const patternPreview = $derived(patterning ? downloads.filter((d) => patterning!.ids.includes(d.id)).slice(0, 3).map((d, i) => {
    const dot = d.name.lastIndexOf("."); const stem = dot > 0 ? d.name.slice(0, dot) : d.name; const ext = dot > 0 ? d.name.slice(dot + 1) : "";
    const day = new Date(d.addedMs); const date = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`;
    let site = ""; try { site = new URL(d.url).hostname.replace(/^www\./, ""); } catch {}
    const width = Math.max(2, String(patterning!.ids.length).length);
    let out = patterning!.pattern.replaceAll("{name}", stem).replaceAll("{n}", String(i + 1).padStart(width, "0")).replaceAll("{date}", date).replaceAll("{site}", site).replaceAll("{ext}", ext);
    if (!patterning!.pattern.includes("{ext}") && ext) out += "." + ext;
    return out;
  }) : []);
  // Statistics: what finished, per day (last 14 days) and per month (last 12 months).
  const statistics = $derived.by(() => {
    if (!stats) return null;
    const done = downloads.filter((d) => d.status === "Completed" && d.completedMs);
    const dayKey = (t: number) => { const d = new Date(t); return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };
    const monthKey = (t: number) => { const d = new Date(t); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`; };
    const days: { label: string; bytes: number; files: number }[] = [];
    for (let i = 13; i >= 0; i--) days.push({ label: dayKey(Date.now() - i * 86400000), bytes: 0, files: 0 });
    const months: { label: string; bytes: number; files: number }[] = [];
    for (let i = 11; i >= 0; i--) { const d = new Date(); d.setDate(1); d.setMonth(d.getMonth() - i); months.push({ label: monthKey(d.getTime()), bytes: 0, files: 0 }); }
    const from = Date.now() - 14 * 86400000;
    for (const d of done) {
      const bytes = d.total ?? d.written;
      const day = d.completedMs! >= from ? days.find((x) => x.label === dayKey(d.completedMs!)) : undefined;
      if (day) { day.bytes += bytes; day.files++; }
      const month = months.find((x) => x.label === monthKey(d.completedMs!));
      if (month) { month.bytes += bytes; month.files++; }
    }
    const total = done.reduce((s, d) => s + (d.total ?? d.written), 0);
    return { days, months, total, files: done.length, dayMax: Math.max(1, ...days.map((x) => x.bytes)), monthMax: Math.max(1, ...months.map((x) => x.bytes)) };
  });
  // ---- Settings export and import (passwords are never written to the file)
  async function exportSettings() {
    menu = null;
    const path = await saveFile({ title: "Export settings", defaultPath: "kestrel-settings.json", filters: [{ name: "Kestrel settings", extensions: ["json"] }] });
    if (typeof path !== "string") return;
    try { await call("export_settings", { path }); message = "Settings exported (without passwords)."; } catch (e) { message = String(e); }
  }
  async function importSettings() {
    menu = null;
    const path = await pickFile({ title: "Import settings", filters: [{ name: "Kestrel settings", extensions: ["json"] }] });
    if (typeof path !== "string") return;
    try { await call("import_settings", { path }); } catch (e) { message = String(e); }
  }

  async function doRename(e: SubmitEvent) { e.preventDefault(); if (!renaming) return; try { await call("rename_download", renaming); renaming = null; } catch (err) { message = String(err); renaming = null; } }
  const showWindow = (name: string) => { call("show_window", { name }); menu = null; };
  async function toggleQueue(name: string) {
    if (runningQueues.includes(name)) { await call("stop_queue", { name }); message = `${name} stopped.`; }
    else { await call("start_queue", { name }); message = `${name} started.`; }
  }
  function startCountdown(queue: string, action: AfterQueue) {
    if (action === "nothing") return;
    countdown = { queue, action, seconds: 60 };
    const timer = setInterval(() => {
      if (!countdown) { clearInterval(timer); return; }
      countdown.seconds -= 1;
      if (countdown.seconds <= 0) { clearInterval(timer); const a = countdown.action; countdown = null; call("power_action", { action: a }); }
    }, 1000);
  }
  const actionName: Record<string, string> = { exit: "Kestrel will close", sleep: "The computer will sleep", hibernate: "The computer will hibernate", shutdown: "The computer will shut down" };

  type MenuItem = { label: string; run?: () => void; disabled?: () => boolean; check?: () => boolean; key?: string; sep?: boolean; sub?: () => MenuItem[] };
  const HOME = "https://github.com/AchrafTsouli/kestrel-download-manager";
  const web = (url: string) => { call("open_folder", { path: url }); menu = null; };
  const linkPattern = /^((https?|ftps?|sftp):\/\/|magnet:\?)\S+$/i;
  // Opens the batch window with these links already filled in (the window reads them once).
  function batchWith(links: string[]) {
    try { localStorage.setItem("kestrel.batch", links.join("\n")); } catch {}
    showWindow("batch");
  }
  async function batchFromClipboard() {
    menu = null;
    const text = await navigator.clipboard?.readText?.().catch(() => "") ?? "";
    const links = text.split(/\s+/).filter((l) => linkPattern.test(l));
    if (!links.length) { message = "The clipboard holds no download links."; return; }
    batchWith(links);
  }
  async function exportList(idm: boolean) {
    menu = null;
    const path = await saveFile({ title: idm ? "Export to IDM export file" : "Export to text file", defaultPath: idm ? "downloads.ef2" : "downloads.txt", filters: idm ? [{ name: "IDM export file", extensions: ["ef2"] }] : [{ name: "Text file", extensions: ["txt"] }] });
    if (typeof path !== "string") return;
    const list = (selected.size > 1 ? chosen : downloads).map((d) => d.url);
    try { await call("write_link_list", { path, urls: list, idm }); message = `Exported ${list.length} link${list.length === 1 ? "" : "s"}.`; } catch (e) { message = String(e); }
  }
  async function importList(idm: boolean) {
    menu = null;
    const path = await pickFile({ title: idm ? "Import from IDM export file" : "Import from text file", filters: idm ? [{ name: "IDM export file", extensions: ["ef2"] }] : [{ name: "Text file", extensions: ["txt", "lst", "csv"] }, { name: "All files", extensions: ["*"] }] });
    if (typeof path !== "string") return;
    try {
      const links = await call<string[]>("read_link_list", { path });
      if (links.length) batchWith(links); else message = "No download links in that file.";
    } catch (e) { message = String(e); }
  }
  // Downloads > Find: the search box; Find next (F3): the next listed download matching it.
  const findBox = () => { menu = null; const box = document.querySelector<HTMLInputElement>("input.search"); box?.focus(); box?.select(); };
  function findNext() {
    menu = null;
    if (!search.trim()) { findBox(); return; }
    if (!visible.length) { message = `Nothing matches “${search}”.`; return; }
    const at = visible.findIndex((d) => selected.has(d.id));
    const next = visible[(at + 1) % visible.length];
    selected = new Set([next.id]); anchor = next.id;
    reveal(next.id);
  }
  // Speed limiter: on/off keeps the last limit chosen in Options.
  function limiter(on: boolean) {
    if (!settings) return;
    const speedLimit = on ? (settings.lastSpeedLimit || 1024 * 1024) : 0;
    call("save_settings", { settings: { ...settings, speedLimit } });
    message = on ? `Speed limiter on: ${size(speedLimit)}/s for all downloads.` : "Speed limiter off.";
  }
  const sizes = [80, 90, 100, 110, 125, 150, 175, 200];
  function zoom(step: number) {
    if (!settings) return;
    const at = sizes.indexOf(Number(settings.interfaceSize) || 100);
    const interfaceSize = step === 0 ? 100 : sizes[Math.min(sizes.length - 1, Math.max(0, (at < 0 ? 2 : at) + step))];
    call("save_settings", { settings: { ...settings, interfaceSize } });
  }
  const queueItems = (starting: boolean): MenuItem[] => queues.length
    ? queues.map((q: { name: string }) => ({ label: q.name, run: () => toggleQueue(q.name), disabled: () => runningQueues.includes(q.name) !== !starting }))
    : [{ label: "No queues", disabled: () => true }];
  const tellFriend = () => { navigator.clipboard?.writeText(`Kestrel, a free download manager: ${HOME}`); message = "Link to Kestrel copied. Paste it to a friend."; menu = null; };

  const menus: Record<string, MenuItem[]> = {
    Tasks: [
      { label: "Add new download", key: "Ctrl+N", run: openAdd },
      { label: "Open torrent file…", run: openTorrentFile },
      { label: "Add magnet link…", run: openAdd },
      { label: "Torrent settings…", run: () => showWindow("options") },
      { label: "Add batch download", sep: true, run: () => showWindow("batch") },
      { label: "Add batch download from clipboard", run: batchFromClipboard },
      { label: "Download video from a page…", sep: true, run: openAdd },
      { label: "Link grabber…", run: () => showWindow("grabber") },
      { label: "Export", sep: true, sub: () => [
        { label: "To IDM export file…", run: () => exportList(true) },
        { label: "To text file…", run: () => exportList(false) },
        { label: "Settings…", sep: true, run: exportSettings },
      ] },
      { label: "Import", sub: () => [
        { label: "From IDM export file…", run: () => importList(true) },
        { label: "From text file…", run: () => importList(false) },
        { label: "Settings…", sep: true, run: importSettings },
      ] },
      { label: "Exit", sep: true, run: () => call("exit_app") },
    ],
    File: [
      { label: "Open", key: "Enter", run: () => chosen[0] && openFile(chosen[0]), disabled: () => chosen.length !== 1 || chosen[0].status !== "Completed" },
      { label: "Open with…", run: () => chosen[0] && call("open_with", { id: chosen[0].id }).catch((e) => (message = String(e))), disabled: () => chosen.length !== 1 || chosen[0].status !== "Completed" },
      { label: "Open folder", run: () => chosen[0] && showFolder(chosen[0]), disabled: () => chosen.length !== 1 },
      { label: "Show progress", run: () => chosen[0] && call("show_progress", { id: chosen[0].id }), disabled: () => chosen.length !== 1 },
      { label: "Copy address", run: () => { navigator.clipboard?.writeText(chosen.map((d) => d.url).join("\n")); menu = null; }, disabled: () => !chosen.length },
      { label: "Stop download", sep: true, run: stop, disabled: () => !canStop },
      { label: "Download now", run: resume, disabled: () => !canResume },
      { label: "Redownload", run: () => chosen[0] && call("redownload", { id: chosen[0].id }), disabled: () => chosen.length !== 1 },
      { label: "Rename", run: () => chosen[0] && (renaming = { id: chosen[0].id, name: chosen[0].name }), disabled: () => chosen.length !== 1 || running(chosen[0].status) },
      { label: "Rename with a pattern…", run: openPattern, disabled: () => !chosen.some((d) => !running(d.status) && d.kind !== "torrent") },
      { label: "Refresh address…", run: () => chosen[0] && openRefresh(chosen[0]), disabled: () => chosen.length !== 1 || !canRefresh(chosen[0]) },
      { label: "Checksum…", run: () => chosen[0] && openChecksum(chosen[0]), disabled: () => chosen.length !== 1 || chosen[0].status !== "Completed" },
      { label: "Remove", key: "Del", sep: true, run: () => askDelete(ids()), disabled: () => selected.size === 0 },
      { label: "Remove with files", run: () => { const list = ids(); menu = null; if (list.length) { confirmDelete = list; deleteFiles = true; } }, disabled: () => selected.size === 0 },
    ],
    Downloads: [
      { label: "Pause all", run: () => call("pause_all") },
      { label: "Stop all", run: () => call("pause_all"), disabled: () => !downloads.some((d) => running(d.status)) },
      { label: "Resume all", run: () => call("resume_all") },
      { label: "Delete all completed", run: deleteCompleted, disabled: () => !downloads.some((d) => d.status === "Completed") },
      { label: "Find", key: "Ctrl+F", sep: true, run: findBox },
      { label: "Find next", key: "F3", run: findNext },
      { label: "Select all", key: "Ctrl+A", run: () => { selected = new Set(visible.map((d) => d.id)); menu = null; } },
      { label: "Scheduler", sep: true, run: () => showWindow("scheduler") },
      { label: "Start queue", sub: () => queueItems(true) },
      { label: "Stop queue", sub: () => queueItems(false) },
      { label: "Speed limiter", sep: true, sub: () => [
        { label: "Turn on", check: () => (settings?.speedLimit ?? 0) > 0, run: () => limiter(true), disabled: () => (settings?.speedLimit ?? 0) > 0 },
        { label: "Turn off", run: () => limiter(false), disabled: () => !settings?.speedLimit },
        { label: "Settings…", sep: true, run: () => showWindow("options") },
      ] },
      { label: "Site grabber…", run: () => showWindow("grabber") },
      { label: "Options", sep: true, run: () => showWindow("options") },
    ],
    View: [
      { label: "Categories", check: () => showCategories, run: () => (showCategories = !showCategories) },
      { label: "Toolbar", check: () => showToolbar, run: () => (showToolbar = !showToolbar) },
      { label: "Dark mode", sep: true, check: () => settings?.theme !== "light", run: () => { if (settings) call("save_settings", { settings: { ...settings, theme: settings.theme === "light" ? "dark" : "light" } }); } },
      { label: "Interface size", sub: () => [
        { label: "Larger", key: "Ctrl++", run: () => zoom(1) },
        { label: "Smaller", key: "Ctrl+-", run: () => zoom(-1) },
        { label: `Normal (now ${settings?.interfaceSize ?? 100} %)`, key: "Ctrl+0", run: () => zoom(0) },
      ] },
      { label: "Language…", run: () => showWindow("options") },
      { label: "Statistics…", sep: true, run: () => { stats = true; menu = null; } },
    ],
    Help: [
      { label: "Help contents", run: () => web(`${HOME}#readme`) },
      { label: "Add the browser extension…", run: () => showWindow("extension") },
      { label: "Keyboard shortcuts", run: () => { shortcuts = true; menu = null; } },
      { label: "Home page", sep: true, run: () => web(HOME) },
      { label: "Report a problem", run: () => web(`${HOME}/issues/new`) },
      { label: "Open log", run: () => { menu = null; call("open_log").catch((e) => (message = String(e))); } },
      { label: "Check for updates…", run: checkUpdates },
      { label: "Tell a friend", sep: true, run: tellFriend },
      { label: "About Kestrel", run: () => { about = true; menu = null; } },
    ],
  };
</script>

<div class="window">
  <div class="menubar" role="menubar">
    {#each Object.keys(menus) as name}
      <div class="menu-root">
        <button class="menu-title" class:open={menu === name} onclick={() => (menu = menu === name ? null : name)} onmouseenter={() => menu && (menu = name)}>{name}</button>
        {#if menu === name}
          <div class="dropdown" role="menu">
            {#each menus[name] as item}
              {#if item.sep}<div class="sep"></div>{/if}
              {#if item.sub}
                <div class="sub-root" role="none" onmouseenter={() => (submenu = item.label)} onmouseleave={() => (submenu = null)}>
                  <button role="menuitem" aria-haspopup="menu" class="menu-item" class:hot={submenu === item.label} onclick={() => (submenu = item.label)}>
                    <span class="tick"></span>{item.label}<span class="key">▸</span>
                  </button>
                  {#if submenu === item.label}
                    <div class="dropdown flyout" role="menu">
                      {#each item.sub() as child}
                        {#if child.sep}<div class="sep"></div>{/if}
                        <button role="menuitem" class="menu-item" disabled={child.disabled?.()} onclick={() => { child.run?.(); menu = null; submenu = null; }}>
                          <span class="tick">{child.check?.() ? "✓" : ""}</span>{child.label}<span class="key">{child.key ?? ""}</span>
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              {:else}
                <button role="menuitem" class="menu-item" disabled={item.disabled?.()} onmouseenter={() => (submenu = null)} onclick={() => { item.run?.(); if (name !== "View") menu = null; }}>
                  <span class="tick">{item.check?.() ? "✓" : ""}</span>{item.label}<span class="key">{item.key ?? ""}</span>
                </button>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    <div class="brand"><img src="/logo.png" alt="" width="18" height="18" /><span>KESTREL</span></div>
  </div>

  {#if showToolbar}
    <div class="toolbar" role="toolbar">
      <button class="tool" onclick={openAdd}><Icon name="add" /><span>Add URL</span></button>
      <button class="tool" onclick={resume} disabled={!canResume}><Icon name="resume" /><span>Resume</span></button>
      <button class="tool" onclick={stop} disabled={!canStop}><Icon name="stop" /><span>Stop</span></button>
      <button class="tool" onclick={() => call("pause_all")} disabled={active.length === 0 && !downloads.some((d) => d.status === "Queued")}><Icon name="stopAll" /><span>Stop All</span></button>
      <button class="tool" onclick={() => askDelete(ids())} disabled={selected.size === 0}><Icon name="delete" /><span>Delete</span></button>
      <button class="tool" onclick={deleteCompleted} disabled={!downloads.some((d) => d.status === "Completed")}><Icon name="deleteDone" /><span>Delete Co…</span></button>
      <button class="tool" onclick={() => showWindow("options")}><Icon name="options" /><span>Options</span></button>
      <button class="tool" onclick={() => showWindow("scheduler")}><Icon name="scheduler" /><span>Scheduler</span></button>
      <button class="tool" onclick={() => toggleQueue(currentQueue)} disabled={runningQueues.includes(currentQueue)} title={"Start " + currentQueue}><Icon name="startQueue" /><span>Start Qu…</span></button>
      <button class="tool" onclick={() => toggleQueue(currentQueue)} disabled={!runningQueues.includes(currentQueue)} title={"Stop " + currentQueue}><Icon name="stopQueue" /><span>Stop Qu…</span></button>
      <button class="tool" onclick={() => { navigator.clipboard?.writeText("Kestrel, a free download manager: https://github.com/AchrafTsouli/kestrel-download-manager"); message = "Link to Kestrel copied. Paste it to a friend."; }}><Icon name="share" /><span>Tell a Friend</span></button>
    </div>
  {/if}

  <div class="main" style:grid-template-columns={showCategories ? "220px 1fr" : "1fr"}>
    {#if showCategories}
      <nav class="tree" aria-label="Categories">
        <div class="pane-title">Categories <button class="pane-close" onclick={() => (showCategories = false)} aria-label="Hide categories">×</button></div>
        <button class="node" class:on={view === "all"} onclick={() => (view = "all")}><span class="twisty">▾</span><Icon name="folder" size={16} />All Downloads<small>{count("all") || ""}</small></button>
        {#each categories as type}
          <button class="node child" class:on={view === "type:" + type} onclick={() => (view = "type:" + type)}><Icon name={type} size={15} />{type}<small>{count("type:" + type) || ""}</small></button>
        {/each}
        <button class="node" class:on={view === "unfinished"} onclick={() => (view = "unfinished")}><span class="twisty"></span><Icon name="folder" size={16} />Unfinished<small>{count("unfinished") || ""}</small></button>
        <button class="node" class:on={view === "failed"} onclick={() => (view = "failed")}><span class="twisty"></span><Icon name="failed" size={16} />Failed<small>{count("failed") || ""}</small></button>
        <button class="node" class:on={view === "finished"} onclick={() => (view = "finished")}><span class="twisty"></span><Icon name="folder" size={16} />Finished<small>{count("finished") || ""}</small></button>
        <div class="node static"><span class="twisty">▾</span><Icon name="folder" size={16} />Queues</div>
        {#each queues as q}
          <button class="node child" class:on={view === "queue:" + q.name} onclick={() => (view = "queue:" + q.name)} ondblclick={() => showWindow("scheduler")} title={runningQueues.includes(q.name) ? "Running" : "Stopped"}>
            <Icon name="queue" size={15} />{q.name}{#if runningQueues.includes(q.name)}<span class="live" aria-label="running"></span>{/if}<small>{count("queue:" + q.name) || ""}</small>
          </button>
        {/each}
      </nav>
    {/if}

    <section class="listpane">
      <div class="list-head">
        <h1>{viewTitle}</h1>
        <input class="search" type="search" placeholder="Search downloads…" bind:value={search} aria-label="Search downloads" />
      </div>
      <div class="grid-wrap" bind:clientHeight={gridHeight} bind:this={gridWrap} onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}>
        <table class="grid" style:min-width="{tableWidth}px">
          <colgroup>{#each columns as c}<col style:width={widthOf(c) ? widthOf(c) + "px" : null} />{/each}</colgroup>
          <thead>
            <tr>
              {#each columns as c}
                <th class={c.align}>
                  <span class="th">
                    <span class="th-label">{c.label}</span>
                    {#if c.key !== "q"}
                      <button class="funnel" class:on={!!filters[c.key]} onclick={(e) => { e.stopPropagation(); filterOpen = filterOpen === c.key ? null : c.key; }} title={filters[c.key] ? `Filtered: ${filters[c.key]}` : `Filter by ${c.label}`} aria-label={`Filter by ${c.label}`}>
                        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0.5 1h9L6 5.2V9L4 8V5.2z" /></svg>
                      </button>
                    {/if}
                  </span>
                  <span class="resize" role="separator" aria-orientation="vertical" aria-label={`Resize ${c.label}`} onpointerdown={(e) => startResize(e, c)} ondblclick={() => { delete widths[c.key]; try { localStorage.setItem("kestrel.columns", JSON.stringify(widths)); } catch {} }}></span>
                  {#if filterOpen === c.key}
                    <div class="filter-pop" role="dialog" aria-label={`Filter by ${c.label}`} onmousedown={(e) => e.stopPropagation()}>
                      <!-- svelte-ignore a11y_autofocus -->
                      <input class="input" placeholder="Contains…" bind:value={filters[c.key]} autofocus onkeydown={(e) => (e.key === "Enter" || e.key === "Escape") && (filterOpen = null)} />
                      <button class="btn small" onclick={() => { filters[c.key] = ""; filterOpen = null; }}>Clear</button>
                    </div>
                  {/if}
                </th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#if firstRow > 0}<tr class="spacer" aria-hidden="true"><td colspan={columns.length} style:height="{firstRow * ROW}px"></td></tr>{/if}
            {#each rows as d, i (d.id)}
              <tr class:sel={selected.has(d.id)} class:alt={(firstRow + i) % 2 === 1} onmousedown={(e) => rowDown(d, e)} onmousemove={rowMove} onmouseup={() => (dragFrom = null)} ondblclick={() => activate(d)} oncontextmenu={(e) => rowMenu(d, e)} title={d.error ?? d.url}>
                <td class="name"><FileIcon {d} /><span>{d.name}</span></td>
                <td class="c">{d.status === "Queued" ? "⏱" : ""}</td>
                <td class="r">{size(d.total)}</td>
                <td><span class="status {statusClass(d)}">{statusText(d)}</span></td>
                <td class="r">{timeLeft(d)}</td>
                <td class="r">{d.bytesPerSecond > 0 ? size(d.bytesPerSecond) + "/s" : ""}</td>
                <td>{stamp(d.lastTryMs)}</td>
                <td>{stamp(d.addedMs)}</td>
                <td class="desc">{d.description}</td>
              </tr>
            {/each}
            {#if lastRow < shown.length}<tr class="spacer" aria-hidden="true"><td colspan={columns.length} style:height="{(shown.length - lastRow) * ROW}px"></td></tr>{/if}
            <!-- Like IDM: the empty grid continues below the downloads. -->
            {#each { length: fillerRows } as _, j}
              <tr class="filler" class:alt={(shown.length + j) % 2 === 1} aria-hidden="true">{#each columns as _c}<td></td>{/each}</tr>
            {/each}
          </tbody>
        </table>
        {#if shown.length === 0}
          <div class="empty">{downloads.length === 0 ? "No downloads yet. Click Add URL, or press Ctrl+N, and paste a link." : "Nothing in this view."}</div>
        {/if}
      </div>
    </section>
  </div>

  <footer class="statusbar">
    <span class="msg">{message}</span>
    <span class="totals">Total: <b>{downloads.length}</b>&nbsp;&nbsp; Active: <b>{active.length}</b>{totalSpeed > 0 ? ` · ${size(totalSpeed)}/s` : ""}&nbsp;&nbsp; Finished: <b>{count("finished")}</b></span>
  </footer>
</div>

{#if context}
  <div class="context dropdown" style:left="{context.x}px" style:top="{context.y}px" role="menu">
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status !== "Completed"} onclick={() => { openFile(chosen[0]); context = null; }}><span class="tick"></span>Open</button>
    <button class="menu-item" disabled={chosen.length !== 1} onclick={() => { showFolder(chosen[0]); context = null; }}><span class="tick"></span>Open folder</button>
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status === "Completed"} onclick={() => { call("show_progress", { id: chosen[0].id }); context = null; }}><span class="tick"></span>Show progress</button>
    <div class="sep"></div>
    <button class="menu-item" disabled={!canResume} onclick={() => { resume(); context = null; }}><span class="tick"></span>Resume</button>
    <button class="menu-item" disabled={!canStop} onclick={() => { stop(); context = null; }}><span class="tick"></span>Stop</button>
    <button class="menu-item" disabled={!canResume} onclick={() => { call("enqueue", { ids: ids() }); context = null; }}><span class="tick"></span>Download later (queue)</button>
    <button class="menu-item submenu" onmouseenter={() => (queueMenu = true)} onclick={() => (queueMenu = !queueMenu)}><span class="tick"></span>Move to queue<span class="key">▸</span></button>
    {#if queueMenu}
      <div class="dropdown sub" role="menu">
        {#each queues as q}<button class="menu-item" onclick={() => { call("set_queue", { ids: ids(), queue: q.name }); context = null; }}><span class="tick">{chosen.every((d) => d.queue === q.name) ? "✓" : ""}</span>{q.name}</button>{/each}
      </div>
    {/if}
    <div class="sep"></div>
    <button class="menu-item" disabled={chosen.length !== 1 || running(chosen[0].status)} onclick={() => { renaming = { id: chosen[0].id, name: chosen[0].name }; context = null; }}><span class="tick"></span>Rename</button>
    <button class="menu-item" disabled={!chosen.some((d) => !running(d.status) && d.kind !== "torrent")} onclick={openPattern}><span class="tick"></span>Rename with a pattern…</button>
    <button class="menu-item" disabled={chosen.length !== 1 || !canRefresh(chosen[0])} onclick={() => openRefresh(chosen[0])}><span class="tick"></span>Refresh address…</button>
    <button class="menu-item" disabled={chosen.length !== 1 || chosen[0].status !== "Completed"} onclick={() => openChecksum(chosen[0])}><span class="tick"></span>Checksum…</button>
    <button class="menu-item" disabled={chosen.length !== 1} onclick={() => { call("redownload", { id: chosen[0].id }); context = null; }}><span class="tick"></span>Redownload</button>
    <button class="menu-item" onclick={() => { navigator.clipboard?.writeText(chosen.map((d) => d.url).join("\n")); context = null; message = "Address copied."; }}><span class="tick"></span>Copy address</button>
    <button class="menu-item danger" onclick={() => askDelete(ids())}><span class="tick"></span>Delete<span class="key">Del</span></button>
  </div>
{/if}

{#if adding}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (adding = false)}>
    <form class="dialog add-dialog" onsubmit={add} aria-labelledby="add-title">
      <div class="dialog-title" id="add-title">Enter new address to download</div>
      <div class="add-grid">
        <div class="add-fields">
          <div class="field"><label for="address">Address</label>
            <!-- svelte-ignore a11y_autofocus -->
            <input id="address" bind:value={address} list="recent-addresses" autofocus autocomplete="off" spellcheck="false" placeholder="https://… or magnet:?…" />
            <datalist id="recent-addresses">{#each recentAddresses as a}<option value={a}></option>{/each}</datalist>
          </div>
          <fieldset class="auth">
            <legend><label class="checkbox"><input type="checkbox" bind:checked={useAuth} /> Use authorization</label></legend>
            <div class="auth-row">
              <label for="login" class:off={!useAuth}>Login</label><input id="login" bind:value={login} disabled={!useAuth} autocomplete="off" spellcheck="false" />
              <label for="password" class:off={!useAuth}>Password</label><input id="password" type="password" bind:value={password} disabled={!useAuth} autocomplete="off" />
            </div>
          </fieldset>
          {#if addError}<p class="error" role="alert">{addError}</p>{/if}
        </div>
        <div class="add-buttons">
          <button type="submit" class="btn primary" disabled={!address.trim()}>OK</button>
          <button type="button" class="btn" onclick={() => (adding = false)}>Cancel</button>
        </div>
      </div>
    </form>
  </div>
{/if}

{#if refreshing}
  <div class="scrim" role="presentation">
    <form class="dialog" onsubmit={doRefresh} aria-labelledby="refresh-title">
      <div class="dialog-title" id="refresh-title">Refresh address</div>
      <div class="dialog-body">
        <p class="muted">Has the link to <b>{refreshing.name}</b> expired? Open the page where you found it, copy the new download link and paste it here. Kestrel keeps what it already has when the new link gives the same file.</p>
        <div class="field"><label for="newurl">New address</label>
          <!-- svelte-ignore a11y_autofocus -->
          <input id="newurl" bind:value={refreshing.url} autofocus spellcheck="false" onfocus={(e) => (e.currentTarget as HTMLInputElement).select()} /></div>
        {#if refreshing.error}<p class="error" role="alert">{refreshing.error}</p>{/if}
      </div>
      <div class="dialog-actions"><button type="submit" class="btn primary" disabled={refreshing.busy || !refreshing.url.trim()}>{refreshing.busy ? "Checking…" : "Use this address"}</button><button type="button" class="btn" onclick={() => (refreshing = null)}>Cancel</button></div>
    </form>
  </div>
{/if}

{#if summing}
  <div class="scrim" role="presentation">
    <div class="dialog" role="dialog" aria-labelledby="sum-title">
      <div class="dialog-title" id="sum-title">Checksum</div>
      <div class="dialog-body">
        <p class="muted">{summing.name}</p>
        <div class="field"><label for="algo">Type</label>
          <select id="algo" bind:value={summing.algorithm} onchange={computeChecksum}><option value="md5">MD5</option><option value="sha1">SHA-1</option><option value="sha256">SHA-256</option></select></div>
        <div class="field"><label for="sumval">Checksum</label>
          <div class="sumrow"><input id="sumval" class="mono" readonly value={summing.busy ? "Reading the file…" : summing.value} /><button class="btn" disabled={!summing.value} onclick={() => { navigator.clipboard?.writeText(summing!.value); message = "Checksum copied."; }}>Copy</button></div></div>
        <div class="field"><label for="sumexp">Compare with</label>
          <input id="sumexp" class="mono" bind:value={summing.expected} placeholder="Paste the checksum the site gives" spellcheck="false" /></div>
        {#if checksumVerdict === "match"}<p class="good-text">✓ The checksums match: the file is exactly the published one.</p>{/if}
        {#if checksumVerdict === "differ"}<p class="error" role="alert">✗ The checksums differ: the file is damaged or not the published one (or choose another type).</p>{/if}
        {#if summing.error}<p class="error" role="alert">{summing.error}</p>{/if}
      </div>
      <div class="dialog-actions"><button class="btn" onclick={() => (summing = null)}>Close</button></div>
    </div>
  </div>
{/if}

{#if patterning}
  <div class="scrim" role="presentation">
    <form class="dialog" onsubmit={doPattern} aria-labelledby="pat-title">
      <div class="dialog-title" id="pat-title">Rename with a pattern</div>
      <div class="dialog-body">
        <div class="field"><label for="pattern">Name pattern</label>
          <!-- svelte-ignore a11y_autofocus -->
          <input id="pattern" bind:value={patterning.pattern} autofocus spellcheck="false" /></div>
        <p class="muted">{"{name}"} current name · {"{n}"} number · {"{date}"} day added · {"{site}"} site · {"{ext}"} extension (kept when left out)</p>
        <div class="preview">{#each patternPreview as p}<div class="mono">{p}</div>{/each}{#if patterning.ids.length > 3}<div class="muted">…</div>{/if}</div>
        {#if patterning.error}<p class="error" role="alert">{patterning.error}</p>{/if}
      </div>
      <div class="dialog-actions"><button type="submit" class="btn primary" disabled={!patterning.pattern.trim()}>Rename</button><button type="button" class="btn" onclick={() => (patterning = null)}>Cancel</button></div>
    </form>
  </div>
{/if}

{#if stats && statistics}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (stats = false)}>
    <div class="dialog wide" role="dialog" aria-labelledby="stats-title">
      <div class="dialog-title" id="stats-title">Statistics</div>
      <div class="dialog-body">
        <p><b>{statistics.files}</b> finished · <b>{size(statistics.total)}</b> in all</p>
        <div class="chart-title">Last 14 days</div>
        <div class="bars">{#each statistics.days as d}<div class="bar" title={`${d.label}: ${size(d.bytes)} · ${d.files}`}><div class="fill" style:height="{(d.bytes / statistics.dayMax) * 100}%"></div><span>{d.label.slice(3)}</span></div>{/each}</div>
        <div class="chart-title">Last 12 months</div>
        <div class="bars">{#each statistics.months as m}<div class="bar" title={`${m.label}: ${size(m.bytes)} · ${m.files}`}><div class="fill" style:height="{(m.bytes / statistics.monthMax) * 100}%"></div><span>{m.label.slice(5)}</span></div>{/each}</div>
        <p class="muted">Counted from the downloads still in the list.</p>
      </div>
      <div class="dialog-actions"><button class="btn" onclick={() => (stats = false)}>Close</button></div>
    </div>
  </div>
{/if}

{#if renaming}
  <div class="scrim" role="presentation">
    <form class="dialog small" onsubmit={doRename} aria-labelledby="ren-title">
      <div class="dialog-title" id="ren-title">Rename</div>
      <div class="dialog-body"><div class="field"><label for="newname">Name</label>
        <!-- svelte-ignore a11y_autofocus -->
        <input id="newname" bind:value={renaming.name} autofocus spellcheck="false" /></div></div>
      <div class="dialog-actions"><button type="submit" class="btn primary">Rename</button><button type="button" class="btn" onclick={() => (renaming = null)}>Cancel</button></div>
    </form>
  </div>
{/if}

{#if confirmDelete}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="alertdialog" aria-labelledby="del-title">
      <div class="dialog-title" id="del-title">Delete {confirmDelete.length === 1 ? "this download" : `${confirmDelete.length} downloads`}?</div>
      <div class="dialog-body">
        <p>{confirmDelete.length === 1 ? downloads.find((d) => d.id === confirmDelete![0])?.name : "The selected downloads"} will be removed from the list.</p>
        <label class="checkbox"><input type="checkbox" bind:checked={deleteFiles} /> Also delete the files from disk</label>
      </div>
      <div class="dialog-actions"><button class="btn danger" onclick={doDelete}>Delete</button><button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button></div>
    </div>
  </div>
{/if}

{#if countdown}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="alertdialog" aria-labelledby="cd-title">
      <div class="dialog-title" id="cd-title">{countdown.queue} has finished</div>
      <div class="dialog-body"><p>{actionName[countdown.action]} in <b>{countdown.seconds}</b> seconds.</p></div>
      <div class="dialog-actions">
        <button class="btn primary" onclick={() => { const a = countdown!.action; countdown = null; call("power_action", { action: a }); }}>Now</button>
        <button class="btn" onclick={() => (countdown = null)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if crash}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="dialog" aria-labelledby="crash-title">
      <div class="dialog-title" id="crash-title">Kestrel closed unexpectedly</div>
      <div class="dialog-body">
        <p>Kestrel stopped because of an error last time. Sending the report helps fix it; it opens on GitHub, where you can read and change it before sending.</p>
        <pre class="report">{crash.text}</pre>
      </div>
      <div class="dialog-actions">
        <button class="btn primary" onclick={() => { call("report_crash", { id: crash!.id }); crash = null; }}>Report on GitHub</button>
        <button class="btn" onclick={() => { call("dismiss_crash", { id: crash!.id }); crash = null; }}>Don't send</button>
      </div>
    </div>
  </div>
{/if}

{#if update}
  <div class="scrim" role="presentation">
    <div class="dialog small" role="dialog" aria-labelledby="update-title">
      <div class="dialog-title" id="update-title">Update available</div>
      <div class="dialog-body">
        <p><b>Kestrel {update.version}</b> is available (you have {update.current}).</p>
        <p class="muted">{update.signed ? "Downloads are paused and continue after the update. Kestrel restarts when it is installed." : "This release is not signed for automatic installation; its download page opens in your browser."}</p>
      </div>
      <div class="dialog-actions">
        <button class="btn primary" onclick={installUpdate} disabled={installing}>{installing ? "Downloading…" : update.signed ? "Install now" : "Open download page"}</button>
        <button class="btn" onclick={() => (update = null)} disabled={installing}>Later</button>
      </div>
    </div>
  </div>
{/if}

{#if shortcuts}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (shortcuts = false)}>
    <div class="dialog small" role="dialog" aria-labelledby="keys-title">
      <div class="dialog-title" id="keys-title">Keyboard shortcuts</div>
      <div class="dialog-body shortcuts">
        <kbd>Ctrl+N</kbd><span>Add new download</span>
        <kbd>Enter</kbd><span>Open the file, or show its progress</span>
        <kbd>Del</kbd><span>Remove the selected downloads</span>
        <kbd>Ctrl+A</kbd><span>Select all</span>
        <kbd>Ctrl+F</kbd><span>Find</span>
        <kbd>F3</kbd><span>Find next</span>
        <kbd>Ctrl+ +/−/0</kbd><span>Interface size larger, smaller, normal</span>
        <kbd>Esc</kbd><span>Close a menu or dialog</span>
      </div>
      <div class="dialog-actions"><button class="btn primary" onclick={() => (shortcuts = false)}>OK</button></div>
    </div>
  </div>
{/if}

{#if about}
  <div class="scrim" role="presentation" onmousedown={(e) => e.target === e.currentTarget && (about = false)}>
    <div class="dialog small" role="dialog" aria-labelledby="about-title">
      <div class="dialog-title" id="about-title">About Kestrel</div>
      <div class="dialog-body about"><img src="/logo.png" alt="" width="48" height="48" /><div><b>Kestrel {version}</b><p>Rust engine · Tauri interface · MIT licence</p><p class="muted">github.com/AchrafTsouli/kestrel-download-manager</p></div></div>
      <div class="dialog-actions"><button class="btn primary" onclick={() => (about = false)}>OK</button></div>
    </div>
  </div>
{/if}

<style>
  .report { max-height: 160px; overflow: auto; white-space: pre-wrap; font-size: 12px; background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 8px; }
  .window { display: grid; grid-template-rows: auto auto 1fr auto; height: 100%; }
  .menubar { display: flex; align-items: center; height: 26px; padding: 0 6px; background: var(--chrome); }
  .menu-root { position: relative; }
  .menu-title { height: 22px; padding: 0 9px; border: 0; border-radius: 3px; background: transparent; cursor: default; }
  .menu-title:hover, .menu-title.open { background: var(--raised); }
  .dropdown { position: absolute; top: 24px; left: 0; z-index: 30; min-width: 230px; width: max-content; padding: 4px; background: var(--menu); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 10px 28px rgb(0 0 0 / 0.35); display: grid; }
  .sub-root { position: relative; display: grid; }
  .dropdown.flyout { top: -5px; left: calc(100% + 2px); }
  .menu-item.hot { background: var(--hover, rgb(127 127 127 / 0.15)); }
  .shortcuts { display: grid; grid-template-columns: auto 1fr; gap: 6px 18px; }
  .shortcuts kbd { font: inherit; font-weight: 600; white-space: nowrap; }
  .dropdown.sub { top: auto; left: 100%; margin-top: -60px; }
  .menu-item { display: grid; grid-template-columns: 18px 1fr auto; align-items: center; gap: 4px; white-space: nowrap; height: 26px; padding: 0 10px 0 4px; border: 0; border-radius: 4px; background: transparent; text-align: left; cursor: default; }
  .menu-item:hover:not(:disabled) { background: var(--selection); color: var(--selection-text); }
  .menu-item:disabled { color: var(--muted); opacity: 0.55; }
  .menu-item.danger:hover { background: color-mix(in srgb, var(--bad) 28%, transparent); }
  .tick { text-align: center; color: var(--accent); }
  .key { color: var(--muted); font-size: 11px; padding-left: 18px; }
  .sep { height: 1px; margin: 4px 6px; background: var(--line); }
  .brand { margin-left: auto; margin-right: 6px; display: flex; align-items: center; gap: 6px; font-weight: 700; letter-spacing: 0.06em; color: var(--accent); }
  .brand img { border-radius: 4px; }
  .toolbar { display: flex; gap: 2px; padding: 4px 8px 5px; background: var(--chrome); border-bottom: 1px solid var(--line); overflow: hidden; }
  .tool { display: grid; justify-items: center; align-content: center; gap: 3px; width: 70px; height: 62px; padding: 4px 2px; border: 1px solid transparent; border-radius: 6px; background: transparent; color: var(--icon); cursor: default; flex: none; }
  .tool span { font-size: 11px; color: var(--text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
  .tool:hover:not(:disabled) { background: var(--raised); border-color: var(--line); }
  .tool:active:not(:disabled) { background: var(--selection); }
  .tool:disabled { opacity: 0.35; }
  .main { display: grid; min-height: 0; }
  .tree { display: flex; flex-direction: column; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; padding-bottom: 8px; }
  .pane-title { display: flex; justify-content: space-between; align-items: center; height: 24px; padding: 0 8px; font-weight: 600; color: var(--muted); border-bottom: 1px solid var(--line); }
  .pane-close { border: 0; background: none; color: var(--muted); font-size: 14px; line-height: 1; padding: 0 2px; }
  .node { display: flex; align-items: center; gap: 6px; height: 21px; padding: 0 8px 0 6px; border: 0; background: transparent; text-align: left; white-space: nowrap; cursor: default; color: var(--text); flex: none; }
  .node.child { padding-left: 34px; }
  .node:not(.static):hover { background: var(--raised); }
  .node.on { background: var(--selection); color: var(--selection-text); }
  .node small { margin-left: auto; color: var(--muted); font-size: 10.5px; }
  .node.on small { color: inherit; }
  .twisty { width: 10px; font-size: 9px; color: var(--muted); }
  .live { width: 7px; height: 7px; border-radius: 50%; background: var(--good); }
  .listpane { display: grid; grid-template-rows: auto 1fr; min-width: 0; min-height: 0; background: var(--bg); }
  .list-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; height: 30px; padding: 0 8px 0 10px; background: var(--surface); border-bottom: 1px solid var(--line); }
  .list-head h1 { margin: 0; font-size: 12px; font-weight: 600; }
  .search { width: 200px; height: 22px; padding: 0 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .grid-wrap { overflow: auto; position: relative; }
  .grid { width: 100%; border-collapse: collapse; table-layout: fixed; font-variant-numeric: tabular-nums; }
  .grid th { position: sticky; top: 0; z-index: 2; height: 25px; padding: 0 4px 0 6px; text-align: left; font-weight: 600; color: var(--text); background: var(--chrome); border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); white-space: nowrap; }
  .th { display: flex; align-items: center; gap: 4px; min-width: 0; }
  .th-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .grid th.r .th-label { text-align: right; }
  .grid th.c .th-label { text-align: center; }
  .resize { position: absolute; top: 0; right: -3px; z-index: 3; width: 6px; height: 100%; cursor: col-resize; }
  .resize:hover { background: color-mix(in srgb, var(--accent) 45%, transparent); }
  .funnel { flex: none; display: grid; place-items: center; width: 16px; height: 16px; padding: 0; border: 0; border-radius: 3px; background: transparent; color: var(--muted); cursor: default; }
  .funnel svg { fill: currentColor; }
  .funnel:hover { background: var(--raised); color: var(--text); }
  .funnel.on { color: var(--accent); }
  .filter-pop { position: absolute; top: 26px; left: 0; z-index: 5; display: flex; gap: 6px; width: 240px; padding: 6px; background: var(--menu); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 10px 28px rgb(0 0 0 / 0.35); font-weight: 400; }
  .filter-pop .input { flex: 1; min-width: 0; }
  .grid th { position: sticky; }
  .grid td { height: 21px; padding: 0 6px; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .grid tr.filler td { height: 21px; }
  .grid tbody tr.filler:hover { background: transparent; }
  .grid tbody tr.alt { background: var(--row-alt); }
  .grid tbody tr.spacer td { padding: 0; border: 0; }
  .grid tbody tr.spacer:hover { background: transparent; }
  .grid tbody tr:hover { background: color-mix(in srgb, var(--selection) 45%, transparent); }
  .grid tbody tr.sel { background: var(--selection); color: var(--selection-text); }
  .grid .r { text-align: right; }
  .grid .c { text-align: center; color: var(--accent); }
  .name { display: flex; align-items: center; gap: 5px; }
  .name span { overflow: hidden; text-overflow: ellipsis; }
  .desc { color: var(--muted); }
  tr.sel .desc { color: inherit; }
  .status.good { color: var(--good); } .status.bad { color: var(--bad); } .status.warn { color: var(--warn); } .status.run { color: var(--accent); }
  .progress { display: flex; align-items: center; gap: 6px; }
  .bar { flex: 1; height: 5px; border-radius: 3px; background: var(--raised); overflow: hidden; }
  .bar i { display: block; height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); transition: width 0.25s linear; }
  .progress span { width: 30px; text-align: right; font-size: 10.5px; color: var(--muted); }
  .empty { position: absolute; inset: 40px 0 auto; text-align: center; color: var(--muted); }
  .statusbar { display: flex; justify-content: space-between; gap: 16px; height: 24px; align-items: center; padding: 0 10px; background: var(--chrome); border-top: 1px solid var(--line); color: var(--muted); }
  .msg { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .totals { white-space: nowrap; }
  .totals b { color: var(--text); font-weight: 600; }
  .context { position: fixed; top: 0; left: 0; }
  .submenu { position: relative; }
  .scrim { position: fixed; inset: 0; z-index: 40; display: grid; place-items: center; background: rgb(0 0 0 / 0.35); }
  .dialog { width: min(540px, 92vw); background: var(--surface); border: 1px solid var(--line); border-radius: 8px; box-shadow: 0 18px 50px rgb(0 0 0 / 0.45); overflow: hidden; }
  .dialog.small { width: min(420px, 92vw); }
  .add-dialog { width: min(600px, 94vw); }
  .add-grid { display: grid; grid-template-columns: 1fr auto; gap: 14px; padding: 14px; }
  .add-fields { display: grid; gap: 10px; min-width: 0; }
  .add-buttons { display: flex; flex-direction: column; gap: 8px; }
  .auth { margin: 0; padding: 4px 10px 10px; border: 1px solid var(--line); border-radius: 4px; }
  .auth legend { padding: 0 4px; }
  .auth-row { display: grid; grid-template-columns: auto 1fr auto 1fr; align-items: center; gap: 8px; }
  .auth-row label { color: var(--muted); }
  .auth-row label.off { opacity: 0.5; }
  .auth-row input { min-width: 0; height: 26px; padding: 0 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; }
  .auth-row input:disabled { opacity: 0.5; }
  .dialog-title { padding: 10px 14px; font-weight: 600; background: var(--chrome); border-bottom: 1px solid var(--line); }
  .dialog-body { padding: 14px; display: grid; gap: 10px; }
  .dialog-body p { margin: 0; }
  .field { display: grid; grid-template-columns: 64px 1fr; align-items: center; gap: 8px; }
  .field label { color: var(--muted); }
  .field input { height: 28px; padding: 0 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; user-select: text; }
  .field input:focus { border-color: var(--accent); outline: none; }
  .error { color: var(--bad); }
  .checkbox { display: flex; align-items: center; gap: 6px; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; padding: 0 14px 14px; }
  .about { grid-template-columns: auto 1fr; align-items: center; gap: 14px; }
  .about img { border-radius: 10px; }
  .muted { color: var(--muted); }
  .dialog.wide { width: min(640px, 94vw); }
  .field { grid-template-columns: 96px 1fr; }
  .field select { height: 28px; padding: 0 6px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); color: var(--text); font: inherit; }
  .sumrow { display: flex; gap: 6px; min-width: 0; }
  .sumrow input { flex: 1; min-width: 0; }
  .mono { font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; }
  .good-text { color: var(--good); }
  .preview { display: grid; gap: 2px; padding: 6px 8px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); overflow: hidden; }
  .preview div { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chart-title { color: var(--muted); font-size: 12px; margin-top: 4px; }
  .bars { display: flex; align-items: flex-end; gap: 4px; height: 110px; padding: 6px; border: 1px solid var(--line); border-radius: 4px; background: var(--bg); }
  .bar { flex: 1; height: 100%; display: flex; flex-direction: column; justify-content: flex-end; align-items: stretch; gap: 3px; min-width: 0; }
  .bar .fill { background: var(--accent); border-radius: 2px 2px 0 0; min-height: 1px; }
  .bar span { font-size: 10px; color: var(--muted); text-align: center; }
</style>
