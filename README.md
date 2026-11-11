<p align="center"><img src="rust/KESTREL_Icon_Pack/app/png/kestrel-128.png" width="96" alt="Kestrel icon"></p>

<h1 align="center">Kestrel Download Manager</h1>

<p align="center">A fast, free download manager for Windows, with browser capture, video downloads and torrents.<br>No ads, no trial timer, no account.</p>

<p align="center"><a href="https://github.com/AchrafTsouli/flux-downloader/releases/latest"><b>Download the latest installer</b></a></p>

Kestrel is the new name of Flux. Version 2.0 is a rewrite in Rust: smaller, faster to start and
lighter on memory, with the same windows you know from IDM. It installs over Flux 1.x and keeps your
downloads, settings and site logins.

## Features

- **Fast downloads:** 8 connections per file by default (up to 32). A free connection takes over half of the slowest remaining part, so speed holds until the last byte; a dropped connection picks up its part again at once.
- **No double disk space:** every connection writes straight into one file at its final size, with no assembling step and no copy at the end.
- **Reliable resume** after a pause, a crash, a restart or a lost connection, with a disk-space check before starting.
- **Browser capture:** the extension for Chrome, Edge and Brave (Opera and Vivaldi accept it too) hands links, downloads and magnet links to Kestrel, with the cookies and page they need. Pause it per site from its button.
- **Videos:** a "Download this video" button over playing videos; pick a quality with its audio on YouTube and many other sites, playlists, subtitles, audio only, and HLS/DASH streams.
- **Torrents:** magnet links and .torrent files; choose which files to download.
- **Download pages:** links that answer with a "your download will start…" page are followed to the file, like a browser.
- **IDM-style windows:** start dialog with zip preview and the program's own logo before downloading, progress window with per-connection details, background downloads in the tray, download list with filters and Windows file icons.
- **Queues and scheduler:** start and stop times, weekdays, files at the same time, and exit, sleep or shut down when done.
- **Speed limiter,** for all downloads or one file.
- **Archives:** unpack zip (and 7z/rar with 7-Zip) when the download completes.
- **Import and export** download lists, IDM export files included.
- **English, French and Arabic** (right to left), light and dark themes, interface size up to 200 %.
- **Private by design:** history and site logins are encrypted for your Windows account; the browser talks to Kestrel only through a Windows named pipe, never a local web server.

## Install

1. Download `Kestrel-Setup-<version>-win-x64.exe` from [Releases](https://github.com/AchrafTsouli/flux-downloader/releases/latest) and run it. It needs 64-bit Windows 10 22H2 or Windows 11, with Microsoft Edge WebView2 (built into Windows 11 and current Windows 10), and no administrator rights.
2. On first start, Kestrel opens **Add the browser extension**: open your browser's extensions page from there, turn on **Developer mode**, choose **Load unpacked** and select the folder it shows. You can open the guide again from **Help → Add the browser extension…**.

Windows SmartScreen may warn about the installer because it is not code-signed yet. Choose **More info → Run anyway**.

## Build from source

Requirements: Windows, [Rust](https://rustup.rs) (stable, 1.88 or newer), [Node.js 22](https://nodejs.org) and [PowerShell 7.4+](https://learn.microsoft.com/powershell/scripting/install/installing-powershell-on-windows).

```powershell
git clone https://github.com/AchrafTsouli/flux-downloader.git
cd flux-downloader
pwsh scripts/Get-VideoTools.ps1           # yt-dlp, FFmpeg, QuickJS and aria2, checked against video-tools/sources.json
cd rust/app; npm ci; cd ../..
pwsh scripts/Build-Test.ps1               # quick build: rust/target/release/flux.exe
```

The installer: `pwsh scripts/Install-InnoCompiler.ps1` once, then `pwsh scripts/Build-Flux2Installer.ps1`. The result is written to `installers/`.

Tests:

```powershell
cd rust; cargo test --workspace           # engine, storage, updates, app
node --test BrowserExtension/*.test.js    # browser extension
```

## Releasing a new version

1. Raise `version` in `rust/Cargo.toml`, commit and push.
2. `pwsh scripts/Publish-Release.ps1` builds the installer, signs `version.json` with the release key (`tools/release-signer`) and publishes both as a GitHub release. A version with a suffix (`2.1.0-beta.1`) is published as a pre-release.

Installed copies (Kestrel and Flux 1.x) check `releases/latest/download/version.json` and only install a release signed with the project's key. The private key stays on the release computer; it never goes into the repository.

## Project layout

| Folder | Contents |
|---|---|
| `rust/app/` | The desktop app: Tauri 2 shell (`src-tauri/`) and Svelte windows (`src/`) |
| `rust/crates/flux-engine/` | Download engine: segmented HTTP, one sparse file, resume, limits |
| `rust/crates/flux-store/` | History and settings (SQLite), encrypted for the Windows account |
| `rust/crates/flux-media/`, `flux-torrent/` | Videos (yt-dlp, FFmpeg) and torrents (aria2) |
| `rust/crates/flux-ipc/`, `flux-nmhost/` | Browser bridge: native messaging host and named pipe |
| `rust/crates/flux-update/`, `flux-cli/` | Signed updates; command-line tool |
| `rust/tools/` | Local test server (`labserver`) and the release signer |
| `BrowserExtension/` | The browser extension |
| `installer/`, `scripts/` | Inno Setup installer and the build and release scripts |

Some internal names (`flux-*`, `com.swiftfetch.bridge`) date from before the rename and are kept so existing installations and the published extension keep working.

## License

Kestrel is released under the [MIT License](LICENSE): anyone may use, modify and redistribute it.

## Third-party components

Kestrel bundles [yt-dlp](https://github.com/yt-dlp/yt-dlp) (Unlicense), a minimal LGPL build of [FFmpeg](https://ffmpeg.org) (build script and source link in `video-tools/`), [QuickJS-ng](https://github.com/quickjs-ng/quickjs) (MIT) and [aria2](https://aria2.github.io) (GPL-2.0, run as a separate program). Their licences are installed in the `licenses` folder.

You are responsible for respecting copyright and the terms of the sites you download from.
