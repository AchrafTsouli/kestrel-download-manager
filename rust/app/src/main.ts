import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
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

// Every window loads this page; its label says which screen it is ("progress-<id>", "options", …).
const label = getCurrentWindow().label;
const [kind, ...rest] = label.split("-");
const id = rest.join("-");
const target = document.getElementById("app")!;
const screens: Record<string, any> = { progress: Progress, complete: Complete, start: Start, options: Options, scheduler: Scheduler, batch: Batch, video: Video };
mount(screens[kind] ?? Main, { target, props: { id } });
