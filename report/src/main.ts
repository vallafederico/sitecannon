import "./style.css";

import type { ReportData } from "./types";
import { h } from "./ui";
import { contentView } from "./views/content";
import { graphView } from "./views/graph";
import { overview } from "./views/overview";
import { structure } from "./views/structure";
import { issues, pages, templates } from "./views/tables";

async function loadData(): Promise<ReportData> {
  const raw = JSON.parse(document.getElementById("sc-data")!.textContent || "null");
  // In `vite dev` the placeholder is still there; load a sample exported with `sitecannon report --json`.
  if (typeof raw === "string") return (await fetch("/sample.json")).json();
  return raw as ReportData;
}

const TABS: [string, string][] = [
  ["overview", "Overview"],
  ["structure", "Structure"],
  ["content", "Content"],
  ["graph", "Link graph"],
  ["templates", "Templates"],
  ["issues", "Issues"],
  ["pages", "Pages"],
];

function host(url: string) {
  try { return new URL(url).host; } catch { return url; }
}

async function start() {
  const data = await loadData();
  const app = document.getElementById("app")!;
  document.title = `${host(data.site.seed)} · SiteCannon report`;

  const saved = localStorage.getItem("sc-theme");
  if (saved) document.documentElement.dataset.theme = saved;
  const themeBtn = h("button", { class: "theme-toggle", type: "button" }, "Theme");
  themeBtn.addEventListener("click", () => {
    const dark = document.documentElement.dataset.theme
      ? document.documentElement.dataset.theme === "dark"
      : matchMedia("(prefers-color-scheme: dark)").matches;
    const next = dark ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem("sc-theme", next);
  });

  const nav = h("nav", { class: "tabs", "aria-label": "Report sections" });
  for (const [key, label] of TABS) nav.append(h("a", { href: `#${key}` }, label));
  const main = h("main");
  app.append(h("header", { class: "top" }, h("span", { class: "brand" }, "SiteCannon"), h("span", { class: "site" }, host(data.site.seed)), nav, themeBtn), main);

  // Views are built once and kept, so the graph layout and table state survive tab switches.
  const cache = new Map<string, HTMLElement[]>();
  function route() {
    const [tab, arg] = (location.hash.slice(1) || "overview").split("/");
    const key = TABS.some(([k]) => k === tab) ? tab : "overview";
    markCurrent(nav, key);
    main.className = key === "graph" ? "wide" : "";
    let els = key === "issues" && arg ? undefined : cache.get(key);
    if (!els) {
      els = build(key, data, arg);
      cache.set(key, els);
    }
    main.replaceChildren(...els);
    window.scrollTo({ top: 0 });
  }
  window.addEventListener("hashchange", route);
  route();
}

function markCurrent(nav: HTMLElement, key: string) {
  nav.querySelectorAll("a").forEach((a) => {
    if (a.getAttribute("href") === `#${key}`) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  });
}

function build(key: string, d: ReportData, arg?: string): HTMLElement[] {
  switch (key) {
    case "structure": return structure(d);
    case "content": return contentView(d);
    case "graph": return graphView(d);
    case "templates": return templates(d);
    case "issues": return issues(d, arg);
    case "pages": return pages(d);
    default: return overview(d);
  }
}

start();
