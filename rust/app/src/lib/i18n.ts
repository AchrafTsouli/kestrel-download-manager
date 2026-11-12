// Interface languages. The windows are written in English; this translates what they show (text,
// tooltips, placeholders, window titles) from catalogs keyed by the English text, and keeps up as
// the windows change. Anything without a translation stays English. Arabic is laid out right to left.
import { Window } from "@tauri-apps/api/window";
import fr from "../locales/fr.json";
import ar from "../locales/ar.json";

export const languages: { code: string; name: string }[] = [
  { code: "auto", name: "Automatic (Windows language)" },
  { code: "en", name: "English" },
  { code: "fr", name: "Français" },
  { code: "ar", name: "العربية" },
];
const catalogs: Record<string, Record<string, string>> = { fr, ar };
const RTL = new Set(["ar"]);
const ATTRIBUTES = ["title", "placeholder", "aria-label"];

let exact = new Map<string, string>();
let patterns: [RegExp, string][] = [];
// What the patterns gave for each text seen (null: none matched), for the current language.
const matched = new Map<string, string | null>();
let current = "en";
// What each text node or attribute said in English, and what it was last turned into.
const texts = new WeakMap<Node, { english: string; shown: string }>();
const attributes = new WeakMap<Element, Record<string, { english: string; shown: string }>>();

// Size units: French writes octets (Ko, Mo), Arabic its own abbreviations.
const UNITS: Record<string, Record<string, string>> = {
  fr: { B: "o", KB: "Ko", MB: "Mo", GB: "Go", bytes: "octets" },
  ar: { B: "ب", KB: "ك.ب", MB: "م.ب", GB: "ج.ب", bytes: "بايت" },
};
export function unit(name: string): string {
  return UNITS[current]?.[name] ?? name;
}

/** "auto" follows Windows' language when Kestrel has it. */
export function resolve(code: string): string {
  if (code && code !== "auto") return catalogs[code] || code === "en" ? code : "en";
  const windows = (navigator.language || "en").split("-")[0].toLowerCase();
  return catalogs[windows] ? windows : "en";
}

/** A string in the current language: exact entries, then entries with {} placeholders. */
export function t(text: string): string {
  if (current === "en") return text;
  const inner = text.trim();
  if (!inner) return text;
  let found = exact.get(inner);
  if (found === undefined) {
    // Changing values (speeds, times left) come back several times a second: each is matched once.
    const known = matched.get(inner);
    if (known !== undefined) found = known ?? undefined;
    else {
      for (const [regex, template] of patterns) {
        const m = regex.exec(inner);
        if (m) { let i = 1; found = template.replace(/\{\}/g, () => m[i++] ?? ""); break; }
      }
      if (matched.size > 5000) matched.clear();
      matched.set(inner, found ?? null);
    }
  }
  if (found === undefined) return text;
  // Keep the spaces around it (they separate it from the values next to it).
  return text.slice(0, text.indexOf(inner)) + found + text.slice(text.indexOf(inner) + inner.length);
}

function translateText(node: Text) {
  const seen = texts.get(node);
  const english = seen && node.data === seen.shown ? seen.english : node.data;
  const shown = t(english);
  texts.set(node, { english, shown });
  if (node.data !== shown) node.data = shown;
}
function translateAttributes(element: Element) {
  const memory = attributes.get(element) ?? {};
  for (const name of ATTRIBUTES) {
    const value = element.getAttribute(name);
    if (value == null) continue;
    const seen = memory[name];
    const english = seen && value === seen.shown ? seen.english : value;
    const shown = t(english);
    memory[name] = { english, shown };
    if (value !== shown) element.setAttribute(name, shown);
  }
  attributes.set(element, memory);
}
function translateTree(root: Node) {
  if (root.nodeType === Node.TEXT_NODE) return translateText(root as Text);
  if (root.nodeType !== Node.ELEMENT_NODE) return;
  const element = root as Element;
  if (element.tagName === "SCRIPT" || element.tagName === "STYLE" || element.tagName === "TEXTAREA") return;
  translateAttributes(element);
  for (const child of Array.from(element.childNodes)) translateTree(child);
}

let observer: MutationObserver | null = null;
function watch() {
  if (observer) return;
  observer = new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === "characterData") translateText(r.target as Text);
      else if (r.type === "attributes") translateAttributes(r.target as Element);
      else r.addedNodes.forEach(translateTree);
    }
  });
  observer.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ATTRIBUTES });
}

// Window titles set from the code go through the same table.
const setTitle = Window.prototype.setTitle;
Window.prototype.setTitle = function (title: string) { return setTitle.call(this, t(title)); };

export function setLanguage(code: string) {
  const next = resolve(code);
  const catalog = catalogs[next] ?? {};
  exact = new Map();
  patterns = [];
  matched.clear();
  for (const [english, translated] of Object.entries(catalog)) {
    if (!translated) continue;
    if (english.includes("{}")) {
      const source = english.split("{}").map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("(.+?)");
      patterns.push([new RegExp(`^${source}$`), translated]);
    } else exact.set(english, translated);
  }
  // The most specific pattern first: "{} of {} files · {}. Only…" before "{} files · {}".
  patterns.sort((a, b) => b[0].source.replace(/\(\.\+\?\)/g, "").length - a[0].source.replace(/\(\.\+\?\)/g, "").length);
  current = next;
  document.documentElement.lang = next;
  document.documentElement.dir = RTL.has(next) ? "rtl" : "ltr";
  translateTree(document.body);
  watch();
}
