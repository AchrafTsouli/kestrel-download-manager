// End-to-end test of Kestrel's main window: the real app (rust/target/release/kestrel.exe) on a
// separate test profile, driven through the WebView's debugging port. Nothing touches the mouse,
// the keyboard or the user's own Kestrel data.
//
//   cd rust/app; npm run test:ui          (build first: pwsh scripts/Build-Test.ps1)
//
// Covers: Add URL dialog, a download that stops and resumes from where it was, the queue (download
// later, start queue), delete, and the bytes on disk.

import { spawn, spawnSync, execSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

const RUST = resolve(import.meta.dirname, "../..");
const EXE = join(RUST, "target", "release", "kestrel.exe");
const PORT = 9333;
const RATE = 2 << 20; // the test server sends 2 MB/s in total, so a download lasts long enough to stop it
const SIZE = 8 << 20;

// ------------------------------------------------------------------ test server
const byteAt = (i) => (i * 31 + 7) & 255;
const content = (start, end) => { const b = Buffer.alloc(end - start + 1); for (let i = 0; i < b.length; i++) b[i] = byteAt(start + i); return b; };
let gate = Promise.resolve();
const pace = (n) => (gate = gate.then(() => new Promise((r) => setTimeout(r, (n / RATE) * 1000))));

const server = createServer(async (req, res) => {
  const total = SIZE;
  let [start, end] = [0, total - 1];
  const range = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
  if (range) { start = +range[1]; end = range[2] ? Math.min(+range[2], total - 1) : total - 1; }
  res.writeHead(range ? 206 : 200, {
    "Content-Type": "application/octet-stream", "Content-Length": end - start + 1, "Accept-Ranges": "bytes", ETag: '"ui-test-1"',
    "Last-Modified": "Mon, 02 Nov 2026 10:00:00 GMT", ...(range ? { "Content-Range": `bytes ${start}-${end}/${total}` } : {}),
  });
  if (req.method === "HEAD") return res.end();
  let closed = false;
  res.on("close", () => (closed = true));
  for (let at = start; at <= end && !closed; at += 64 << 10) {
    const to = Math.min(at + (64 << 10) - 1, end);
    await pace(to - at + 1);
    if (!closed) res.write(content(at, to));
  }
  res.end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const url = (name) => `http://127.0.0.1:${server.address().port}/${name}`;

// ------------------------------------------------------------------ test profile and app
const running = execSync('tasklist /FI "IMAGENAME eq Kestrel.exe" /FO CSV /NH', { encoding: "utf8" });
if (/kestrel\.exe/i.test(running)) {
  console.error("Close Kestrel first: a second Kestrel would hand the test over to it.");
  process.exit(2);
}
assert.ok(existsSync(EXE), `build the app first (${EXE})`);
const root = mkdtempSync(join(tmpdir(), "kestrel-ui-"));
const env = { ...process.env, KESTREL_DATA_DIR: join(root, "data"), KESTREL_TEST_DOWNLOADS: join(root, "downloads"), PATH: `${process.env.USERPROFILE}\\.cargo\\bin;${process.env.PATH}` };
const seed = spawnSync("cargo", ["test", "-q", "-p", "kestrel-app", "seed_test_profile", "--", "--ignored"], { cwd: RUST, env, encoding: "utf8" });
assert.equal(seed.status, 0, "seeding the test profile: " + seed.stderr);
const app = spawn(EXE, [], { env: { ...env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` }, stdio: "ignore" });
process.on("exit", () => { try { app.kill(); } catch {} });

// ------------------------------------------------------------------ debugging protocol
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let page;
for (let i = 0; i < 60 && !page; i++) {
  await sleep(500);
  try { page = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).find((t) => t.type === "page"); } catch {}
}
assert.ok(page, "the main window opened");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let n = 0; const pending = new Map();
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); pending.get(m.id)?.(m); pending.delete(m.id); });
const send = (method, params = {}) => new Promise((r) => { const id = ++n; pending.set(id, r); ws.send(JSON.stringify({ id, method, params })); });
async function js(expression) {
  const { result } = await send("Runtime.evaluate", { expression: HELPERS + expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? "page error");
  return result.result.value;
}
async function until(what, expression, seconds = 20) {
  for (let i = 0; i < seconds * 10; i++) { const v = await js(expression); if (v) return v; await sleep(100); }
  throw new Error(`timed out: ${what}`);
}
// In the page: rows by file name, toolbar buttons by label, menu items by text.
const HELPERS = `if (!window.t) window.t = {
  row: (name) => [...document.querySelectorAll(".grid tbody tr")].find((r) => r.querySelector("td.name span")?.textContent === name),
  status: (name) => t.row(name)?.querySelector(".status")?.textContent ?? null,
  tool: (label) => [...document.querySelectorAll("button.tool")].find((b) => b.textContent.trim().startsWith(label)),
  item: (text) => [...document.querySelectorAll(".context .menu-item")].find((b) => b.textContent.trim().startsWith(text)),
  select: (names) => names.forEach((name, i) => t.row(name).dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0, ctrlKey: i > 0 }))),
  menuItem: (text) => [...document.querySelectorAll(".menubar .menu-item")].find((b) => b.textContent.trim().startsWith(text)),
  menuTitle: (text) => [...document.querySelectorAll(".menubar .menu-title")].find((b) => b.textContent.trim() === text),
  menu: (name) => t.row(name).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 200, clientY: 200 })),
};
`;

const results = [];
async function check(name, body) {
  const started = Date.now();
  try { await body(); results.push(`ok    ${name} (${((Date.now() - started) / 1000).toFixed(1)} s)`); }
  catch (error) { results.push(`FAIL  ${name}: ${error.message}`); process.exitCode = 1; }
}
const percent = (s) => s && /%$/.test(s) ? parseFloat(s) : null;

try {
  await until("the list is ready", `document.querySelector(".grid") !== null`);

  await check("Add URL starts a download", async () => {
    await js(`t.tool("Add URL").click(); true`);
    await until("the dialog", `document.querySelector("#address") !== null`);
    await js(`(() => { const i = document.querySelector("#address"); i.value = ${JSON.stringify(url("ui-one.bin"))}; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
    await js(`document.querySelector("form.add-dialog").requestSubmit(); true`);
    await until("the download runs", `(() => { const s = t.status("ui-one.bin"); return s && /%$/.test(s) && parseFloat(s) > 0; })()`);
  });

  let stoppedAt = 0;
  await check("Stop keeps the progress", async () => {
    await js(`t.select(["ui-one.bin"]); true`);
    await until("Stop is enabled", `!t.tool("Stop").disabled`);
    await js(`t.tool("Stop").click(); true`);
    await until("paused", `t.status("ui-one.bin") === "Paused"`);
    const shown = await js(`t.row("ui-one.bin").textContent`);
    stoppedAt = Number(/(\d+(?:\.\d+)?)\s*%/.exec(shown)?.[1] ?? 0);
    assert.ok(stoppedAt >= 0);
  });

  await check("Resume continues and the file is complete and correct", async () => {
    await js(`t.tool("Resume").click(); true`);
    const first = await until("running again", `(() => { const s = t.status("ui-one.bin"); return /%$/.test(s ?? "") ? s : null; })()`);
    assert.ok(percent(first) >= stoppedAt, `resumed at ${first}, stopped at ${stoppedAt}%`);
    await until("complete", `t.status("ui-one.bin") === "Complete"`, 30);
    const bytes = readFileSync(join(root, "downloads", "ui-one.bin"));
    assert.equal(bytes.length, SIZE);
    assert.ok(bytes.equals(content(0, SIZE - 1)), "the bytes on disk are the server's");
  });

  await check("Download later, then Start queue runs both", async () => {
    for (const name of ["ui-two.bin", "ui-three.bin"]) {
      await js(`t.tool("Add URL").click(); true`);
      await until("the dialog", `document.querySelector("#address") !== null`);
      await js(`(() => { const i = document.querySelector("#address"); i.value = ${JSON.stringify(url(name))}; i.dispatchEvent(new Event("input", { bubbles: true })); document.querySelector("form.add-dialog").requestSubmit(); })()`);
      await until(`${name} listed`, `t.row(${JSON.stringify(name)}) !== undefined`);
    }
    await js(`t.select(["ui-two.bin", "ui-three.bin"]); true`);
    await until("Stop is enabled", `!t.tool("Stop").disabled`);
    await js(`t.tool("Stop").click(); true`);
    await until("both paused", `t.status("ui-two.bin") === "Paused" && t.status("ui-three.bin") === "Paused"`);
    await js(`t.menu("ui-two.bin"); true`);
    await until("the row menu", `t.item("Download later") !== undefined`);
    await js(`t.item("Download later").click(); true`);
    await until("both queued", `t.status("ui-two.bin") === "Queued" && t.status("ui-three.bin") === "Queued"`);
    await js(`t.tool("Start Qu").click(); true`);
    await until("both complete", `t.status("ui-two.bin") === "Complete" && t.status("ui-three.bin") === "Complete"`, 40);
  });

  await check("Checksum of a finished file matches its real SHA-256", async () => {
    await js(`t.select(["ui-one.bin"]); t.menu("ui-one.bin"); true`);
    await until("the row menu", `t.item("Checksum") !== undefined`);
    await js(`t.item("Checksum").click(); true`);
    const value = await until("the checksum", `(() => { const v = document.querySelector("#sumval")?.value ?? ""; return /^[0-9a-f]{64}$/.test(v) ? v : null; })()`, 30);
    assert.equal(value, createHash("sha256").update(content(0, SIZE - 1)).digest("hex"));
    await js(`(() => { const i = document.querySelector("#sumexp"); i.value = ${JSON.stringify("x")}; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
    await until("a mismatch is shown", `document.body.textContent.includes("The checksums differ")`);
    await js(`[...document.querySelectorAll(".dialog .btn")].find((b) => b.textContent.trim() === "Close").click(); true`);
  });

  await check("Statistics count the finished downloads", async () => {
    await js(`t.menuTitle("View").click(); true`);
    await until("the View menu", `t.menuItem("Statistics") !== undefined`);
    await js(`t.menuItem("Statistics").click(); true`);
    const text = await until("the statistics", `document.querySelector("#stats-title")?.closest(".dialog")?.textContent ?? null`);
    assert.match(text, /3\s*finished/);
    await js(`[...document.querySelectorAll(".dialog .btn")].find((b) => b.textContent.trim() === "Close").click(); true`);
  });

  await check("Rename with a pattern", async () => {
    await js(`t.select(["ui-two.bin"]); t.menu("ui-two.bin"); true`);
    await until("the row menu", `t.item("Rename with a pattern") !== undefined`);
    await js(`t.item("Rename with a pattern").click(); true`);
    await until("the dialog", `document.querySelector("#pattern") !== null`);
    await js(`(() => { const i = document.querySelector("#pattern"); i.value = "{n}-{name}"; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
    await until("the preview", `document.querySelector(".preview")?.textContent.includes("01-ui-two.bin")`);
    await js(`document.querySelector("#pattern").closest("form").requestSubmit(); true`);
    await until("the new name in the list", `t.row("01-ui-two.bin") !== undefined`);
    assert.ok(existsSync(join(root, "downloads", "01-ui-two.bin")), "the file was renamed on disk");
  });

  await check("Refresh address continues from a new link", async () => {
    await js(`t.tool("Add URL").click(); true`);
    await until("the dialog", `document.querySelector("#address") !== null`);
    await js(`(() => { const i = document.querySelector("#address"); i.value = ${JSON.stringify(url("ui-four.bin"))}; i.dispatchEvent(new Event("input", { bubbles: true })); document.querySelector("form.add-dialog").requestSubmit(); })()`);
    await until("running", `(() => { const s = t.status("ui-four.bin"); return s && /%$/.test(s) && parseFloat(s) > 0; })()`);
    await js(`t.select(["ui-four.bin"]); true`);
    await until("Stop is enabled", `!t.tool("Stop").disabled`);
    await js(`t.tool("Stop").click(); true`);
    await until("paused", `t.status("ui-four.bin") === "Paused"`);
    await js(`t.menu("ui-four.bin"); true`);
    await until("the row menu", `t.item("Refresh address") !== undefined`);
    await js(`t.item("Refresh address").click(); true`);
    await until("the dialog", `document.querySelector("#newurl") !== null`);
    await js(`(() => { const i = document.querySelector("#newurl"); i.value = ${JSON.stringify(url("mirror/ui-four.bin"))}; i.dispatchEvent(new Event("input", { bubbles: true })); i.closest("form").requestSubmit(); })()`);
    await until("the dialog closes", `document.querySelector("#newurl") === null`, 30);
    await js(`t.select(["ui-four.bin"]); t.tool("Resume").click(); true`);
    await until("complete", `t.status("ui-four.bin") === "Complete"`, 30);
    assert.ok(readFileSync(join(root, "downloads", "ui-four.bin")).equals(content(0, SIZE - 1)), "the bytes on disk are right");
  });

  await check("The interface switches to German", async () => {
    await js(`(async () => { const s = await window.__TAURI_INTERNALS__.invoke("get_settings"); await window.__TAURI_INTERNALS__.invoke("save_settings", { settings: { ...s, language: "de" } }); return true; })()`);
    await until("German toolbar", `t.tool("URL hinzufügen") !== undefined`);
    await js(`(async () => { const s = await window.__TAURI_INTERNALS__.invoke("get_settings"); await window.__TAURI_INTERNALS__.invoke("save_settings", { settings: { ...s, language: "en" } }); return true; })()`);
    await until("English again", `t.tool("Add URL") !== undefined`);
  });

  await check("Markup parsed the way the browser extension's video panel builds it", async () => {
    const parsed = await js(`(() => { const d = new DOMParser().parseFromString('<style>.a{}</style><div class="panel"><svg viewBox="0 0 24 24"><path d="M1 1"/></svg></div>', "text/html"); const nodes = [...d.head.childNodes, ...d.body.childNodes]; const host = document.createElement("div"); const root = host.attachShadow({ mode: "open" }); root.append(...nodes); return [...root.childNodes].map((n) => n.nodeName).join(",") + "|" + root.querySelector("svg").namespaceURI; })()`);
    assert.equal(parsed, "STYLE,DIV|http://www.w3.org/2000/svg");
  });

  await check("Delete removes the row", async () => {
    await js(`t.select(["ui-three.bin"]); t.tool("Delete").click(); true`);
    await until("the confirmation", `[...document.querySelectorAll(".dialog .btn.danger")].length > 0`);
    await js(`document.querySelector(".dialog .btn.danger").click(); true`);
    await until("the row is gone", `t.row("ui-three.bin") === undefined`);
  });
} finally {
  console.log(results.join("\n"));
  ws.close();
  app.kill();
  server.close();
  await sleep(1500);
  try { rmSync(root, { recursive: true, force: true }); } catch {}
}
