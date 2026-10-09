import { barChart, type Datum } from "../charts";
import type { ReportData } from "../types";
import { dataTable, fmt, h, hideTip, link, pathOf, section, showTip } from "../ui";

// Section groups are adjacent blocks in a stack, so four categorical slots are safe;
// every block also carries a text label, so colour is never the only cue.
const GROUPS: [string, string][] = [
  ["content", "--series-1"],
  ["intro & conversion", "--series-2"],
  ["lists & navigation", "--series-3"],
  ["proof & detail", "--series-4"],
  ["minor", "--other"],
];
const groupVar = (g: string) => GROUPS.find(([k]) => k === g)?.[1] ?? "--other";

const KIND_LABEL: Record<string, string> = {
  hero: "Hero", cta: "Call to action", form: "Form", listing: "Listing / cards", testimonials: "Testimonials",
  logos: "Logo strip", stats: "Stats", team: "Team", pricing: "Pricing", faq: "FAQ", article: "Article body",
  features: "Features", content: "Content", media: "Media", video: "Video", minor: "Minor",
};
const kindLabel = (k: string) => KIND_LABEL[k] ?? k;

function sectionSlice(u: URL | null): string {
  if (!u) return "/";
  const seg = u.pathname.split("/").filter(Boolean)[0];
  return seg ? `/${seg}` : "/";
}

function legend(): HTMLElement {
  const l = h("div", { class: "legend" });
  for (const [g, v] of GROUPS) l.append(h("span", { class: "key" }, h("span", { class: "swatch", style: `background:var(${v})` }), g));
  return l;
}

/** Topics (rows) against where they live (columns): URL sections or templates. */
function topicMatrix(d: ReportData): HTMLElement {
  const p = d.pages;
  const topics = d.topics.slice(0, 14);
  const holder = h("div", {});
  const sel = h("select", { "aria-label": "Columns" }, h("option", { value: "section" }, "URL section"), h("option", { value: "template" }, "Template"));

  function render() {
    const byTemplate = sel.value === "template";
    const colOf = (i: number) => {
      if (byTemplate) {
        const t = d.templates.find((x) => x.id === p.template[i]);
        return t && t.pages > 1 ? `#${t.id} ${t.pattern.split(" · ")[0]}` : "one-off pages";
      }
      try { return sectionSlice(new URL(p.url[i])); } catch { return "/"; }
    };
    const colCount = new Map<string, number>();
    p.url.forEach((_, i) => colCount.set(colOf(i), (colCount.get(colOf(i)) ?? 0) + 1));
    const cols = [...colCount.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12).map(([c]) => c);
    const counts = topics.map((t) => cols.map((c) => p.url.filter((_, i) => p.topic[i] === t.id && colOf(i) === c).length));
    const max = Math.max(1, ...counts.flat());
    const steps = ["--seq-100", "--seq-200", "--seq-300", "--seq-450", "--seq-600"];
    const table = h("table", { class: "heatmap" });
    const head = h("tr", {}, h("th", {}, "Topic"));
    for (const c of cols) head.append(h("th", { class: "rot", title: c }, h("span", {}, c.length > 28 ? `${c.slice(0, 27)}…` : c)));
    table.append(h("thead", {}, head));
    const body = h("tbody");
    topics.forEach((t, r) => {
      const tr = h("tr", {}, h("th", { scope: "row" }, t.label));
      cols.forEach((c, k) => {
        const v = counts[r][k];
        const n = v === 0 ? -1 : Math.min(steps.length - 1, Math.floor((v / max) * steps.length));
        const td = h("td", { class: `cell${n >= 0 ? ` step-${n}` : ""}`, style: n >= 0 ? `background:var(${steps[n]})` : "", tabindex: 0 }, v ? String(v) : "");
        const tip = (e: { clientX: number; clientY: number }) => showTip(e.clientX, e.clientY, [["pages", fmt.int(v)], [byTemplate ? "template" : "section", c]], t.label);
        td.addEventListener("pointermove", tip);
        td.addEventListener("pointerleave", hideTip);
        tr.append(td);
      });
      body.append(tr);
    });
    table.append(body);
    holder.replaceChildren(h("div", { class: "table-scroll" }, table));
  }
  sel.addEventListener("change", render);
  render();
  return h("div", {}, h("div", { class: "controls" }, h("label", {}, "Columns", sel)), holder);
}

function block(kind: string, group: string, heading: string | null, words: number, topic: string | null, share?: number): HTMLElement {
  const height = Math.round(Math.min(200, 30 + Math.sqrt(words) * 7));
  return h("div", { class: `block group-${group.replace(/\W+/g, "-")}`, style: `min-height:${height}px;border-left-color:var(${groupVar(group)})` },
    h("div", { class: "block-head" }, h("strong", {}, kindLabel(kind)), share != null ? h("span", { class: "muted" }, ` · on ${fmt.pct(share)} of pages`) : null),
    heading ? h("div", { class: "block-heading" }, heading) : null,
    h("div", { class: "block-meta muted" }, [words ? `${fmt.int(words)} words` : null, topic].filter(Boolean).join(" · ")),
  );
}

function anatomy(d: ReportData): HTMLElement {
  const p = d.pages, s = d.sections;
  const topicLabel = new Map(d.topics.map((t) => [t.id, t.label]));
  const byPage = new Map<number, number[]>();
  s.page.forEach((pg, i) => { if (!byPage.has(pg)) byPage.set(pg, []); byPage.get(pg)!.push(i); });

  const templates = d.templates.filter((t) => t.pages > 1);
  const oneOffs = d.templates.filter((t) => t.pages === 1);
  const tplSel = h("select", { "aria-label": "Template" });
  for (const t of templates) tplSel.append(h("option", { value: String(t.id) }, `#${t.id} ${t.pattern.split(" · ")[0]} (${t.pages} pages)`));
  if (oneOffs.length) tplSel.append(h("option", { value: "oneoff" }, `One-off pages (${oneOffs.length})`));
  const pageSel = h("select", { "aria-label": "Page" });
  const typical = h("div", { class: "stack" });
  const single = h("div", { class: "stack" });

  function pagesOf(): number[] {
    const ids = tplSel.value === "oneoff" ? new Set(oneOffs.map((t) => t.id)) : new Set([Number(tplSel.value)]);
    return p.url.map((_, i) => i).filter((i) => p.template[i] != null && ids.has(p.template[i]!));
  }

  function renderTypical() {
    const pages = pagesOf();
    pageSel.replaceChildren();
    for (const i of pages.slice(0, 300)) pageSel.append(h("option", { value: String(i) }, pathOf(p.url[i])));
    typical.replaceChildren();
    if (tplSel.value === "oneoff") {
      typical.append(h("p", { class: "muted" }, "One-off pages share no layout; pick one on the right."));
    } else {
      // Most common section type at each position, across the template's pages.
      const lengths = pages.map((i) => (byPage.get(i) ?? []).length).sort((a, b) => a - b);
      const median = lengths[Math.floor(lengths.length / 2)] ?? 0;
      for (let pos = 0; pos < median; pos++) {
        const tally = new Map<string, { n: number; words: number; group: string; topics: Map<number, number> }>();
        for (const i of pages) {
          const si = byPage.get(i)?.[pos];
          if (si == null) continue;
          const e = tally.get(s.kind[si]) ?? { n: 0, words: 0, group: s.group[si], topics: new Map() };
          e.n++;
          e.words += s.words[si];
          if (s.topic[si] != null) e.topics.set(s.topic[si]!, (e.topics.get(s.topic[si]!) ?? 0) + 1);
          tally.set(s.kind[si], e);
        }
        const [kind, e] = [...tally.entries()].sort((a, b) => b[1].n - a[1].n)[0] ?? [];
        if (!kind || !e) continue;
        const topTopic = [...e.topics.entries()].sort((a, b) => b[1] - a[1])[0];
        typical.append(block(kind, e.group, null, Math.round(e.words / e.n), topTopic ? `mostly “${topicLabel.get(topTopic[0])}”` : null, e.n / pages.length));
      }
      if (!median) typical.append(h("p", { class: "muted" }, "No sections detected for this template."));
    }
    renderPage();
  }

  function renderPage() {
    single.replaceChildren();
    const i = Number(pageSel.value);
    if (Number.isNaN(i)) return;
    single.append(h("div", { class: "muted" }, link(p.url[i], p.title[i] ?? pathOf(p.url[i]))));
    const secs = byPage.get(i) ?? [];
    if (!secs.length) single.append(h("p", { class: "muted" }, "No sections detected (content may be rendered by JavaScript)."));
    for (const si of secs) {
      single.append(block(s.kind[si], s.group[si], s.heading[si], s.words[si], s.topic[si] != null ? topicLabel.get(s.topic[si]!) ?? null : null));
    }
  }

  tplSel.addEventListener("change", renderTypical);
  pageSel.addEventListener("change", renderPage);
  renderTypical();
  return h("div", {},
    h("div", { class: "controls" }, h("label", {}, "Template", tplSel), h("label", {}, "Page", pageSel)),
    legend(),
    h("div", { class: "anatomy" },
      h("div", {}, h("h3", {}, "Typical page"), h("p", { class: "sub" }, "The most common section at each position across this template’s pages."), typical),
      h("div", {}, h("h3", {}, "This page"), h("p", { class: "sub" }, "Sections top to bottom; block height follows word count."), single)));
}

function inventory(d: ReportData): HTMLElement {
  const s = d.sections, p = d.pages;
  const tpls = d.templates.filter((t) => t.pages > 1).slice(0, 8);
  const kinds = [...new Set(s.kind)].filter((k) => k !== "minor");
  const rows = kinds.map((k) => {
    const idx = s.kind.map((x, i) => (x === k ? i : -1)).filter((i) => i >= 0);
    const pagesWith = new Set(idx.map((i) => s.page[i]));
    return [
      kindLabel(k),
      idx.length,
      pagesWith.size,
      ...tpls.map((t) => new Set(idx.filter((i) => p.template[s.page[i]] === t.id).map((i) => s.page[i])).size),
    ];
  });
  return dataTable(
    [
      { label: "Section type" },
      { label: "Sections", numeric: true },
      { label: "Pages", numeric: true },
      ...tpls.map((t) => ({ label: `#${t.id} ${t.pattern.split(" · ")[0].slice(0, 22)}`, numeric: true })),
    ],
    rows,
    { pageSize: 20, initialSort: [2, "desc"], search: false },
  );
}

export function contentView(d: ReportData): HTMLElement[] {
  if (!d.topics?.length && !d.sections?.page?.length) {
    return [section("Content", undefined, h("p", {}, "No content analysis in this report. Re-run `sitecannon analyze` with a crawl that stored HTML."))];
  }
  const topicBars: Datum[] = d.topics.slice(0, 16).map((t) => ({ label: t.label, value: t.pages, tip: [["pages", fmt.int(t.pages)], ["key terms", t.terms.join(", ")]] }));
  return [
    h("div", { class: "grid-2" },
      section("Topics", "What the site talks about, found from the words on each page (each page counted once, in its closest topic). Labels are the terms that set a topic apart.", barChart(topicBars, { labelWidth: 300 })),
      section("Where topics live", "Pages per topic in each part of the site. A topic spread across many sections, or a section mixing many topics, is an IA conversation.", topicMatrix(d))),
    section("Page anatomy", "What sits where on the page: each page split into its sections, typed by role (hero, features, testimonials, FAQ…) and tagged with their topic.", anatomy(d)),
    section("Section inventory", "How often each kind of section appears, and in which templates. This is the starting component list for the rebuild.", inventory(d)),
  ];
}
