// Small DOM toolkit. All text goes in via textContent: titles, anchors and URLs come
// from crawled pages and must never be interpreted as HTML.

import type { Cell, Severity } from "./types";

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, string | number | boolean | EventListener | undefined>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = String(v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  append(el, ...children);
  return el;
}

export function append(el: Element, ...children: Child[]) {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

const SVG_NS = "http://www.w3.org/2000/svg";
export function s<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  return el;
}

export const fmt = {
  int: (n: number | null | undefined) => (n == null ? "–" : Math.round(n).toLocaleString("en-US")),
  compact: (n: number) =>
    n >= 10_000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}K` : Math.round(n).toLocaleString("en-US"),
  pct: (x: number) => `${(x * 100).toFixed(x < 0.1 ? 1 : 0)}%`,
  dec: (n: number | null | undefined, d = 2) => (n == null ? "–" : n.toFixed(d)),
};

/** Shortens a URL to its path for display; the full URL stays in the link and title. */
export function pathOf(url: string): string {
  try {
    const u = new URL(url);
    return (u.pathname + u.search) || "/";
  } catch {
    return url;
  }
}

export function link(url: string, label?: string): HTMLElement {
  const safe = /^https?:\/\//i.test(url);
  if (!safe) return h("span", {}, label ?? url);
  return h("a", { href: url, target: "_blank", rel: "noopener noreferrer", title: url }, label ?? pathOf(url));
}

// --- Status: color is never the only carrier; every badge has an icon and a label. ---

const SEVERITY_LABEL: Record<Severity, string> = {
  critical: "Critical",
  serious: "Serious",
  warning: "Warning",
  info: "Info",
};

export function severityIcon(sev: Severity): SVGSVGElement {
  const svg = s("svg", { viewBox: "0 0 16 16", width: 16, height: 16, class: `sev-icon sev-${sev}`, "aria-hidden": "true" });
  if (sev === "critical") {
    svg.append(s("circle", { cx: 8, cy: 8, r: 7 }));
    svg.append(s("path", { d: "M5.5 5.5l5 5M10.5 5.5l-5 5", class: "glyph" }));
  } else if (sev === "serious" || sev === "warning") {
    svg.append(s("path", { d: "M8 1.5l7 13H1z" }));
    svg.append(s("path", { d: "M8 6v4M8 12v.5", class: "glyph" }));
  } else {
    svg.append(s("circle", { cx: 8, cy: 8, r: 7 }));
    svg.append(s("path", { d: "M8 7v4.5M8 4.5v.5", class: "glyph" }));
  }
  return svg;
}

export function severityBadge(sev: Severity): HTMLElement {
  return h("span", { class: `sev-badge sev-${sev}` }, severityIcon(sev), SEVERITY_LABEL[sev]);
}

// --- Tooltip: one floating element, positioned near the pointer. ---

let tip: HTMLElement | null = null;
export function showTip(x: number, y: number, rows: [string, string][], heading?: string) {
  if (!tip) {
    tip = h("div", { class: "tooltip", role: "tooltip" });
    document.body.append(tip);
  }
  tip.replaceChildren();
  if (heading) tip.append(h("div", { class: "tip-head" }, heading));
  for (const [label, value] of rows) {
    tip.append(h("div", { class: "tip-row" }, h("strong", {}, value), h("span", {}, label)));
  }
  tip.style.display = "block";
  const pad = 14;
  const { width, height } = tip.getBoundingClientRect();
  const left = x + pad + width > window.innerWidth ? x - pad - width : x + pad;
  const top = Math.min(y + pad, window.innerHeight - height - 8);
  tip.style.left = `${Math.max(8, left)}px`;
  tip.style.top = `${Math.max(8, top)}px`;
}
export function hideTip() {
  if (tip) tip.style.display = "none";
}

// --- DataTable: search, sort, pagination over in-memory rows. ---

export interface Column {
  label: string;
  numeric?: boolean;
  /** Renders a cell; defaults to text (URLs become links). */
  render?: (value: Cell, row: Cell[]) => Node | string;
  /** Sort key when it differs from the raw value. */
  sortValue?: (value: Cell, row: Cell[]) => number | string;
  width?: string;
}

function defaultCell(v: Cell): Node | string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string" && /^https?:\/\//i.test(v)) return link(v);
  if (Array.isArray(v)) {
    const wrap = h("div", { class: "cell-list" });
    for (const item of v) wrap.append(h("div", {}, defaultCell(item)));
    return wrap;
  }
  if (typeof v === "number") return fmt.int(v) === "–" ? "" : Number.isInteger(v) ? fmt.int(v) : fmt.dec(v);
  if (typeof v === "boolean") return v ? "yes" : "no";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function searchText(row: Cell[]): string {
  return row.map((c) => (c === null ? "" : typeof c === "object" ? JSON.stringify(c) : String(c))).join(" ").toLowerCase();
}

export function dataTable(
  columns: Column[],
  rows: Cell[][],
  opts: { pageSize?: number; search?: boolean; note?: string; initialSort?: [number, "asc" | "desc"] } = {},
): HTMLElement {
  const pageSize = opts.pageSize ?? 25;
  let filtered = rows;
  let page = 0;
  let sort: [number, "asc" | "desc"] | null = opts.initialSort ?? null;
  const index = rows.map(searchText);

  const tbody = h("tbody");
  const pager = h("div", { class: "pager" });
  const thead = h("thead");
  const headRow = h("tr");
  thead.append(headRow);

  columns.forEach((c, i) => {
    const th = h("th", { class: c.numeric ? "num" : "", scope: "col", style: c.width ? `width:${c.width}` : undefined });
    const btn = h("button", { class: "sort", type: "button" }, c.label);
    btn.addEventListener("click", () => {
      sort = sort && sort[0] === i ? [i, sort[1] === "asc" ? "desc" : "asc"] : [i, c.numeric ? "desc" : "asc"];
      page = 0;
      render();
    });
    th.append(btn);
    headRow.append(th);
  });

  function sorted(): Cell[][] {
    if (!sort) return filtered;
    const [i, dir] = sort;
    const col = columns[i];
    const key = (r: Cell[]) => {
      const v = col.sortValue ? col.sortValue(r[i], r) : r[i];
      return v === null || v === undefined ? (col.numeric ? -Infinity : "") : (v as number | string);
    };
    const out = [...filtered].sort((a, b) => {
      const ka = key(a), kb = key(b);
      const cmp = typeof ka === "number" && typeof kb === "number" ? ka - kb : String(ka).localeCompare(String(kb));
      return dir === "asc" ? cmp : -cmp;
    });
    return out;
  }

  function render() {
    const all = sorted();
    const pages = Math.max(1, Math.ceil(all.length / pageSize));
    page = Math.min(page, pages - 1);
    tbody.replaceChildren();
    for (const r of all.slice(page * pageSize, (page + 1) * pageSize)) {
      const tr = h("tr");
      columns.forEach((c, i) => {
        tr.append(h("td", { class: c.numeric ? "num" : "" }, c.render ? c.render(r[i], r) : defaultCell(r[i])));
      });
      tbody.append(tr);
    }
    headRow.querySelectorAll("th").forEach((th, i) => {
      th.setAttribute("aria-sort", sort && sort[0] === i ? (sort[1] === "asc" ? "ascending" : "descending") : "none");
    });
    pager.replaceChildren(
      h("span", { class: "muted" }, `${fmt.int(all.length)} rows`),
      h("button", { type: "button", disabled: page === 0, onclick: () => { page--; render(); } }, "Previous"),
      h("span", {}, `${page + 1} / ${pages}`),
      h("button", { type: "button", disabled: page >= pages - 1, onclick: () => { page++; render(); } }, "Next"),
    );
  }

  const wrap = h("div", { class: "datatable" });
  if (opts.search !== false && rows.length > 10) {
    const input = h("input", { type: "search", placeholder: "Filter rows…", "aria-label": "Filter rows" });
    input.addEventListener("input", () => {
      const q = input.value.trim().toLowerCase();
      filtered = q ? rows.filter((_, i) => index[i].includes(q)) : rows;
      page = 0;
      render();
    });
    wrap.append(h("div", { class: "table-tools" }, input, opts.note ? h("span", { class: "muted" }, opts.note) : null));
  }
  wrap.append(h("div", { class: "table-scroll" }, h("table", {}, thead, tbody)), pager);
  render();
  return wrap;
}

export function section(title: string, subtitle?: string, ...children: Child[]): HTMLElement {
  return h("section", { class: "card" }, h("h2", {}, title), subtitle ? h("p", { class: "sub" }, subtitle) : null, ...children);
}
