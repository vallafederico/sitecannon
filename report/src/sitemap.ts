// Visual sitemap: a card-and-connector tree (the Octopus/Slickplan look), with collapsible
// branches, pan & zoom and SVG export. Colours are resolved to literal values at render time
// so an exported SVG looks the same outside the report.

import { hierarchy, tree, type HierarchyPointNode } from "d3-hierarchy";
import { select } from "d3-selection";
import { zoom, zoomIdentity, type ZoomBehavior } from "d3-zoom";

import { fmt, h } from "./ui";

export interface MapNode {
  label: string;
  sub?: string;
  href?: string;
  count?: number;
  countLabel?: string;
  children: number[];
  /** Colour key for the card's top strip (e.g. template), resolved by `colorOf`. */
  colorKey?: string;
  dashed?: boolean;
}

interface Item {
  id: number;
  more?: number; // "+N more" placeholder for the hidden children of `parent`
  parent?: number;
  children?: Item[];
}

const CARD_W = 188;
const CARD_H = 58;
const GAP_X = 18;
const GAP_Y = 46;
const MAX_KIDS = 12;
const SVG_NS = "http://www.w3.org/2000/svg";

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, text?: string) {
  const e = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

function clip(s: string, n: number) {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

export function visualSitemap(
  nodes: MapNode[],
  root: number,
  colorOf: (key: string | undefined) => string,
  opts: { openDepth?: number; legend?: HTMLElement } = {},
): HTMLElement {
  const open = new Set<number>();
  const showAll = new Set<number>();
  // Open the first levels by default.
  (function seed(id: number, depth: number) {
    if (depth >= (opts.openDepth ?? 1)) return;
    open.add(id);
    nodes[id].children.slice(0, MAX_KIDS).forEach((c) => seed(c, depth + 1));
  })(root, 0);
  for (const n of nodes) n.children.sort((a, b) => (nodes[b].count ?? 0) - (nodes[a].count ?? 0));

  const svg = el("svg", { class: "sitemap-svg", role: "img", "aria-label": "Visual sitemap" });
  const g = el("g");
  svg.append(g);
  const wrap = h("div", { class: "sitemap-wrap" });
  wrap.append(svg);

  function build(id: number): Item {
    const kids = nodes[id].children;
    if (!open.has(id) || kids.length === 0) return { id };
    const visible = showAll.has(id) ? kids : kids.slice(0, MAX_KIDS - 1);
    const children: Item[] = visible.map(build);
    if (visible.length < kids.length) children.push({ id: -1, more: kids.length - visible.length, parent: id });
    return { id, children };
  }

  let zoomer: ZoomBehavior<SVGSVGElement, unknown>;
  let lastBounds = { x0: 0, x1: 0, y0: 0, y1: 0 };

  function render() {
    const colors = {
      surface: css("--surface-1"),
      border: css("--axis"),
      text: css("--text-primary"),
      muted: css("--text-muted"),
      link: css("--grid") || "#ccc",
      accent: css("--accent"),
    };
    const rootItem = hierarchy<Item>(build(root), (d) => d.children);
    const layout = tree<Item>().nodeSize([CARD_W + GAP_X, CARD_H + GAP_Y]).separation((a, b) => (a.parent === b.parent ? 1 : 1.15));
    const laid = layout(rootItem);
    g.replaceChildren();

    let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
    laid.each((d) => {
      x0 = Math.min(x0, d.x - CARD_W / 2);
      x1 = Math.max(x1, d.x + CARD_W / 2);
      y0 = Math.min(y0, d.y);
      y1 = Math.max(y1, d.y + CARD_H);
    });
    lastBounds = { x0, x1, y0, y1 };

    // Elbow connectors: down from the parent, across, down into the child.
    const links = el("g", { fill: "none", stroke: colors.border, "stroke-width": 1.25 });
    for (const l of laid.links()) {
      const sx = l.source.x, sy = l.source.y + CARD_H, tx = l.target.x, ty = l.target.y;
      const my = sy + GAP_Y / 2;
      links.append(el("path", { d: `M${sx},${sy}V${my}H${tx}V${ty}` }));
    }
    g.append(links);

    laid.each((d: HierarchyPointNode<Item>) => {
      const card = el("g", { transform: `translate(${d.x - CARD_W / 2},${d.y})`, class: "sm-card", tabindex: 0 });
      if (d.data.more) {
        card.append(el("rect", { width: CARD_W, height: CARD_H, rx: 8, fill: colors.surface, stroke: colors.border, "stroke-dasharray": "4 3" }));
        card.append(el("text", { x: CARD_W / 2, y: CARD_H / 2 + 5, "text-anchor": "middle", fill: colors.muted, "font-size": 13 }, `+ ${fmt.int(d.data.more)} more`));
        card.addEventListener("click", () => { showAll.add(d.data.parent!); render(); });
        g.append(card);
        return;
      }
      const n = nodes[d.data.id];
      const hasKids = n.children.length > 0;
      card.append(el("rect", {
        width: CARD_W, height: CARD_H, rx: 8, fill: colors.surface, stroke: colors.border,
        ...(n.dashed ? { "stroke-dasharray": "4 3" } : {}),
      }));
      // Colour strip carries identity; text stays in ink colours.
      card.append(el("path", { d: `M0,8A8,8 0 0 1 8,0H${CARD_W - 8}A8,8 0 0 1 ${CARD_W},8V9H0Z`, fill: colorOf(n.colorKey) }));
      card.append(el("text", { x: 10, y: 24, fill: colors.text, "font-size": 13, "font-weight": 600 }, clip(n.label, 24)));
      if (n.sub) card.append(el("text", { x: 10, y: 40, fill: colors.muted, "font-size": 11 }, clip(n.sub, 30)));
      const footer = [n.countLabel, hasKids ? (open.has(d.data.id) ? "▾" : `▸ ${n.children.length}`) : ""].filter(Boolean).join("  ");
      if (footer) card.append(el("text", { x: CARD_W - 10, y: CARD_H - 8, "text-anchor": "end", fill: colors.muted, "font-size": 10 }, footer));
      if (n.href) {
        const a = el("a", { href: n.href, target: "_blank", rel: "noopener noreferrer" });
        a.append(el("text", { x: CARD_W - 10, y: 22, "text-anchor": "end", fill: colors.accent, "font-size": 12 }, "↗"));
        a.addEventListener("click", (e) => e.stopPropagation());
        card.append(a);
      }
      const title = el("title", {}, [n.label, n.sub, n.countLabel].filter(Boolean).join("\n"));
      card.append(title);
      const toggle = () => {
        if (!hasKids) return;
        if (open.has(d.data.id)) open.delete(d.data.id);
        else open.add(d.data.id);
        render();
      };
      card.addEventListener("click", toggle);
      card.addEventListener("keydown", (e) => { if ((e as KeyboardEvent).key === "Enter") toggle(); });
      g.append(card);
    });
  }

  function fit() {
    const { x0, x1, y0, y1 } = lastBounds;
    const W = wrap.clientWidth || 1000, H = wrap.clientHeight || 600;
    // Fit when possible, but never below a readable scale; wide trees centre on the root and pan.
    const k = Math.max(0.75, Math.min(1, (W - 40) / (x1 - x0 || 1), (H - 40) / (y1 - y0 || 1)));
    const cx = (x1 - x0) * k <= W - 40 ? (x0 + x1) / 2 : 0;
    const t = zoomIdentity.translate(W / 2 - cx * k, 20 - y0 * k).scale(k);
    select(svg as SVGSVGElement).call(zoomer.transform, t);
  }

  zoomer = zoom<SVGSVGElement, unknown>().scaleExtent([0.1, 2.5]).on("zoom", (e) => g.setAttribute("transform", e.transform.toString()));
  select(svg as SVGSVGElement).call(zoomer).on("dblclick.zoom", null);

  function setAll(expand: boolean) {
    open.clear();
    if (expand) nodes.forEach((n, i) => { if (n.children.length) open.add(i); });
    else open.add(root);
    render();
    fit();
  }

  function download() {
    const { x0, x1, y0, y1 } = lastBounds;
    const clone = svg.cloneNode(true) as SVGSVGElement;
    clone.setAttribute("xmlns", SVG_NS);
    clone.setAttribute("viewBox", `${x0 - 20} ${y0 - 20} ${x1 - x0 + 40} ${y1 - y0 + 40}`);
    clone.setAttribute("width", String(Math.round(x1 - x0 + 40)));
    clone.setAttribute("height", String(Math.round(y1 - y0 + 40)));
    clone.setAttribute("font-family", "system-ui, -apple-system, sans-serif");
    (clone.firstChild as SVGGElement).removeAttribute("transform");
    clone.insertBefore(el("rect", { x: x0 - 20, y: y0 - 20, width: x1 - x0 + 40, height: y1 - y0 + 40, fill: css("--page") }), clone.firstChild);
    const blob = new Blob([new XMLSerializer().serializeToString(clone)], { type: "image/svg+xml" });
    const a = h("a", { href: URL.createObjectURL(blob), download: "sitemap.svg" });
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }

  const toolbar = h("div", { class: "controls" },
    h("button", { class: "btn", type: "button", onclick: () => fit() }, "Fit to screen"),
    h("button", { class: "btn", type: "button", onclick: () => setAll(true) }, "Expand all"),
    h("button", { class: "btn", type: "button", onclick: () => setAll(false) }, "Collapse"),
    h("button", { class: "btn", type: "button", onclick: download }, "Download SVG"),
    h("span", { class: "muted" }, "Click a card to open or close it · ↗ opens the page · scroll to zoom, drag to pan"),
  );

  render();
  // Fit once the view has a size (it's built before being attached), then leave zoom to the user.
  let fitted = false;
  new ResizeObserver(() => {
    if (!fitted && wrap.clientWidth > 0) {
      fitted = true;
      fit();
    }
  }).observe(wrap);
  new MutationObserver(() => render()).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return h("div", { class: "sitemap" }, toolbar, opts.legend ?? null, wrap);
}
