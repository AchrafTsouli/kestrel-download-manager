# KESTREL icon pack

## App assets
- `app/windows/kestrel.ico`: a multi-resolution Windows icon containing 16, 20, 24, 32, 40, 48, 64, 96, 128, 192 and 256 px frames.
- `app/png/`: transparent PNG exports at 16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256, 512 and 1024 px.
- `app/macos/kestrel.icns`: macOS icon container.
- `app/kestrel-master-1024.png`: padded transparent master.

## Tray recommendation
Use the compact falcon silhouette for the notification area, with a status badge. The globe belongs in the full app icon; the simple silhouette is easier to read at tray sizes.

Choose `tray/dark-taskbar` for a white falcon on dark backgrounds, or `tray/light-taskbar` for a dark falcon on light backgrounds. Each theme includes 16, 20, 24, 32, 40, 48 and 64 px PNG exports and a multi-resolution ICO per state.

| State | Badge |
| --- | --- |
| Idle | None |
| Downloading | Cyan down arrow |
| Paused | Amber pause bars |
| Complete | Green check |
| Error | Red exclamation |

The badge shape communicates state in addition to color. Use one tray icon for the whole application.

Suggested aggregate-state priority: error requiring attention, then active downloads, then pending paused downloads, then recent completion, then idle. A recoverable automatic retry need not replace the downloading state with an error. Show Complete briefly after the queue finishes, then return to Idle. Do not report completion while downloads remain paused. Closing the window should leave transfers running only when the user has enabled background operation.

Suggested tooltip: `KESTREL — 3 downloads • 12.4 MB/s`.
Suggested context menu: Open KESTREL; Add URL; Pause all; Resume all; Speed limit; Open downloads folder; Settings; Quit. Disable actions that do not apply. If Quit would interrupt active transfers, offer to keep downloading or quit and preserve resumable work.

For Windows, load the correct frame for the current DPI rather than stretching a 16 px bitmap. Microsoft recommends providing both 16 and 32 px resources for notification icons and using LoadIconMetric for appropriate loading/scaling:
https://learn.microsoft.com/en-us/windows/win32/shell/notification-area

## Production notes
The colorful app artwork and tray base were generated with the built-in image tool, then exported programmatically. Tray alpha was normalized for monochrome use; status symbols were drawn as geometric badges. The PNG exports are resized masters, not individually hand-hinted pixel art. Files were checked for dimensions, transparency and ICO frames, and previewed against light/dark backgrounds. They have not been tested inside a running Windows application; test the chosen tray icon on actual taskbars and display scales before release.

`sources/` contains the generated masters, the normalized tray silhouette and the generation prompts. Run `sources/build_kestrel_icons.py` to reproduce the exports in a sibling `KESTREL_Icon_Pack_Rebuilt` folder. It requires Python, Pillow, NumPy and SciPy. The selected icons are delivered as PNG/ICO/ICNS, not editable vector artwork.
