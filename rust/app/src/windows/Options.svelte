<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { call, applyTheme, categories, type Settings } from "../lib/api";

  let s: Settings | null = $state(null);
  let tab = $state("General");
  let error = $state("");
  let limitKb = $state(0);
  const tabs = ["General", "Save to", "Downloads", "Connection", "Site logins", "Rules", "Browser", "Updates"];
  async function pickScanner() {
    const chosen = await pickFolder({ title: "Virus scanner program", filters: [{ name: "Programs", extensions: ["exe", "bat", "cmd"] }] });
    if (typeof chosen === "string" && s) s.virusScanner = chosen;
  }
  const win = getCurrentWindow();

  onMount(() => { call<Settings>("get_settings").then((v) => { s = v; limitKb = Math.round(v.speedLimit / 1024); applyTheme(v.theme); }); });
  async function browse() {
    const chosen = await pickFolder({ directory: true, defaultPath: s?.downloadDir, title: "Download folder" });
    if (typeof chosen === "string" && s) s.downloadDir = chosen;
  }
  async function save(close: boolean) {
    if (!s) return;
    error = "";
    try {
      await call("save_settings", { settings: { ...s, speedLimit: Math.max(0, limitKb) * 1024, connections: Number(s.connections), maxRetries: Number(s.maxRetries) } });
      applyTheme(s.theme);
      if (close) win.close();
    } catch (e) { error = String(e); }
  }
  const limits = $derived(s ? Object.entries(s.hostLimits) : []);
</script>

<div class="win">
  {#if s}
    <div class="layout">
      <nav class="tabs" aria-label="Option pages">
        {#each tabs as t}<button class="tab" class:on={tab === t} onclick={() => (tab = t)}>{t}</button>{/each}
      </nav>
      <div class="win-body">
        {#if tab === "General"}
          <h2>General</h2>
          <label class="check"><input type="checkbox" bind:checked={s.startWithWindows} /> Start Flux when I sign in to Windows (in the tray)</label>
          <label class="check"><input type="checkbox" bind:checked={s.minimizeToTray} /> Closing the main window keeps Flux running in the tray</label>
          <label class="check"><input type="checkbox" bind:checked={s.monitorClipboard} /> Offer download links I copy</label>
          <label class="check"><input type="checkbox" bind:checked={s.playSounds} /> Play a sound when a download finishes</label>
          <div class="form">
            <label for="theme">Appearance</label>
            <select id="theme" class="input" bind:value={s.theme}><option value="dark">Dark</option><option value="light">Light</option></select>
          </div>
        {:else if tab === "Save to"}
          <h2>Save to</h2>
          <div class="form">
            <label for="dir">Download folder</label>
            <div class="row"><input id="dir" class="input" bind:value={s.downloadDir} spellcheck="false" /><button class="btn" onclick={browse}>Browse…</button></div>
          </div>
          <label class="check"><input type="checkbox" bind:checked={s.categoryFolders} /> Save each category in its own folder (Video, Music, Programs…)</label>
        {:else if tab === "Downloads"}
          <h2>Downloads</h2>
          <label class="check"><input type="checkbox" bind:checked={s.showStartDialog} /> Show the start dialog (file name, folder, size) before downloading</label>
          <label class="check"><input type="checkbox" bind:checked={s.showProgressWindow} /> Show the progress window when a download starts</label>
          <label class="check"><input type="checkbox" bind:checked={s.showCompleteDialog} /> Show the "download complete" window</label>
          <div class="form">
            <label for="retries">Retries</label><input id="retries" class="input short" type="number" min="0" max="20" bind:value={s.maxRetries} />
          </div>
          <fieldset class="group">
            <legend>Virus scan</legend>
            <div class="form">
              <label for="scanner">Scanner</label>
              <div class="row"><input id="scanner" class="input" bind:value={s.virusScanner} placeholder="None" spellcheck="false" /><button class="btn" onclick={pickScanner}>Browse…</button></div>
              <label for="scanargs">Arguments</label>
              <input id="scanargs" class="input" bind:value={s.virusScannerArgs} spellcheck="false" disabled={!s.virusScanner.trim()} />
            </div>
            <p class="note">Runs on every finished file; {"{file}"} is replaced by its path. If the scanner reports a problem, the file is renamed to ….blocked. Windows Defender: <code>C:\Program Files\Windows Defender\MpCmdRun.exe</code> with <code>-Scan -ScanType 3 -File "{"{file}"}" -DisableRemediation</code></p>
          </fieldset>
        {:else if tab === "Connection"}
          <h2>Connection</h2>
          <div class="form">
            <label for="conn">Connections per file</label>
            <select id="conn" class="input short" bind:value={s.connections}>{#each [1, 2, 4, 8, 16, 24, 32] as n}<option value={n}>{n}</option>{/each}</select>
            <label for="limit">Speed limit</label>
            <div class="row"><input id="limit" class="input short" type="number" min="0" bind:value={limitKb} /> <span class="muted">KB/s for all downloads (0 = no limit)</span></div>
          </div>
          <fieldset class="group">
            <legend>Proxy</legend>
            <div class="form">
              <label for="proxy">Address</label>
              <input id="proxy" class="input" bind:value={s.proxy} placeholder="None: connect directly (e.g. 10.0.0.1:8080 or socks5://host:1080)" spellcheck="false" />
              <label for="puser">User name</label>
              <input id="puser" class="input" bind:value={s.proxyUsername} autocomplete="off" spellcheck="false" disabled={!s.proxy.trim()} />
              <label for="ppass">Password</label>
              <input id="ppass" class="input" type="password" bind:value={s.proxyPassword} autocomplete="off" disabled={!s.proxy.trim()} />
            </div>
            <p class="note">Used for files, videos and torrent trackers. Saved encrypted for your Windows account.</p>
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
          <div class="form">
            <label for="ua">Browser identity</label>
            <input id="ua" class="input" bind:value={s.userAgent} placeholder="Flux's built-in identity (a current Chrome)" spellcheck="false" />
          </div>
          <fieldset class="group">
            <legend>Sites that allow fewer connections</legend>
            {#if limits.length === 0}<p class="note">None yet. When a site refuses extra connections, Flux remembers how many work and uses that next time.</p>{/if}
            {#each limits as [host, n]}
              <div class="row"><span class="host">{host}</span><span class="muted">{n} connection{n === 1 ? "" : "s"}</span><button class="btn small" onclick={() => { delete s!.hostLimits[host]; s!.hostLimits = { ...s!.hostLimits }; }}>Forget</button></div>
            {/each}
          </fieldset>
        {:else if tab === "Site logins"}
          <h2>Site logins</h2>
          <p class="note">Flux signs in with these when a download from the site asks for a password (unless the browser already sent one). Saved encrypted for your Windows account.</p>
          {#each s.siteLogins as login, i}
            <fieldset class="group">
              <div class="row">
                <label class="check"><input type="checkbox" bind:checked={login.enabled} /> Use</label>
                <div class="left"></div>
                <button class="btn small" onclick={() => { s!.siteLogins = s!.siteLogins.filter((_, j) => j !== i); }}>Remove</button>
              </div>
              <div class="form">
                <label for="lo{i}">Site</label><input id="lo{i}" class="input" bind:value={login.origin} placeholder="https://files.example.com/" spellcheck="false" />
                <label for="lu{i}">User name</label><input id="lu{i}" class="input" bind:value={login.username} autocomplete="off" spellcheck="false" />
                <label for="lp{i}">Password</label><input id="lp{i}" class="input" type="password" bind:value={login.password} autocomplete="off" />
              </div>
            </fieldset>
          {/each}
          <div><button class="btn" onclick={() => { s!.siteLogins = [...s!.siteLogins, { enabled: true, origin: "https://", username: "", password: "" }]; }}>Add a login</button></div>
        {:else if tab === "Rules"}
          <h2>Rules</h2>
          <p class="note">The first rule that matches a new download decides where it goes and how. Empty conditions match everything.</p>
          {#each s.rules as rule, i}
            <fieldset class="group">
              <div class="row">
                <label class="check"><input type="checkbox" bind:checked={rule.enabled} /></label>
                <input class="input" bind:value={rule.name} aria-label="Rule name" />
                <button class="btn small" disabled={i === 0} onclick={() => { const r = [...s!.rules]; [r[i - 1], r[i]] = [r[i], r[i - 1]]; s!.rules = r; }} title="Check this rule earlier">↑</button>
                <button class="btn small" onclick={() => { s!.rules = s!.rules.filter((_, j) => j !== i); }}>Remove</button>
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
        {:else if tab === "Browser"}
          <h2>Browser</h2>
          <label class="check"><input type="checkbox" bind:checked={s.captureEnabled} /> Take downloads from the browser</label>
          <label class="check"><input type="checkbox" bind:checked={s.videoPanel} /> Show the "Download this video" button over videos</label>
          <label class="muted" for="ext">File types to take</label>
          <textarea id="ext" class="input" rows="4" bind:value={s.captureExtensions} spellcheck="false"></textarea>
          <label class="muted" for="excluded">Never take downloads from these sites (one per line)</label>
          <textarea id="excluded" class="input" rows="3" bind:value={s.excludedSites} spellcheck="false"></textarea>
        {:else if tab === "Updates"}
          <h2>Updates</h2>
          <label class="check"><input type="checkbox" bind:checked={s.autoCheckUpdates} /> Check for new versions of Flux every day</label>
          <label class="check"><input type="checkbox" bind:checked={s.autoUpdateTools} /> Keep the video downloader (yt-dlp) up to date</label>
          <div class="form"><label for="feed">Update feed</label><input id="feed" class="input" bind:value={s.updateFeed} placeholder="Official Flux releases on GitHub" spellcheck="false" /></div>
        {/if}
        {#if error}<p class="error-text">{error}</p>{/if}
      </div>
    </div>
  {/if}
  <div class="win-actions">
    <button class="btn primary" onclick={() => save(true)}>OK</button>
    <button class="btn" onclick={() => save(false)}>Apply</button>
    <button class="btn" onclick={() => win.close()}>Cancel</button>
  </div>
</div>

<style>
  .layout { display: grid; grid-template-columns: 150px 1fr; min-height: 0; }
  .tabs { display: flex; flex-direction: column; padding: 10px 6px; gap: 2px; background: var(--chrome); border-right: 1px solid var(--line); }
  .tab { text-align: left; height: 28px; padding: 0 10px; border: 0; border-radius: 4px; background: transparent; cursor: default; }
  .tab:hover { background: var(--raised); }
  .tab.on { background: var(--selection); color: var(--selection-text); }
  h2 { margin: 0 0 4px; font-size: 14px; }
  .short { width: 110px; flex: none !important; }
  .host { flex: 1; }
  .small { min-width: 0; height: 24px; }
</style>
