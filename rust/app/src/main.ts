import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { setLanguage } from "./lib/i18n";
import { applyLook } from "./lib/api";
import "./theme.css";
import "./lib/controls.css";
import Main from "./Main.svelte";
import Progress from "./windows/Progress.svelte";
import Complete from "./windows/Complete.svelte";
import Start from "./windows/Start.svelte";
import Options from "./windows/Options.svelte";
import Scheduler from "./windows/Scheduler.svelte";
import Batch from "./windows/Batch.svelte";
import Video from "./windows/Video.svelte";
import Torrent from "./windows/Torrent.svelte";
import Grabber from "./windows/Grabber.svelte";
import Extension from "./windows/Extension.svelte";

// Every window loads this page; its label says which screen it is ("progress-<id>", "options", …).
const label = getCurrentWindow().label;
const [kind, ...rest] = label.split("-");
const id = rest.join("-");
const target = document.getElementById("app")!;
const screens: Record<string, any> = { progress: Progress, complete: Complete, start: Start, options: Options, scheduler: Scheduler, batch: Batch, video: Video, torrent: Torrent, grabber: Grabber, extension: Extension };
// The language is set before the window is drawn, and follows Options while it is open.
const win = getCurrentWindow();
invoke<{ language: string; interfaceSize: number; dateOrder: string; clock: string }>("get_settings")
  .then((s) => { applyLook(s); setLanguage(s.language); })
  .catch(() => setLanguage("en"))
  .finally(() => {
    mount(screens[kind] ?? Main, { target, props: { id } });
    win.title().then((title) => win.setTitle(title)).catch(() => {});
  });
listen<{ language: string; interfaceSize: number; dateOrder: string; clock: string }>("settings", ({ payload }) => { applyLook(payload); setLanguage(payload.language); });
