<script lang="ts">
  // Options, laid out like IDM's and MDM's configuration dialog: two rows of tabs, one page each.
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { call, applyTheme, categories, type Settings } from "../lib/api";
  import { languages } from "../lib/i18n";

  let s: Settings | null = $state(null);
  let tab = $state("General");
  let error = $state("");
  let saved = $state("");
  let limitKb = $state(0);
  let bridgeNote = $state("");
  let checks: { browser: string; ok: boolean; detail: string }[] = $state([]);
  let newHost = $state("");
  let newHostN = $state(2);
  const win = getCurrentWindow();
  const rows = [
    ["General", "File types", "Save to", "Downloads", "Connection", "Updates"],
    ["Proxy / Socks", "Sites Logins", "Rules", "Sounds", "Regional", "Archives"],
  ];
  // The row holding the open page sits next to the page, as in IDM.
  const ordered = $derived(rows[0].includes(tab) ? [rows[1], rows[0]] : [rows[0], rows[1]]);
  const soundNames = ["Download complete", "Download failed", "Queue processing started", "Queue processing stopped/finished"];
  const keys = ["Alt", "Ctrl", "Shift", "None"];

  onMount(() => { call<Settings>("get_settings").then((v) => { s = v; limitKb = Math.round(v.speedLimit / 1024); applyTheme(v.theme); }); });
  async function browse() {
    const chosen = await pickFolder({ directory: true, defaultPath: s?.downloadDir, title: "Download folder" });
    if (typeof chosen === "string" && s) s.downloadDir = chosen;
  }
  async function browseCategory(c: string) {
    if (!s) return;
    const chosen = await pickFolder({ directory: true, defaultPath: s.categoryDirs[c] || s.downloadDir, title: `Folder for ${c}` });
    if (typeof chosen === "string") s.categoryDirs = { ...s.categoryDirs, [c]: chosen };
  }
  async function pickScanner() {
    const chosen = await pickFolder({ title: "Virus scanner program", filters: [{ name: "Programs", extensions: ["exe", "bat", "cmd"] }] });
    if (typeof chosen === "string" && s) s.virusScanner = chosen;
  }
  async function pickSound(i: number) {
    const chosen = await pickFolder({ title: "Sound file", filters: [{ name: "Sounds", extensions: ["wav"] }] });
    if (typeof chosen === "string" && s) s.sounds[i].file = chosen;
  }
  async function save(close: boolean) {
    if (!s) return;
    error = ""; saved = "";
    try {
      await call("save_settings", { settings: { ...s, speedLimit: Math.max(0, limitKb) * 1024, connections: Number(s.connections), maxRetries: Number(s.maxRetries), interfaceSize: Number(s.interfaceSize), firstDay: Number(s.firstDay), duplicateAction: Number(s.duplicateAction), archiveTarget: Number(s.archiveTarget), proxyMode: Number(s.proxyMode) } });
      applyTheme(s.theme);
      if (close) win.close(); else saved = "Options saved.";
    } catch (e) { error = String(e); }
  }
  const limits = $derived(s ? Object.entries(s.hostLimits).sort((a, b) => a[0].localeCompare(b[0])) : []);
  function folderOf(c: string) { return !s ? "" : s.categoryDirs[c] || (s.categoryFolders && c !== "Other" ? `${s.downloadDir}\\${c}` : s.downloadDir); }
</script>

<div class="win">
  {#if s}
    <div class="win-body">
      <div class="tabrows" role="tablist" aria-label="Option pages">
        {#each ordered as row}
          <div class="tabrow">
            {#each row as t}<button role="tab" class="tab" class:on={tab === t} onclick={() => (tab = t)}>{t}</button>{/each}
          </div>
        {/each}
      </div>
      <div class="page">
        {#if tab === "General"}
          <fieldset class="group">
            <legend>System integration</legend>
            <label class="check"><input type="checkbox" bind:checked={s.startWithWindows} /> Launch Kestrel on startup (it waits in the notification area)</label>
            <label class="check"><input type="checkbox" bind:checked={s.monitorClipboard} /> Offer to download links copied to the clipboard</label>
            <label class="check"><input type="checkbox" bind:checked={s.minimizeToTray} /> Keep running in the notification area when closing the window</label>
            <label class="check"><input type="checkbox" bind:checked={s.confirmDelete} /> Confirm before deleting downloads</label>
          </fieldset>
          <fieldset class="group">
            <legend>Browser integration</legend>
            <label class="check"><input type="checkbox" bind:checked={s.captureEnabled} /> Take downloads from Chrome, Edge, Brave and Firefox</label>
            <label class="check"><input type="checkbox" bind:checked={s.videoPanel} /> Show a download button over videos in browsers</label>
            <div class="line">
              <span class="muted">Hold</span>
              <select class="input key" bind:value={s.bypassKey}>{#each keys as k}<option value={k}>{k}</option>{/each}</select>
              <span class="muted">to let the browser download a link, and</span>
              <select class="input key" bind:value={s.forceKey}>{#each keys as k}<option value={k}>{k}</option>{/each}</select>
              <span class="muted">to make Kestrel take it.</span>
            </div>
            <div class="line">
              <button class="btn" onclick={async () => { try { error = ""; bridgeNote = await call<string>("register_browsers"); checks = await call("test_browsers"); } catch (e) { error = String(e); } }}>Connect browsers</button>
              <button class="btn" onclick={async () => (checks = await call("test_browsers"))}>Test browser integration...</button>
            </div>
            {#if bridgeNote}<p class="note">{bridgeNote}</p>{/if}
            {#if checks.length}
              <div class="table">
                {#each checks as c}<div class="trow"><span class={c.ok ? "good" : "bad"}>{c.ok ? "✓" : "✗"}</span><span>{c.browser}</span><span class="muted">{c.detail}</span></div>{/each}
              </div>
            {/if}
          </fieldset>
          <div class="form">
            <label for="language">Language</label>
            <select id="language" class="input" bind:value={s.language}>{#each languages as l}<option value={l.code}>{l.name}</option>{/each}</select>
            <label for="theme">Appearance</label>
            <select id="theme" class="input" bind:value={s.theme}><option value="dark">Dark</option><option value="light">Light</option></select>
            <label for="size">Interface size</label>
            <select id="size" class="input short" bind:value={s.interfaceSize}>{#each [80, 90, 100, 110, 125, 150, 175, 200] as n}<option value={n}>{n} %</option>{/each}</select>
          </div>
        {:else if tab === "File types"}
          <fieldset class="group">
            <legend>Downloaded file types</legend>
            <label class="muted" for="ext">Automatically start downloading the following file types:</label>
            <textarea id="ext" class="input" rows="4" bind:value={s.captureExtensions} spellcheck="false"></textarea>
          </fieldset>
          <fieldset class="group">
            <legend>Sites and addresses left to the browser</legend>
            <label class="muted" for="excluded">Don't take downloads from these sites (one per line):</label>
            <textarea id="excluded" class="input" rows="3" bind:value={s.excludedSites} spellcheck="false"></textarea>
            <label class="muted" for="addresses">Don't take these addresses (one per line, * for any text):</label>
            <textarea id="addresses" class="input" rows="3" bind:value={s.excludedAddresses} placeholder="*.example.com/updates/*" spellcheck="false"></textarea>
          </fieldset>
        {:else if tab === "Save to"}
          <div class="form">
            <label for="dir">Default download directory</label>
            <div class="row"><input id="dir" class="input" bind:value={s.downloadDir} spellcheck="false" /><button class="btn" onclick={browse}>Browse...</button></div>
          </div>
          <label class="check"><input type="checkbox" bind:checked={s.categoryFolders} /> Save each category in its own folder</label>
          <fieldset class="group">
            <legend>Categories and their folders</legend>
            <div class="table">
              {#each categories as c}
                <div class="trow cat"><span>{c}</span><span class="path" title={folderOf(c)}>{folderOf(c)}</span>
                  <button class="btn small" onclick={() => browseCategory(c)}>Change...</button>
                  <button class="btn small" disabled={!s.categoryDirs[c]} onclick={() => { const d = { ...s!.categoryDirs }; delete d[c]; s!.categoryDirs = d; }} title="Use the default folder again">Reset</button>
                </div>
              {/each}
            </div>
          </fieldset>
          <label class="check"><input type="checkbox" bind:checked={s.setCreationDate} /> Set the file's date as provided by the server</label>
          <p class="note">Temporary directory: not needed. Kestrel writes every connection straight into the final file, so there is no assembling step and no copy.</p>
        {:else if tab === "Downloads"}
          <fieldset class="group">
            <legend>Default download settings</legend>
            <label class="check"><input type="checkbox" bind:checked={s.showStartDialog} /> Show start download dialog</label>
            <label class="check"><input type="checkbox" bind:checked={s.onlyAddToQueue} /> Do not start downloading, only add files to the queue</label>
            <label class="check"><input type="checkbox" bind:checked={s.showProgressWindow} /> Show the progress window when a download starts</label>
            <label class="check"><input type="checkbox" bind:checked={s.showCompleteDialog} /> Show download complete dialog</label>
            <div class="form">
              <label for="dup">If a duplicate download link is added</label>
              <select id="dup" class="input" bind:value={s.duplicateAction}>
                <option value={0}>Add the duplicate with a numbered file name</option>
                <option value={1}>Add the duplicate and overwrite the existing file</option>
                <option value={2}>Don't download it again</option>
              </select>
              <label for="retries">Retries</label><input id="retries" class="input short" type="number" min="0" max="20" bind:value={s.maxRetries} />
              <label for="ua">User-Agent</label>
              <input id="ua" class="input" bind:value={s.userAgent} placeholder="Kestrel's built-in identity (a current Chrome)" spellcheck="false" />
            </div>
          </fieldset>
          <fieldset class="group">
            <legend>Virus checking settings</legend>
            <div class="form">
              <label for="scanner">Scanner</label>
              <div class="row"><input id="scanner" class="input" bind:value={s.virusScanner} placeholder="None" spellcheck="false" /><button class="btn" onclick={pickScanner}>Browse...</button></div>
              <label for="scanargs">Arguments</label>
              <input id="scanargs" class="input" bind:value={s.virusScannerArgs} spellcheck="false" disabled={!s.virusScanner.trim()} />
            </div>
            <p class="note">Runs on every finished file; {"{file}"} is replaced by its path. If the scanner reports a problem, the file is renamed to ….blocked.</p>
          </fieldset>
        {:else if tab === "Connection"}
          <fieldset class="group">
            <legend>Connections and limits</legend>
            <div class="form">
              <label for="conn">Max. connections number</label>
              <select id="conn" class="input short" bind:value={s.connections}>{#each [1, 2, 4, 8, 12, 16, 24, 32] as n}<option value={n}>{n}</option>{/each}</select>
              <label for="limit">Speed limit</label>
              <div class="row"><input id="limit" class="input short" type="number" min="0" bind:value={limitKb} /> <span class="muted">KB/s for all downloads (0 = no limit)</span></div>
            </div>
          </fieldset>
          <fieldset class="group">
            <legend>Exceptions: servers that allow fewer connections</legend>
            <div class="table">
              {#each limits as [host, n]}
                <div class="trow exc"><span>{host}</span><input class="input num" type="number" min="1" max="32" value={n} onchange={(e) => { s!.hostLimits = { ...s!.hostLimits, [host]: Math.max(1, Number((e.currentTarget as HTMLInputElement).value)) }; }} aria-label="Connections" /><button class="btn small" onclick={() => { const h = { ...s!.hostLimits }; delete h[host]; s!.hostLimits = h; }}>Delete</button></div>
              {:else}
                <p class="note">None yet. When a server refuses extra connections, Kestrel remembers how many work.</p>
              {/each}
            </div>
            <div class="line">
              <input class="input" bind:value={newHost} placeholder="Server, e.g. files.example.com" spellcheck="false" />
              <input class="input num" type="number" min="1" max="32" bind:value={newHostN} aria-label="Connections" />
              <button class="btn" disabled={!newHost.trim()} onclick={() => { s!.hostLimits = { ...s!.hostLimits, [newHost.trim().toLowerCase()]: Math.max(1, Number(newHostN)) }; newHost = ""; }}>New exception</button>
            </div>
          </fieldset>
          <fieldset class="group">
            <legend>VPN</legend>
            <div class="form">
              <label for="vpn">Windows VPN</label>
              <input id="vpn" class="input" bind:value={s.vpnProfile} placeholder="Name of a VPN in Windows Settings > Network > VPN" spellcheck="false" />
            </div>
            <label class="check"><input type="checkbox" bind:checked={s.connectVpn} disabled={!s.vpnProfile.trim()} /> Connect it before downloads start</label>
            <label class="check"><input type="checkbox" bind:checked={s.disconnectVpnWhenIdle} disabled={!s.vpnProfile.trim()} /> Disconnect it when nothing is downloading</label>
          </fieldset>
        {:else if tab === "Updates"}
          <label class="check"><input type="checkbox" bind:checked={s.autoCheckUpdates} /> Check for new versions of Kestrel every day</label>
          <label class="check"><input type="checkbox" bind:checked={s.autoUpdateTools} /> Keep the video downloader (yt-dlp) up to date</label>
          <div class="form"><label for="feed">Update feed</label><input id="feed" class="input" bind:value={s.updateFeed} placeholder="Official Kestrel releases on GitHub" spellcheck="false" /></div>
        {:else if tab === "Proxy / Socks"}
          <fieldset class="group">
            <legend>Proxy / socks configuration</legend>
            <label class="check"><input type="radio" name="pm" value={0} bind:group={s.proxyMode} /> No proxy/socks</label>
            <label class="check"><input type="radio" name="pm" value={1} bind:group={s.proxyMode} /> Use system settings</label>
            <label class="check"><input type="radio" name="pm" value={2} bind:group={s.proxyMode} /> Manual proxy/socks configuration</label>
            <div class="form indent">
              <label for="proxy">Address</label>
              <input id="proxy" class="input" bind:value={s.proxy} placeholder="10.0.0.1:8080, http://host:port or socks5://host:1080" spellcheck="false" disabled={s.proxyMode != 2} />
              <label for="puser">UserName</label>
              <input id="puser" class="input" bind:value={s.proxyUsername} autocomplete="off" spellcheck="false" disabled={s.proxyMode != 2} />
              <label for="ppass">Password</label>
              <input id="ppass" class="input" type="password" bind:value={s.proxyPassword} autocomplete="off" disabled={s.proxyMode != 2} />
            </div>
            <p class="note">Used for files, videos and torrent trackers. Saved encrypted for your Windows account.</p>
          </fieldset>
        {:else if tab === "Sites Logins"}
          <fieldset class="group">
            <legend>User names and passwords for servers/sites</legend>
            <div class="table">
              <div class="trow login head"><span></span><span>Site/path</span><span>User</span><span>Password</span><span></span></div>
              {#each s.siteLogins as login, i}
                <div class="trow login">
                  <input type="checkbox" bind:checked={login.enabled} aria-label="Use" />
                  <input class="input" bind:value={login.origin} placeholder="https://files.example.com/" spellcheck="false" />
                  <input class="input" bind:value={login.username} autocomplete="off" spellcheck="false" />
                  <input class="input" type="password" bind:value={login.password} autocomplete="off" />
                  <button class="btn small" onclick={() => { s!.siteLogins = s!.siteLogins.filter((_, j) => j !== i); }}>Delete</button>
                </div>
              {/each}
            </div>
            <div><button class="btn" onclick={() => { s!.siteLogins = [...s!.siteLogins, { enabled: true, origin: "https://", username: "", password: "" }]; }}>New login</button></div>
            <p class="note">Sent when a download from the site asks for a password (unless the browser already sent one). Saved encrypted for your Windows account.</p>
          </fieldset>
        {:else if tab === "Rules"}
          <p class="note">The first rule that matches a new download decides where it goes and how. Empty conditions match everything.</p>
          {#each s.rules as rule, i}
            <fieldset class="group">
              <div class="line">
                <input type="checkbox" bind:checked={rule.enabled} aria-label="Use" />
                <input class="input" bind:value={rule.name} aria-label="Rule name" />
                <button class="btn small" disabled={i === 0} onclick={() => { const r = [...s!.rules]; [r[i - 1], r[i]] = [r[i], r[i - 1]]; s!.rules = r; }} title="Check this rule earlier">↑</button>
                <button class="btn small" onclick={() => { s!.rules = s!.rules.filter((_, j) => j !== i); }}>Delete</button>
              </div>
              <div class="form">
                <label for="rd{i}">If the site has</label><input id="rd{i}" class="input" bind:value={rule.domain} placeholder="example.com" spellcheck="false" />
                <label for="re{i}">and the type is</label><input id="re{i}" class="input" bind:value={rule.extensions} placeholder="iso, mkv, zip" spellcheck="false" />
                <label for="rp{i}">and the address matches</label><input id="rp{i}" class="input" bind:value={rule.urlPattern} placeholder="a regular expression (optional)" spellcheck="false" />
                <label for="rf{i}">save in</label><input id="rf{i}" class="input" bind:value={rule.folder} placeholder="the category's folder" spellcheck="false" />
                <label for="rc{i}">as category</label>
                <select id="rc{i}" class="input" bind:value={rule.category}><option value="">(by file type)</option>{#each categories as c}<option value={c}>{c}</option>{/each}</select>
                <label for="rq{i}">in queue</label>
                <select id="rq{i}" class="input" bind:value={rule.queue}><option value="">Main download queue</option>{#each s.queues as q}<option value={q.name}>{q.name}</option>{/each}</select>
                <label for="rn{i}">connections</label>
                <select id="rn{i}" class="input short" bind:value={rule.connections}><option value={null}>default</option>{#each [1, 2, 4, 8, 16, 32] as n}<option value={n}>{n}</option>{/each}</select>
                <label for="rl{i}">speed limit</label>
                <div class="row"><input id="rl{i}" class="input short" type="number" min="0" value={rule.speedLimit ? Math.round(rule.speedLimit / 1024) : ""} placeholder="none" oninput={(e) => { const kb = Number((e.currentTarget as HTMLInputElement).value); rule.speedLimit = kb > 0 ? kb * 1024 : null; }} /> <span class="muted">KB/s</span></div>
              </div>
            </fieldset>
          {/each}
          <div><button class="btn" onclick={() => { s!.rules = [...s!.rules, { name: "New rule", enabled: true, domain: "", extensions: "", urlPattern: "", folder: "", category: "", queue: "", connections: null, speedLimit: null }]; }}>Add a rule</button></div>
        {:else if tab === "Sounds"}
          <fieldset class="group">
            <legend>Select sounds for Kestrel events</legend>
            <div class="table">
              {#each soundNames as name, i}
                <div class="trow snd">
                  <label class="check"><input type="checkbox" bind:checked={s.sounds[i].on} /> {name}</label>
                  <input class="input" bind:value={s.sounds[i].file} placeholder="Windows' sound" spellcheck="false" disabled={!s.sounds[i].on} />
                  <button class="btn small" onclick={() => pickSound(i)} disabled={!s.sounds[i].on}>Browse...</button>
                </div>
              {/each}
            </div>
          </fieldset>
        {:else if tab === "Regional"}
          <fieldset class="group">
            <legend>Dates and times</legend>
            <div class="form">
              <label for="dorder">Date order</label>
              <select id="dorder" class="input" bind:value={s.dateOrder}><option value="auto">As Windows</option><option value="dmy">Day, month, year</option><option value="mdy">Month, day, year</option><option value="ymd">Year, month, day</option></select>
              <label for="clock">Clock</label>
              <select id="clock" class="input" bind:value={s.clock}><option value="auto">As Windows</option><option value="12">12-hour (9:41 PM)</option><option value="24">24-hour (21:41)</option></select>
              <label for="fday">First day of the week</label>
              <select id="fday" class="input" bind:value={s.firstDay}><option value={1}>Monday</option><option value={6}>Saturday</option><option value={0}>Sunday</option></select>
            </div>
            <p class="note">Used by the list of downloads and the Scheduler.</p>
          </fieldset>
        {:else if tab === "Archives"}
          <fieldset class="group">
            <legend>Archives</legend>
            <label class="check"><input type="checkbox" bind:checked={s.extractArchives} /> Extract archives when their download completes (zip; 7z and rar with 7-Zip)</label>
            <label class="check indent"><input type="checkbox" bind:checked={s.deleteExtractedArchives} disabled={!s.extractArchives} /> Then move the archives to the Recycle Bin</label>
            <span class="muted">What an archive holds goes:</span>
            <label class="check indent"><input type="radio" name="at" value={0} bind:group={s.archiveTarget} disabled={!s.extractArchives} /> Into a folder named after it, when it holds several things</label>
            <label class="check indent"><input type="radio" name="at" value={1} bind:group={s.archiveTarget} disabled={!s.extractArchives} /> Always into a folder named after it</label>
            <label class="check indent"><input type="radio" name="at" value={2} bind:group={s.archiveTarget} disabled={!s.extractArchives} /> Straight into the archive's folder</label>
          </fieldset>
        {/if}
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
      {#if saved}<p class="note">{saved}</p>{/if}
    </div>
  {/if}
  <div class="win-actions">
    <button class="btn primary" onclick={() => save(true)}>OK</button>
    <button class="btn" onclick={() => save(false)}>Apply</button>
    <button class="btn" onclick={() => win.close()}>Cancel</button>
  </div>
</div>

<style>
  .win-body { display: flex; flex-direction: column; gap: 0; min-height: 0; overflow: hidden; }
  .tabrows { display: flex; flex-direction: column; gap: 0; }
  .tabrow { display: flex; gap: 2px; }
  .tab { flex: 1; padding: 5px 8px; border: 1px solid var(--line); border-bottom: 0; border-radius: 3px 3px 0 0; background: var(--raised); color: var(--muted); cursor: default; white-space: nowrap; }
  .tab.on { background: var(--surface); color: var(--text); font-weight: 600; }
  .page { flex: 1; overflow: auto; border: 1px solid var(--line); padding: 12px; display: flex; flex-direction: column; gap: 10px; background: var(--surface); }
  .group { display: flex; flex-direction: column; gap: 6px; }
  .line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .indent { padding-inline-start: 22px; }
  .short { width: 110px; flex: none !important; }
  .key { width: 80px; flex: none !important; }
  .num { width: 70px; flex: none !important; }
  .small { min-width: 0; height: 24px; }
  .table { display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 3px; background: var(--bg); }
  .trow { display: grid; grid-template-columns: 24px 140px 1fr; gap: 8px; align-items: center; padding: 4px 8px; border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent); }
  .trow.cat { grid-template-columns: 100px 1fr auto auto; }
  .trow.exc { grid-template-columns: 1fr 70px auto; }
  .trow.login { grid-template-columns: 20px 1.4fr 1fr 1fr auto; }
  .trow.login.head { color: var(--muted); background: var(--chrome); }
  .trow.snd { grid-template-columns: 250px 1fr auto; }
  .path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); }
  .good { color: var(--good); } .bad { color: var(--bad); }
</style>
