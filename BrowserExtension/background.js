import { installMedia } from "./media-background.js";
installMedia(chrome);
import { shouldCapture } from "./capture-policy.js";
import { captureDownload, directUrl, recoverHandoffs, sendNative, statusStorage } from "./transfer.js";
import { downloadSession, installHeaderCapture } from "./session.js";
installHeaderCapture(chrome);

const recovery = recoverHandoffs(chrome).catch(console.error);
// Sites where the popup paused Kestrel: nothing is captured there.
const hostOf = url => { try { return new URL(url).hostname; } catch { return ""; } };
async function pausedOn(...urls) {
  const { pausedSites = [] } = await chrome.storage.local.get("pausedSites");
  return urls.some(url => url && pausedSites.includes(hostOf(url)));
}
const captureIfWanted = item => recovery.then(async () => { if (await pausedOn(item.referrer, item.url || item.finalUrl)) return; await refreshPolicy(); const { capturePolicy = {} } = await chrome.storage.local.get("capturePolicy"); if (shouldCapture(item.url || item.finalUrl, item.filename, capturePolicy)) await captureDownload(chrome, item); });
if (chrome.downloads.onDeterminingFilename) {
  // Chromium: unlike onCreated, this event includes the server/browser's suggested filename.
  chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
    captureIfWanted(item).catch(console.error).finally(() => suggest());
    return true;
  });
} else {
  // Firefox has no onDeterminingFilename; onCreated already carries the target file name.
  chrome.downloads.onCreated.addListener(item => { captureIfWanted(item).catch(console.error); });
}

chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (sender.id !== chrome.runtime.id || !sender.tab) return;
  if (message.action === "capture-policy") {
    refreshPolicy().then(async () => reply({ ok: true, policy: (await chrome.storage.local.get("capturePolicy")).capturePolicy || {} })).catch(() => reply({ ok: false }));
    return true;
  }
  if (message.action === "magnet") {
    (async () => {
      const { captureEnabled = true } = await chrome.storage.local.get("captureEnabled");
      if (!captureEnabled || await pausedOn(sender.tab.url) || !/^magnet:\?(.*&)?xt=urn:btih:/i.test(message.url || "") || message.url.length > 8192) return { ok: false };
      await sendNative(chrome, { action: "download", url: message.url, requestId: crypto.randomUUID() });
      return { ok: true };
    })().then(reply).catch(() => reply({ ok: false }));
    return true;
  }
  if (message.action !== "direct-click" || !directUrl(message.url)) return;
  (async () => {
    const { captureEnabled = true } = await chrome.storage.local.get("captureEnabled");
    await refreshPolicy(); const { capturePolicy = {} } = await chrome.storage.local.get("capturePolicy"); if (!captureEnabled || await pausedOn(sender.tab.url) || !shouldCapture(message.url, message.fileName, capturePolicy, message.force)) { reply({ ok: false }); return; }
    try {
      const session = await downloadSession(chrome, message.url, sender.tab.id, sender.tab.url);
      await sendNative(chrome, { action: "download", url: message.url, fileName: message.fileName || "", requestId: crypto.randomUUID(), session });
      await statusStorage(chrome).set({ lastMessage: "Download opened in Kestrel." }).catch(() => {});
      reply({ ok: true });
    } catch (error) {
      const { browserFallbacks = [] } = await chrome.storage.session.get("browserFallbacks");
      await chrome.storage.session.set({ browserFallbacks: [...browserFallbacks.filter(value => value.expires > Date.now()), { url: message.url, expires: Date.now() + 60000 }] });
      await statusStorage(chrome).set({ lastMessage: `Kept in browser: ${error.message}` });
      reply({ ok: false });
    }
  })().catch(error => { console.error(error); reply({ ok: false }); });
  return true;
});

chrome.runtime.onInstalled.addListener(() => {
  chrome.storage.local.get("videoPanelEnabled").then(value => { if (value.videoPanelEnabled === undefined) return chrome.storage.local.set({ videoPanelEnabled: true }); });
  // Static content scripts only run on navigation; attach panels to tabs already open during an update.
  chrome.tabs.query({}).then(tabs => Promise.allSettled(tabs.filter(tab => /^https?:/.test(tab.url || '')).map(tab =>
    chrome.scripting.executeScript({ target: { tabId: tab.id, allFrames: true }, files: ['media-policy.js', 'video-panel-policy.js', 'video-panel.js'] })
  ))).catch(console.error);
  chrome.contextMenus.removeAll(() => {
    const web = ["http://*/*", "https://*/*"];
    chrome.contextMenus.create({ id: "swiftfetch-download", title: "Download with Kestrel", contexts: ["link"], targetUrlPatterns: web });
    chrome.contextMenus.create({ id: "kestrel-media", title: "Download this video with Kestrel", contexts: ["video", "audio"] });
    chrome.contextMenus.create({ id: "kestrel-image", title: "Download image with Kestrel", contexts: ["image"], targetUrlPatterns: web });
    chrome.contextMenus.create({ id: "kestrel-page", title: "Download this page's video with Kestrel", contexts: ["page"], documentUrlPatterns: web });
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  // A video or audio playing from a blob or stream has no file address: Kestrel reads the page instead.
  const media = info.menuItemId === "kestrel-media" ? (directUrl(info.srcUrl) ? info.srcUrl : info.pageUrl) : null;
  const url = { "swiftfetch-download": info.linkUrl, "kestrel-image": info.srcUrl, "kestrel-page": info.pageUrl }[info.menuItemId] ?? media;
  if (!url || !directUrl(url)) return;
  downloadSession(chrome, url, tab?.id, info.pageUrl).then(session => sendNative(chrome, { action: "download", url, requestId: crypto.randomUUID(), session }))
    .then(() => statusStorage(chrome).set({ lastMessage: "Sent to Kestrel." }))
    .catch(async error => {
      await statusStorage(chrome).set({ lastMessage: error.message });
      await chrome.action.setBadgeText({ text: "!" });
    });
});

let policyAt = 0;
let refreshing;
async function refreshPolicy() {
  if (Date.now() - policyAt < 15000) return;
  if (refreshing) return refreshing;
  refreshing = (async () => {
    try {
      const response = await sendNative(chrome, { action: "capture-policy" });
      if (response.policy) {
        await chrome.storage.local.set({ capturePolicy: response.policy });
        await chrome.contextMenus.update("swiftfetch-download", { visible: response.policy.contextMenu !== false }).catch(() => {});
      }
    } catch { /* Keep last saved capture policy if the native host is unavailable. */ }
    finally { policyAt = Date.now(); refreshing = null; }
  })();
  return refreshing;
}
refreshPolicy();

