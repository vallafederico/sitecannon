import type { Cell, ReportData, Severity } from "../types";
import { dataTable, fmt, h, link, pathOf, section, severityBadge, type Column } from "../ui";

const SEVERITY_ORDER: Severity[] = ["critical", "serious", "warning", "info"];

const label = (col: string) => col.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());

export function issues(d: ReportData, focus?: string): HTMLElement[] {
  const list = [...d.issues].sort(
    (a, b) => SEVERITY_ORDER.indexOf(a.severity) - SEVERITY_ORDER.indexOf(b.severity) || b.total - a.total,
  );
  const out: HTMLElement[] = [];
  const clean = list.filter((i) => i.total === 0);
  for (const i of list.filter((i) => i.total > 0)) {
    const numeric = i.columns.map((_, c) => i.rows.every((r) => r[c] === null || typeof r[c] === "number"));
    const columns: Column[] = i.columns.map((c, k) => ({ label: label(c), numeric: numeric[k] }));
    const note = i.total > i.rows.length ? `Showing the first ${fmt.int(i.rows.length)} of ${fmt.int(i.total)}` : undefined;
    const details = h("details", { class: "card issue", id: `issue-${i.id}`, open: i.id === focus },
      h("summary", {}, severityBadge(i.severity), h("h3", {}, i.title), h("strong", {}, fmt.int(i.total))),
      h("p", { class: "desc" }, i.description),
    );
    // Tables render on first open: some issues have thousands of rows.
    let built = false;
    const build = () => { if (!built && details.open) { built = true; details.append(dataTable(columns, i.rows, { note, pageSize: 20 })); } };
    details.addEventListener("toggle", build);
    build();
    out.push(details);
  }
  if (clean.length) {
    out.push(section("Checks with no findings", undefined, h("p", { class: "muted" }, clean.map((i) => i.title).join(" · "))));
  }
  if (focus) requestAnimationFrame(() => document.getElementById(`issue-${focus}`)?.scrollIntoView({ block: "start" }));
  return out;
}

export function templates(d: ReportData): HTMLElement[] {
  const p = d.pages;
  const stats = new Map<number, { words: number[]; depth: number[] }>();
  p.template.forEach((t, i) => {
    if (t == null) return;
    const s = stats.get(t) ?? { words: [], depth: [] };
    if (p.main_words[i] != null) s.words.push(p.main_words[i]!);
    if (p.depth[i] != null) s.depth.push(p.depth[i]!);
    stats.set(t, s);
  });
  const median = (xs: number[]) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)] : null);
  const total = p.url.length;

  const multi = d.templates.filter((t) => t.pages > 1);
  const rows: Cell[][] = multi.map((t) => [
    t.id, t.pattern, t.pages, t.pages / total, median(stats.get(t.id)?.words ?? []), median(stats.get(t.id)?.depth ?? []), p.url[t.example] ?? null,
  ]);
  const oneOffs: Cell[][] = d.templates.filter((t) => t.pages === 1).map((t) => [p.url[t.example] ?? null, p.title[t.example] ?? null, p.depth[t.example], p.pagerank[t.example]]);

  return [
    section("Templates", "Pages grouped by the layout of their main content (site header and footer excluded). Each template is something the rebuild has to design and build once.",
      dataTable(
        [
          { label: "#", numeric: true, width: "48px" },
          { label: "URL shapes" },
          { label: "Pages", numeric: true },
          { label: "Share", numeric: true, render: (v) => fmt.pct(v as number) },
          { label: "Median words", numeric: true },
          { label: "Median depth", numeric: true },
          { label: "Example", render: (v) => (v ? link(v as string) : "") },
        ],
        rows,
        { pageSize: 25, initialSort: [2, "desc"] },
      )),
    section("One-off pages", "Pages whose layout no other page shares: bespoke landing pages, campaign pages, hubs. Each is its own design and build item.",
      dataTable(
        [{ label: "Page" }, { label: "Title" }, { label: "Depth", numeric: true }, { label: "PageRank", numeric: true }],
        oneOffs,
        { pageSize: 25, initialSort: [3, "desc"] },
      )),
  ];
}

export function pages(d: ReportData): HTMLElement[] {
  const p = d.pages;
  const u = d.usage;
  const tpl = new Map(d.templates.map((t) => [t.id, t.pages > 1 ? `#${t.id} ${t.pattern.split(" · ")[0]}` : "one-off"]));
  const rows: Cell[][] = p.url.map((url, i) => [
    url, p.title[i], p.template[i] == null ? null : tpl.get(p.template[i]!) ?? null, p.depth[i], p.pagerank[i], p.inlinks[i], p.main_words[i],
    ...(u ? [u.sessions[i], u.search_clicks[i]] : []),
    p.indexability[i], p.in_sitemap[i],
  ]);
  const columns: Column[] = [
    { label: "Page", render: (v) => link(v as string, pathOf(v as string)) },
    { label: "Title" },
    { label: "Template" },
    { label: "Depth", numeric: true },
    { label: "PageRank", numeric: true },
    { label: "Links in", numeric: true },
    { label: "Words", numeric: true },
    ...(u ? [{ label: "Sessions", numeric: true }, { label: "Search clicks", numeric: true }] : []),
    { label: "Indexability" },
    { label: "In sitemap" },
  ];
  return [
    section("All pages", `Every HTML page that answered 200, ordered by internal PageRank. ${fmt.int(p.url.length)} pages.`,
      dataTable(columns, rows, { pageSize: 50 })),
  ];
}
