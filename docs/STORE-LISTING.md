# Publishing Kestrel's browser extension

Nothing here is uploaded automatically. The packages are built by:

```
pwsh scripts/Package-Extension.ps1
```

| Store | Package | Fee | Review |
|---|---|---|---|
| Chrome Web Store (also used by Brave, Opera, Vivaldi) | `installers/kestrel-extension-<version>-chromium.zip` | one-time US$5 developer registration | a few days |
| Microsoft Edge Add-ons | the same `-chromium.zip` | free (Partner Center account) | up to 7 days |
| Firefox Add-ons (addons.mozilla.org) | `installers/kestrel-extension-<version>-firefox.zip` | free | automatic signing, then human review |

The Firefox build has not been run in Firefox yet: test it with `about:debugging` > This Firefox > Load Temporary Add-on before uploading.

## After a store accepts it: let the bridge accept the store's ID

The packages carry no fixed `key`, so each store gives the extension its own ID (the developer-mode ID `ljhopjjfadnlpedjofcnnoophngnadml` stays valid for the unpacked copy). The browser bridge only answers IDs it was registered with:

1. Copy the ID from the store's dashboard (Chrome and Edge each give one; Firefox keeps `kestrel@achraftsouli.github.io`).
2. Add it to `allowed_origins` in `installer/Kestrel.iss` (`PrepareToInstall`) next to the developer-mode ID, and to the IDs that `kestrel-nmhost --register` writes.
3. Release a Kestrel update. Until then, users can run `kestrel-nmhost.exe --register <id>` in Kestrel's folder.

## Listing text

**Name:** Kestrel Integration

**Short description (132 characters max):**
Send downloads and videos from your browser to Kestrel, the free download manager for Windows: faster, resumable, in one click.

**Description:**
Kestrel Integration connects your browser to Kestrel, a free, open-source download manager for Windows (MIT licence).

- Downloads you start in the browser are handed to Kestrel, which splits them across several connections, resumes them after a crash or a lost connection, and lets you pause, schedule and limit them.
- A "Download this video" button appears over videos on supported sites: choose the quality, download a whole playlist, or save only part of a video.
- Magnet links open Kestrel's torrent window.
- Choose which file types Kestrel takes, and which sites it never takes from. Hold Alt while clicking to keep a download in the browser.

The extension needs the Kestrel app, available free at https://github.com/AchrafTsouli/kestrel-download-manager. Nothing is sent anywhere else: the extension talks only to Kestrel on your own computer.

**Category:** Productivity (Chrome) · Productivity (Edge) · Download Management (Firefox)

**Single purpose (Chrome):** Hand the user's downloads and the videos they choose to the Kestrel download manager installed on the same computer.

## Permission justifications

| Permission | Why |
|---|---|
| `downloads` | To see a download the browser starts and, when it matches the user's rules, pause it and hand it to Kestrel instead. |
| `nativeMessaging` | The only channel to the Kestrel app on the same computer (host `com.swiftfetch.bridge`). |
| `cookies` | Some sites only allow a download from a signed-in session; the cookies for that one download's site are passed to Kestrel on the same computer so the download works. They are never stored by the extension or sent elsewhere. |
| `webRequest` | To note the headers the browser sent with a download (referring page, language), so Kestrel can repeat the same request. No request is blocked or changed. |
| `storage` | The user's settings (on/off, file types, excluded sites) and short-lived download hand-over state. |
| `contextMenus` | "Download with Kestrel" on links. |
| `scripting` | Adds the video button to pages that were already open when the extension was installed or updated. |
| Host access to http/https pages | The video button and link capture work on any site the user visits; nothing is read beyond the page's video and link addresses. |

**Remote code:** none. All code is in the package.

## Privacy practices (data disclosure forms)

- Collected and transmitted off the device: **nothing**.
- Handled locally: web history (the address of a download or video the user chooses), and authentication information (the cookies for that download's site), passed only to the Kestrel app on the same computer, only to perform the download the user asked for.
- Not sold, not used for advertising, not used for credit decisions, no human reads it.

**Privacy policy URL:** https://github.com/AchrafTsouli/kestrel-download-manager/blob/main/PRIVACY.md (publish `PRIVACY.md` from this repository first).

## Screenshots to prepare (1280 × 800)

1. The video button over a playing video, quality list open.
2. Kestrel's main window with a few downloads and the progress window.
3. The extension popup with its settings.
