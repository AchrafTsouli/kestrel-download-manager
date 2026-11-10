#!/usr/bin/env bash
# Builds Flux 2.0 for Linux (.deb, AppImage) or macOS (.app, .dmg). Run on that system, from the
# repository root, with Rust, Node 20+ and (Linux) the WebKitGTK development packages:
#   sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
# The video tools are not bundled: the package uses the system's FFmpeg and aria2 (Linux: package
# dependencies; macOS: brew install ffmpeg aria2 yt-dlp) and yt-dlp from PATH.
set -euo pipefail
cd "$(dirname "$0")/../rust"
cargo build --release -p flux-nmhost
triple="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p app/src-tauri/binaries
cp "${CARGO_TARGET_DIR:-target}/release/flux-nmhost" "app/src-tauri/binaries/flux-nmhost-$triple"
cd app
if [ "${SKIP_FRONTEND:-}" = "1" ]; then
  cargo tauri build --config '{"build":{"beforeBuildCommand":""}}'
else
  npm ci
  npx tauri build
fi
