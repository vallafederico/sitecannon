import Graph from "graphology";
import { inferSettings } from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import Sigma from "sigma";

import type { ReportData } from "../types";
import { fmt, h, hideTip, link, pathOf, showTip } from "../ui";

type ColorMode = "template" | "section" | "depth";
type EdgeSet = "editorial" | "content" | "all";
const EDGE_MIN_KIND: Record<EdgeSet, number> = { editorial: 2, content: 1, all: 0 };

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
// Ordinal blue ramp for click depth: more clicks = darker.
const DEPTH_RAMP = ["#86b6ef", "#5598e7", "#2a78d6", "#1c5cab", "#104281", "#0d366b"];

function section(url: string): string {
  const seg = pathOf(url).split(/[/?]/).filter(Boolean)[0];
  return seg ? `/${seg}` : "/";
}

export function graphView(d: ReportData): HTMLElement[] {
  const p = d.pages;
  const n = p.url.length;
  const maxPr = Math.max(...p.pagerank, 1);

  // Categorical identity is capped at three hues (all-pairs safe); the rest fold into "Other".
  const templateTop = d.templates.filter((t) => t.pages > 1).slice(0, 3).map((t) => t.id);
  const sectionCounts = new Map<string, number>();
  p.url.forEach((u) => sectionCounts.set(section(u), (sectionCounts.get(section(u)) ?? 0) + 1));
  const sectionsBySize = [...sectionCounts.entries()].sort((a, b) => b[1] - a[1]);
  const sectionTop = sectionsBySize.slice(0, 3).map(([s]) => s);
  const templateName = new Map(d.templates.map((t) => [t.id, t.pattern.split(" · ")[0]]));

  const graph = new Graph({ type: "directed", multi: false });
  const edgeCount = (min: number) => d.edges.filter((k, i) => i % 3 === 2 && k >= min).length;

  // Seed positions by template on a circle so the layout starts from a sensible shape.
  for (let i = 0; i < n; i++) {
    const t = p.template[i] ?? 0;
    const angle = ((t * 137.5) % 360) * (Math.PI / 180);
    const r = 100 + Math.random() * 40;
    graph.addNode(String(i), {
      x: Math.cos(angle) * r + Math.random() * 20,
      y: Math.sin(angle) * r + Math.random() * 20,
      size: 2 + 12 * Math.sqrt(p.pagerank[i] / maxPr),
      label: p.title[i] || pathOf(p.url[i]),
    });
  }

  function setEdges(set: EdgeSet) {
    graph.clearEdges();
    for (let i = 0; i < d.edges.length; i += 3) {
      if (d.edges[i + 2] < EDGE_MIN_KIND[set]) continue;
      const a = String(d.edges[i]), b = String(d.edges[i + 1]);
      if (!graph.hasEdge(a, b)) graph.addEdge(a, b);
    }
  }

  // --- Controls ---
  const colorSel = h("select", { "aria-label": "Colour nodes by" },
    h("option", { value: "template" }, "Template"),
    h("option", { value: "section" }, "URL section"),
    h("option", { value: "depth" }, "Click depth"));
  const highlightSel = h("select", { "aria-label": "Highlight" });
  const edgeSel = h("select", { "aria-label": "Links shown" },
    h("option", { value: "editorial" }, `Editorial links (${fmt.compact(edgeCount(2))})`),
    h("option", { value: "content" }, `+ sidebars & widgets (${fmt.compact(edgeCount(1))})`),
    h("option", { value: "all" }, `+ menus & footer (${fmt.compact(edgeCount(0))})`));
  const search = h("input", { type: "search", placeholder: "Find a page…", "aria-label": "Find a page", list: "graph-pages" });
  const datalist = h("datalist", { id: "graph-pages" });
  for (let i = 0; i < Math.min(n, 5000); i++) datalist.append(h("option", { value: pathOf(p.url[i]) }));
  const relayout = h("button", { class: "btn", type: "button" }, "Re-run layout");
  const legend = h("div", { class: "legend" });
  const controls = h("div", { class: "controls" },
    h("label", {}, "Colour by", colorSel),
    h("label", {}, "Highlight", highlightSel),
    h("label", {}, "Links", edgeSel),
    search, datalist, relayout);

  const container = h("div", { class: "sigma-container" });
  const status = h("div", { class: "graph-status" });
  const panel = h("div", { class: "side-panel", style: "display:none" });
  const wrap = h("div", { class: "graph-wrap" }, container, status, panel);

  let mode: ColorMode = "template";
  let highlight = "";
  let hovered: string | null = null;
  let selected: string | null = null;
  let neighbors = new Set<string>();

  function fillHighlightOptions() {
    highlightSel.replaceChildren(h("option", { value: "" }, "None"));
    if (mode === "template") {
      for (const t of d.templates.filter((t) => t.pages > 1).slice(0, 40)) {
        highlightSel.append(h("option", { value: `t${t.id}` }, `${templateName.get(t.id)} (${t.pages})`));
      }
    } else if (mode === "section") {
      for (const [s, c] of sectionsBySize.slice(0, 40)) highlightSel.append(h("option", { value: `s${s}` }, `${s} (${c})`));
    } else {
      const maxD = Math.max(0, ...p.depth.map((x) => x ?? 0));
      for (let i = 0; i <= maxD; i++) highlightSel.append(h("option", { value: `d${i}` }, `${i} clicks`));
    }
    highlightSel.value = "";
    highlight = "";
  }

  function keyOf(i: number): string {
    if (mode === "template") return `t${p.template[i]}`;
    if (mode === "section") return `s${section(p.url[i])}`;
    return `d${p.depth[i]}`;
  }

  function baseColor(i: number): string {
    const series = [css("--series-1"), css("--series-2"), css("--series-3")];
    const other = css("--other");
    if (highlight) return keyOf(i) === highlight ? series[0] : other;
    if (mode === "template") {
      const k = templateTop.indexOf(p.template[i] ?? -1);
      return k >= 0 ? series[k] : other;
    }
    if (mode === "section") {
      const k = sectionTop.indexOf(section(p.url[i]));
      return k >= 0 ? series[k] : other;
    }
    const dd = p.depth[i];
    return dd == null ? other : DEPTH_RAMP[Math.min(dd, DEPTH_RAMP.length - 1)];
  }

  function paint() {
    graph.forEachNode((key) => graph.setNodeAttribute(key, "color", baseColor(Number(key))));
    legend.replaceChildren();
    const key = (color: string, label: string) =>
      h("span", { class: "key" }, h("span", { class: "dot", style: `background:${color}` }), label);
    const series = [css("--series-1"), css("--series-2"), css("--series-3")];
    if (highlight) {
      legend.append(key(series[0], highlightSel.selectedOptions[0]?.textContent ?? ""), key(css("--other"), "Everything else"));
    } else if (mode === "template") {
      templateTop.forEach((id, k) => legend.append(key(series[k], `${templateName.get(id)} (${d.templates.find((t) => t.id === id)?.pages})`)));
      legend.append(key(css("--other"), "Other templates & one-off pages"));
    } else if (mode === "section") {
      sectionTop.forEach((s, k) => legend.append(key(series[k], `${s} (${sectionCounts.get(s)})`)));
      legend.append(key(css("--other"), "Other sections"));
    } else {
      DEPTH_RAMP.forEach((c, k) => legend.append(key(c, k === DEPTH_RAMP.length - 1 ? `${k}+ clicks` : `${k} clicks`)));
    }
    legend.append(h("span", { class: "muted" }, "Node size: internal PageRank"));
    renderer.refresh();
  }

  setEdges("editorial");
  const renderer = new Sigma(graph, container, {
    renderEdgeLabels: false,
    hideEdgesOnMove: n > 2000,
    labelRenderedSizeThreshold: 9,
    labelColor: { color: css("--text-primary") },
    labelFont: "system-ui, -apple-system, sans-serif",
    defaultEdgeColor: css("--edge"),
    zIndex: true,
    // The view is built before it's attached to the page; a ResizeObserver sizes it on mount.
    allowInvalidContainer: true,
    nodeReducer: (key, attrs) => {
      const focus = hovered ?? selected;
      if (!focus) return attrs;
      if (key === focus) return { ...attrs, highlighted: true, zIndex: 2 };
      if (neighbors.has(key)) return { ...attrs, zIndex: 1 };
      return { ...attrs, color: css("--other"), label: "", zIndex: 0 };
    },
    edgeReducer: (key, attrs) => {
      const focus = hovered ?? selected;
      if (!focus) return attrs;
      const [s, t] = graph.extremities(key);
      return s === focus || t === focus ? { ...attrs, color: css("--series-1"), size: 1 } : { ...attrs, hidden: true };
    },
  });

  function focusOn(node: string | null) {
    neighbors = node ? new Set(graph.neighbors(node)) : new Set();
  }

  renderer.on("enterNode", ({ node, event }) => {
    hovered = node;
    focusOn(node);
    const i = Number(node);
    const ev = event.original as MouseEvent;
    showTip(ev.clientX ?? 0, ev.clientY ?? 0, [
      ["PageRank", fmt.dec(p.pagerank[i])],
      ["links in", fmt.int(p.inlinks[i])],
      ["links out", fmt.int(p.outlinks[i])],
      ["click depth", p.depth[i] == null ? "unreachable" : String(p.depth[i])],
    ], p.title[i] || pathOf(p.url[i]));
    renderer.refresh({ skipIndexation: true });
  });
  renderer.on("leaveNode", () => {
    hovered = null;
    focusOn(selected);
    hideTip();
    renderer.refresh({ skipIndexation: true });
  });
  renderer.on("clickNode", ({ node }) => select(node));
  renderer.on("clickStage", () => select(null));

  function select(node: string | null) {
    selected = node;
    focusOn(node);
    if (!node) {
      panel.style.display = "none";
    } else {
      const i = Number(node);
      const tpl = p.template[i];
      panel.replaceChildren(
        h("h3", {}, p.title[i] || pathOf(p.url[i])),
        link(p.url[i], pathOf(p.url[i])),
        h("dl", {},
          h("dt", {}, "Template"), h("dd", {}, tpl == null ? "–" : `#${tpl} ${templateName.get(tpl) ?? ""}`),
          h("dt", {}, "Click depth"), h("dd", {}, p.depth[i] == null ? "unreachable" : String(p.depth[i])),
          h("dt", {}, "PageRank"), h("dd", {}, fmt.dec(p.pagerank[i])),
          h("dt", {}, "Links in / out"), h("dd", {}, `${fmt.int(p.inlinks[i])} / ${fmt.int(p.outlinks[i])}`),
          h("dt", {}, "Main content"), h("dd", {}, `${fmt.int(p.main_words[i])} words`),
          h("dt", {}, "Indexability"), h("dd", {}, p.indexability[i]),
          h("dt", {}, "In sitemap"), h("dd", {}, p.in_sitemap[i] ? "yes" : "no")),
      );
      panel.style.display = "";
    }
    renderer.refresh({ skipIndexation: true });
  }

  // --- Layout ---
  let layout: FA2Layout | null = null;
  let timer = 0;
  function runLayout() {
    layout?.kill();
    clearTimeout(timer);
    const settings = inferSettings(graph);
    layout = new FA2Layout(graph, { settings: { ...settings, linLogMode: true, barnesHutOptimize: n > 1000, gravity: 0.5, scalingRatio: 2, slowDown: 5 } });
    layout.start();
    const ms = Math.min(20_000, 4_000 + n * 2);
    status.textContent = `Laying out ${fmt.int(n)} pages and ${fmt.int(graph.size)} links…`;
    timer = window.setTimeout(() => {
      layout?.stop();
      status.textContent = `${fmt.int(n)} pages · ${fmt.int(graph.size)} links shown. Drag to pan, scroll to zoom, click a page for details.`;
    }, ms);
  }

  colorSel.addEventListener("change", () => { mode = colorSel.value as ColorMode; fillHighlightOptions(); paint(); });
  highlightSel.addEventListener("change", () => { highlight = highlightSel.value; paint(); });
  edgeSel.addEventListener("change", () => { setEdges(edgeSel.value as EdgeSet); renderer.refresh(); runLayout(); });
  relayout.addEventListener("click", runLayout);
  search.addEventListener("change", () => {
    const q = search.value.trim();
    const i = p.url.findIndex((u) => pathOf(u) === q || u === q);
    if (i < 0) return;
    select(String(i));
    const pos = renderer.getNodeDisplayData(String(i));
    if (pos) renderer.getCamera().animate({ x: pos.x, y: pos.y, ratio: 0.15 }, { duration: 500 });
  });
  new MutationObserver(() => {
    renderer.setSetting("labelColor", { color: css("--text-primary") });
    renderer.setSetting("defaultEdgeColor", css("--edge"));
    paint();
  }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

  new ResizeObserver(() => renderer.resize()).observe(container);

  fillHighlightOptions();
  paint();
  runLayout();

  return [
    h("section", { class: "card" },
      h("h2", {}, "Internal link graph"),
      h("p", { class: "sub" }, "Every page is a dot; every internal link a line. Pages that cluster together link to each other. By default only editorial links are drawn (in the page content, pointing at pages that aren't linked from everywhere): menus, footers and “latest stories” widgets connect everything to everything and hide the structure."),
      controls, legend),
    wrap,
  ];
}
