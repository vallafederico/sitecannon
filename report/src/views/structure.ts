import { visualSitemap, type MapNode } from "../sitemap";
import type { NavItem, ReportData } from "../types";
import { dataTable, fmt, h, link, pathOf, section } from "../ui";

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Card colours: the three largest templates, everything else neutral (matches the link graph). */
function templateColors(d: ReportData) {
  const top = d.templates.filter((t) => t.pages > 1).slice(0, 3).map((t) => String(t.id));
  const colorOf = (key?: string) => {
    if (key === "none") return css("--grid");
    const k = key ? top.indexOf(key) : -1;
    return k >= 0 ? css(`--series-${k + 1}`) : css("--other");
  };
  const legend = h("div", { class: "legend" });
  top.forEach((id, k) => {
    const t = d.templates.find((x) => String(x.id) === id)!;
    legend.append(h("span", { class: "key" }, h("span", { class: "swatch", style: `background:var(--series-${k + 1})` }), `${t.pattern.split(" · ")[0]} (${t.pages})`));
  });
  legend.append(h("span", { class: "key" }, h("span", { class: "swatch", style: "background:var(--other)" }), "Other templates & one-offs"));
  legend.append(h("span", { class: "key" }, h("span", { class: "swatch dashed" }), "Folder with no page"));
  return { colorOf, legend };
}

function shortTitle(t: string | null, url: string) {
  if (!t) return pathOf(url);
  // Drop a trailing " | Brand" / " - Brand".
  return t.replace(/\s+[|\-–—]\s+[^|\-–—]{2,40}$/, "") || t;
}

function urlMap(d: ReportData): { nodes: MapNode[]; root: number } {
  const t = d.url_tree;
  const host = (() => { try { return new URL(d.site.seed).host; } catch { return "/"; } })();
  const nodes: MapNode[] = t.segment.map((seg, i) => {
    const page = t.page[i];
    return {
      label: i === 0 ? host : page >= 0 ? shortTitle(d.pages.title[page], d.pages.url[page]) : `/${seg}`,
      sub: t.path[i],
      href: page >= 0 ? d.pages.url[page] : undefined,
      count: t.subtree[i],
      countLabel: `${fmt.int(t.subtree[i])} page${t.subtree[i] === 1 ? "" : "s"}`,
      children: [],
      colorKey: page >= 0 ? String(d.pages.template[page]) : "none",
      dashed: page < 0 && i > 0,
    };
  });
  t.parent.forEach((p, i) => { if (p >= 0) nodes[p].children.push(i); });
  return { nodes, root: 0 };
}

function clickMap(d: ReportData): { nodes: MapNode[]; root: number } {
  const p = d.pages;
  const nodes: MapNode[] = p.url.map((u, i) => ({
    label: shortTitle(p.title[i], u), sub: pathOf(u), href: u, count: 1, children: [], colorKey: String(p.template[i]),
  }));
  p.parent.forEach((par, i) => { if (par >= 0) nodes[par].children.push(i); });
  const order = [...p.url.keys()].filter((i) => p.depth[i] != null).sort((a, b) => (p.depth[b] ?? 0) - (p.depth[a] ?? 0));
  for (const i of order) if (p.parent[i] >= 0) nodes[p.parent[i]].count! += nodes[i].count!;
  nodes.forEach((n) => { n.countLabel = n.count! > 1 ? `${fmt.int(n.count! - 1)} below` : undefined; });
  return { nodes, root: Math.max(0, p.depth.findIndex((x) => x === 0)) };
}

function navMap(d: ReportData): { nodes: MapNode[]; root: number } {
  const home = Math.max(0, d.pages.depth.findIndex((x) => x === 0));
  const nodes: MapNode[] = [{ label: "Homepage", sub: "/", href: d.pages.url[home], children: [], colorKey: String(d.pages.template[home]) }];
  const add = (n: MapNode) => { nodes.push(n); return nodes.length - 1; };
  for (const region of ["header", "footer"] as const) {
    const items = d.nav.filter((x) => x.region === region && x.coverage >= 0.5 && x.anchor);
    if (!items.length) continue;
    const regionId = add({ label: region === "header" ? "Header & menus" : "Footer", children: [], colorKey: "none" });
    nodes[0].children.push(regionId);
    const groups = groupNav(items);
    for (const [group, list] of groups) {
      let parent = regionId;
      if (group !== "Top level") {
        parent = add({ label: group, sub: `${list.length} links`, children: [], colorKey: "none" });
        nodes[regionId].children.push(parent);
      }
      for (const it of list) {
        nodes[parent].children.push(add({
          label: it.anchor, sub: pathOf(it.url), href: it.url, children: [],
          colorKey: it.page >= 0 ? String(d.pages.template[it.page]) : "none",
          countLabel: fmt.pct(it.coverage),
        }));
      }
    }
  }
  // Keep menu order rather than sorting by size.
  nodes.forEach((n, i) => { n.count = nodes.length - i; });
  return { nodes, root: 0 };
}

function visualView(d: ReportData): HTMLElement {
  const { colorOf, legend } = templateColors(d);
  const holder = h("div", {});
  const sources: Record<string, () => { nodes: MapNode[]; root: number }> = { url: () => urlMap(d), nav: () => navMap(d), click: () => clickMap(d) };
  const sel = h("select", { "aria-label": "Sitemap source" },
    h("option", { value: "url" }, "by URL structure"),
    h("option", { value: "nav" }, "by navigation"),
    h("option", { value: "click" }, "by click depth"));
  const show = () => {
    const { nodes, root } = sources[sel.value]();
    const openDepth = sel.value === "nav" ? 3 : nodes.length <= 80 ? 2 : 1;
    holder.replaceChildren(visualSitemap(nodes, root, colorOf, { openDepth, legend: legend.cloneNode(true) as HTMLElement }));
  };
  sel.addEventListener("change", show);
  show();
  return h("div", {}, h("div", { class: "controls" }, h("label", {}, "Sitemap", sel)), holder);
}

interface TreeNode {
  label: string;
  title?: string;
  href?: string;
  count: number;
  children: number[];
  tags?: string[];
}

const CHILD_PAGE = 150;

/** Collapsible indented tree. Rows are built lazily on expand, so 10k-node trees stay fast. */
function treeView(nodes: TreeNode[], root: number, unit: string, expandDepth = 1): HTMLElement {
  // Scale bars to the largest node below the root, so sections stay comparable.
  const max = Math.max(1, ...nodes.filter((_, i) => i !== root).map((n) => n.count));
  for (const n of nodes) n.children.sort((a, b) => nodes[b].count - nodes[a].count || nodes[a].label.localeCompare(nodes[b].label));
  const container = h("div", { class: "tree", role: "tree" });
  container.append(
    h("div", { class: "tree-row muted" }, h("span", {}, ""), h("span", { class: "tree-count" }, unit), h("span", {}, "")),
  );

  function row(id: number, depth: number): HTMLElement[] {
    const n = nodes[id];
    const wrap = h("div", { role: "treeitem" });
    const hasKids = n.children.length > 0;
    const toggle = h("button", { class: "tree-toggle", type: "button", "aria-label": hasKids ? "Expand" : undefined }, hasKids ? "▸" : "");
    const kids = h("div", { role: "group" });
    let open = false;
    let shown = 0;

    function showMore() {
      const next = n.children.slice(shown, shown + CHILD_PAGE);
      shown += next.length;
      kids.querySelector(".more")?.remove();
      for (const c of next) kids.append(...row(c, depth + 1));
      if (shown < n.children.length) {
        const more = h("button", { class: "btn more", type: "button", style: `margin:4px 0 4px ${(depth + 1) * 18 + 24}px` },
          `Show ${fmt.int(Math.min(CHILD_PAGE, n.children.length - shown))} more of ${fmt.int(n.children.length - shown)}`);
        more.addEventListener("click", showMore);
        kids.append(more);
      }
    }
    function setOpen(v: boolean) {
      open = v;
      toggle.textContent = hasKids ? (open ? "▾" : "▸") : "";
      wrap.setAttribute("aria-expanded", String(open));
      if (open && shown === 0) showMore();
      kids.style.display = open ? "" : "none";
    }
    toggle.addEventListener("click", () => hasKids && setOpen(!open));

    const label = h("span", { class: "tree-label", style: `padding-left:${depth * 18}px` }, toggle);
    label.append(n.href ? h("span", { class: "name" }, link(n.href, n.label)) : h("span", { class: "name" }, n.label));
    if (n.title) label.append(h("span", { class: "name muted", title: n.title }, n.title));
    for (const t of n.tags ?? []) label.append(h("span", { class: "tag" }, t));

    const bar = h("div", { class: "tree-bar" }, h("span", { style: `width:${Math.min(100, (n.count / max) * 100)}%` }));
    const r = h("div", { class: "tree-row" }, label, h("span", { class: "tree-count" }, fmt.int(n.count)), bar);
    wrap.append(r, kids);
    setOpen(hasKids && depth < expandDepth);
    return [wrap];
  }
  container.append(...row(root, 0));
  return container;
}

function urlTree(d: ReportData): HTMLElement {
  const t = d.url_tree;
  const host = (() => { try { return new URL(d.site.seed).host; } catch { return "/"; } })();
  const nodes: TreeNode[] = t.segment.map((seg, i) => {
    const page = t.page[i];
    return {
      label: i === 0 ? host : `/${seg}`,
      href: page >= 0 ? d.pages.url[page] : undefined,
      count: t.subtree[i],
      children: [],
      tags: page < 0 && i > 0 ? ["no page"] : undefined,
    };
  });
  t.parent.forEach((p, i) => { if (p >= 0) nodes[p].children.push(i); });
  return treeView(nodes, 0, "pages");
}

function clickTree(d: ReportData): HTMLElement {
  const p = d.pages;
  const n = p.url.length;
  const nodes: TreeNode[] = p.url.map((u, i) => ({ label: pathOf(u), href: u, title: p.title[i] ?? undefined, count: 1, children: [] }));
  let root = p.depth.findIndex((x) => x === 0);
  p.parent.forEach((par, i) => { if (par >= 0) nodes[par].children.push(i); });
  // Subtree sizes, deepest pages first.
  const order = [...Array(n).keys()].filter((i) => p.depth[i] != null).sort((a, b) => (p.depth[b] ?? 0) - (p.depth[a] ?? 0));
  for (const i of order) if (p.parent[i] >= 0) nodes[p.parent[i]].count += nodes[i].count;
  if (root < 0) root = 0;
  return treeView(nodes, root, "pages below");
}

function groupNav(items: NavItem[]): Map<string, NavItem[]> {
  const groups = new Map<string, NavItem[]>();
  for (const it of items) {
    const key = it.group ?? "Top level";
    if (!groups.has(key)) groups.set(key, []);
    const list = groups.get(key)!;
    if (!list.some((x) => x.url === it.url && x.anchor === it.anchor)) list.push(it);
  }
  return groups;
}

function navView(d: ReportData): HTMLElement {
  const wrap = h("div", {});
  const controls = h("label", { class: "controls" });
  const all = h("input", { type: "checkbox" });
  controls.append(all, "Include links that appear on fewer than half the pages");
  const body = h("div", { class: "grid-2" });

  function render() {
    body.replaceChildren();
    const min = all.checked ? 0 : 0.5;
    for (const region of ["header", "footer"] as const) {
      const items = d.nav.filter((x) => x.region === region && x.coverage >= min && x.anchor);
      const col = h("div", { class: "nav-group" }, h("h2", {}, region === "header" ? "Header & menus" : "Footer"));
      if (!items.length) col.append(h("p", { class: "muted" }, "No links meet this threshold."));
      for (const [group, list] of groupNav(items)) {
        const ul = h("ul");
        for (const it of list) {
          ul.append(h("li", {}, h("span", {}, it.anchor, " ", h("span", { class: "muted" }, "→ "), link(it.url)), h("span", { class: "muted" }, fmt.pct(it.coverage))));
        }
        col.append(h("h3", {}, group), ul);
      }
      body.append(col);
    }
  }
  all.addEventListener("change", render);
  render();
  wrap.append(h("p", { class: "sub" }, "Menu and footer links, grouped by dropdown or column. Percentages show the share of pages that carry the link."), controls, body);
  return wrap;
}

/** Top-level URL sections and whether the persistent navigation reaches them. */
function sectionCoverage(d: ReportData): HTMLElement {
  const t = d.url_tree;
  const global = d.nav.filter((x) => x.coverage >= 0.5);
  const inRegion = (path: string, region: string) =>
    global.some((x) => x.region === region && (pathOf(x.url) + "/").startsWith(path));
  const rows = t.segment
    .map((_, i) => i)
    .filter((i) => t.parent[i] === 0 && t.subtree[i] >= 2)
    .sort((a, b) => t.subtree[b] - t.subtree[a])
    .map((i) => [t.path[i], t.subtree[i], t.page[i] >= 0, inRegion(t.path[i], "header"), inRegion(t.path[i], "footer")]);
  const yesNo = (v: unknown) => (v ? "yes" : h("strong", {}, "no"));
  return dataTable(
    [
      { label: "Section" },
      { label: "Pages", numeric: true },
      { label: "Has landing page", render: yesNo },
      { label: "In header/menu", render: yesNo },
      { label: "In footer", render: yesNo },
    ],
    rows,
    { pageSize: 20, initialSort: [1, "desc"] },
  );
}

export function structure(d: ReportData): HTMLElement[] {
  const views: Record<string, () => HTMLElement> = {
    visual: () => visualView(d),
    url: () => urlTree(d),
    nav: () => navView(d),
    click: () => clickTree(d),
  };
  const labels: Record<string, [string, string]> = {
    visual: ["Visual sitemap", "Card view of the site, built from its URLs, its menus or its click paths. Strip colour = template. Export it as SVG for decks and workshops."],
    url: ["URL structure", "The site as its URLs describe it. Folders tagged “no page” have content beneath but no landing page."],
    nav: ["Navigation", ""],
    click: ["Click depth", "The shortest click path from the homepage to every page. Each page sits under the most-linked page one click closer to home."],
  };
  const holder = h("div", {});
  const sub = h("p", { class: "sub" });
  const seg = h("div", { class: "seg", role: "group", "aria-label": "Structure view" });
  const cache: Record<string, HTMLElement> = {};
  function show(key: string) {
    seg.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.key === key)));
    sub.textContent = labels[key][1];
    cache[key] ??= views[key]();
    holder.replaceChildren(cache[key]);
  }
  for (const key of Object.keys(views)) {
    const b = h("button", { type: "button" }, labels[key][0]);
    b.dataset.key = key;
    b.addEventListener("click", () => show(key));
    seg.append(b);
  }
  show("visual");
  return [
    h("section", { class: "card" }, h("h2", {}, "Three views of one site"), h("p", { class: "sub" }, "How the URLs, the menus and the actual link paths each organise the content. Where they disagree is where the information architecture needs attention."), seg, sub, holder),
    section("Sections vs navigation", "Top-level URL sections, whether they have a landing page, and whether the persistent header or footer links into them.", sectionCoverage(d)),
  ];
}
