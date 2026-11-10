import { sendNative, statusStorage } from "./transfer.js";

const $ = selector => document.querySelector(selector);
const state = $("#state"), message = $("#status");
$("#version").textContent = "v" + chrome.runtime.getManifest().version;

function show(text, error = false) {
  message.hidden = !text;
  message.textContent = text || "";
  message.classList.toggle("error", error);
}
function connection(kind, text) {
  state.className = "state " + kind;
  state.querySelector("b").textContent = text;
}

const stored = await chrome.storage.local.get(["captureEnabled", "videoPanelEnabled", "capturePolicy", "pausedSites"]);
const { lastMessage } = await statusStorage(chrome).get("lastMessage");
if (lastMessage) show(lastMessage, /^Kept in browser|not|fail|error/i.test(lastMessage));

const capture = $("#capture"), videos = $("#videos");
capture.checked = stored.captureEnabled !== false;
videos.checked = stored.videoPanelEnabled ?? true;
capture.addEventListener("change", () => chrome.storage.local.set({ captureEnabled: capture.checked }));
videos.addEventListener("change", () => chrome.storage.local.set({ videoPanelEnabled: videos.checked }));

// The bypass key from Kestrel's Options, so the hint matches what the app does.
const bypass = stored.capturePolicy?.bypassKey || "Alt";
$("#hint").textContent = `${bypass}+click a link to keep it in the browser`;

// This site: Kestrel can be paused here only (links, downloads and the video button).
const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
const host = (() => { try { const u = new URL(tab?.url || ""); return /^https?:$/.test(u.protocol) ? u.hostname : ""; } catch { return ""; } })();
let paused = Array.isArray(stored.pausedSites) ? stored.pausedSites : [];
if (host) {
  const card = $("#site-card"), toggle = $("#site");
  const draw = () => {
    const off = paused.includes(host);
    toggle.checked = !off;
    card.classList.toggle("paused", off);
    card.querySelector(".label").textContent = off ? "Paused on this site" : "Active on this site";
  };
  $("#site-name").textContent = host.replace(/^www\./, "");
  card.hidden = false;
  draw();
  toggle.addEventListener("change", async () => {
    paused = toggle.checked ? paused.filter(h => h !== host) : [...new Set([...paused, host])];
    await chrome.storage.local.set({ pausedSites: paused });
    draw();
  });
} else {
  $("#add").disabled = true;
  $("#refresh-videos").disabled = true;
}

async function check() {
  connection("checking", "Checking…");
  try {
    await sendNative(chrome, { action: "ping" }, 6000);
    connection("ok", "Connected");
    await chrome.action.setBadgeText({ text: "" });
    return true;
  } catch (error) {
    connection("off", "Not connected");
    show(error.message, true);
    return false;
  }
}
check();

async function busy(button, work) {
  button.disabled = true;
  try { await work(); } catch (error) { show(error.message, true); }
  finally { button.disabled = false; }
}
$("#open").addEventListener("click", event => busy(event.currentTarget, async () => {
  await sendNative(chrome, { action: "show" });
  connection("ok", "Connected");
  window.close();
}));
$("#test").addEventListener("click", event => busy(event.currentTarget, async () => {
  show("");
  if (await check()) show("The browser bridge is installed and Kestrel answers.");
}));
$("#add").addEventListener("click", event => busy(event.currentTarget, async () => {
  await sendNative(chrome, { action: "download", url: tab.url, requestId: crypto.randomUUID() });
  show("Sent to Kestrel. For a video page, Kestrel lists its qualities.");
}));
$("#refresh-videos").addEventListener("click", event => busy(event.currentTarget, async () => {
  await chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, files: ["media-policy.js", "video-panel-policy.js", "video-panel.js"] });
  show(videos.checked ? "Looking for videos. Play the video and its download button appears." : "Turn on “Video download button” first.");
}));
