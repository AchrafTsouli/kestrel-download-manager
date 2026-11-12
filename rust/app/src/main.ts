import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { setLanguage } from "./lib/i18n";
import { applyLook } from "./lib/api";
import "./theme.css";
import "./lib/controls.css";
// Every window loads this page; its label says which screen it is ("progress-<id>", "options", …).
const label = getCurrentWindow().label;
const [kind, ...rest] = label.split("-");
const id = rest.join("-");
const target = document.getElementById("app")!;
// Each window loads only its own screen's code.
const screens: Record<string, () => Promise<{ default: any }>> = {
  progress: () => import("./windows/Progress.svelte"),
  complete: () => import("./windows/Complete.svelte"),
  start: () => import("./windows/Start.svelte"),
  options: () => import("./windows/Options.svelte"),
  scheduler: () => import("./windows/Scheduler.svelte"),
  batch: () => import("./windows/Batch.svelte"),
  video: () => import("./windows/Video.svelte"),
  torrent: () => import("./windows/Torrent.svelte"),
  grabber: () => import("./windows/Grabber.svelte"),
  extension: () => import("./windows/Extension.svelte"),
};
const screen = (screens[kind] ?? (() => import("./Main.svelte")))();
// The language is set before the window is drawn, and follows Options while it is open.
const win = getCurrentWindow();
invoke<{ language: string; interfaceSize: number; dateOrder: string; clock: string }>("get_settings")
  .then((s) => { applyLook(s); setLanguage(s.language); })
  .catch(() => setLanguage("en"))
  .finally(() => {
    screen.then(({ default: Screen }) => mount(Screen, { target, props: { id } }));
    win.title().then((title) => win.setTitle(title)).catch(() => {});
  });
listen<{ language: string; interfaceSize: number; dateOrder: string; clock: string }>("settings", ({ payload }) => { applyLook(payload); setLanguage(payload.language); });
