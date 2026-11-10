import { invoke } from "@tauri-apps/api/core";
import { unit } from "./i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { tick } from "svelte";

/** Makes a dialog exactly as tall as what it shows (like WPF's SizeToContent). */
export async function fitHeight() {
  await tick();
  const root = document.querySelector(".win") as HTMLElement | null;
  if (!root) return;
  root.style.height = "auto";
  const height = Math.ceil(root.getBoundingClientRect().height);
  root.style.height = "";
  if (height > 80) await getCurrentWindow().setSize(new LogicalSize(window.innerWidth, height));
}

export type Status = "Queued" | "Connecting" | "Downloading" | "Retrying" | "Finishing" | "Paused" | "Completed" | "Failed";
export type Download = {
  id: string; name: string; url: string; path: string; folder: string; category: string; description: string; queue: string;
  status: Status; kind: "file" | "video" | "torrent"; written: number; total: number | null; bytesPerSecond: number; connections: number;
  segments: { start: number; end: number | null; done: number }[]; resumable: boolean; addedMs: number; lastTryMs: number | null;
  completedMs: number | null; error: string | null; note: string | null; phase: string | null; phaseFraction: number | null;
  estimated: boolean; exists: boolean; speedLimit: number;
};
export type AfterQueue = "nothing" | "exit" | "sleep" | "hibernate" | "shutdown";
export type Queue = { name: string; maxActive: number; scheduleEnabled: boolean; startTime: string | null; stopTime: string | null; days: number[]; after: AfterQueue; retries: number };
export type Settings = {
  downloadDir: string; categoryFolders: boolean; connections: number; speedLimit: number; showStartDialog: boolean; showProgressWindow: boolean;
  showCompleteDialog: boolean; theme: string; language: string; minimizeToTray: boolean; startWithWindows: boolean; monitorClipboard: boolean;
  playSounds: boolean; maxRetries: number; proxy: string; proxyUsername: string; proxyPassword: string; userAgent: string; queues: Queue[]; hostLimits: Record<string, number>;
  updateFeed: string; autoCheckUpdates: boolean; autoUpdateTools: boolean; lastUpdateCheckMs: number; captureExtensions: string;
  captureEnabled: boolean; videoPanel: boolean; excludedSites: string; columns: string[];
  siteLogins: SiteLogin[]; rules: Rule[]; virusScanner: string; virusScannerArgs: string;
  vpnProfile: string; connectVpn: boolean; disconnectVpnWhenIdle: boolean; extractArchives: boolean; deleteExtractedArchives: boolean;
};
export type SiteLogin = { enabled: boolean; origin: string; username: string; password: string };
export type Rule = {
  name: string; enabled: boolean; domain: string; extensions: string; urlPattern: string;
  folder: string; category: string; queue: string; connections: number | null; speedLimit: number | null;
};
export type Probe = { url: string; fileName: string; total: number | null; resumable: boolean; contentType: string | null; category: string; folder: string; error: string | null };

export const call = invoke;
export const categories = ["Compressed", "Documents", "Music", "Programs", "Video", "Images", "Other"];
export const running = (s: Status) => s === "Connecting" || s === "Downloading" || s === "Retrying" || s === "Finishing";
export const resumable = (s: Status) => s === "Paused" || s === "Failed" || s === "Queued";

export function size(bytes: number | null | undefined, empty = "") {
  if (bytes == null) return empty;
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(2) + " " + unit("GB");
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " " + unit("MB");
  if (bytes >= 1024) return Math.round(bytes / 1024) + " " + unit("KB");
  return bytes + " " + unit("B");
}
export const exact = (bytes: number) => bytes.toLocaleString() + " " + unit("bytes");
export const percent = (d: Download) => (d.total ? Math.min(100, (d.written / d.total) * 100) : d.status === "Completed" ? 100 : 0);
export function timeLeft(d: Download) {
  if (!d.total || d.bytesPerSecond <= 0 || !running(d.status)) return "—";
  return clock(Math.round((d.total - d.written) / d.bytesPerSecond));
}
export function clock(seconds: number) {
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(Math.floor(seconds / 3600))}:${two(Math.floor((seconds % 3600) / 60))}:${two(seconds % 60)}`;
}
export const stamp = (ms: number | null) => ms ? new Date(ms).toLocaleString(undefined, { month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }) : "";
export const statusText = (d: Download) => d.status === "Completed" ? (d.exists ? "Complete" : "File missing") : d.status === "Downloading" ? `${percent(d).toFixed(2)}%` : d.status;
export const statusClass = (d: Download) => d.status === "Completed" ? (d.exists ? "good" : "warn") : d.status === "Failed" ? "bad" : d.status === "Paused" ? "warn" : "run";

export function applyTheme(theme: string) {
  document.documentElement.dataset.theme = theme === "light" ? "light" : "dark";
}
