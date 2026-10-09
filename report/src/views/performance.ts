import { barChart, columnChart, type Datum } from "../charts";
import type { Cell, PerfGroup, ReportData } from "../types";
import { dataTable, fmt, h, section, type Column } from "../ui";

const pct = (v: Cell) => (typeof v === "number" ? fmt.pct(v) : "–");
const rate = (v: Cell) => (typeof v === "number" ? `${(v * 100).toFixed(2)}%` : "–");
const num = (v: Cell) => (typeof v === "number" ? fmt.int(v) : "–");

function kpi(label: string, value: string, detail?: string) {
  return h("div", { class: "kpi" }, h("div", { class: "label" }, label), h("div", { class: "value" }, value), detail ? h("div", { class: "detail" }, detail) : null);
}

function groupTable(rows: PerfGroup[], first: string): HTMLElement {
  const cols: Column[] = [
    { label: first },
    { label: "Pages", numeric: true },
    { label: "With sessions", numeric: true },
    { label: "Sessions", numeric: true, render: num },
    { label: "Sessions / page", numeric: true, render: (v) => (typeof v === "number" ? v.toFixed(1) : "–") },
    { label: "Engagement", numeric: true, render: pct },
    { label: "Key events / session", numeric: true, render: rate },
    { label: "Search clicks", numeric: true, render: num },
  ];
  const data: Cell[][] = rows.map((r) => [r.label, r.pages, r.pages_with_sessions, r.sessions, r.sessions_per_page, r.engagement_rate, r.key_events_per_session, r.search_clicks]);
  return dataTable(cols, data, { pageSize: 15, search: false });
}

export function performanceView(d: ReportData): HTMLElement[] {
  const p = d.performance;
  if (!p) return [section("Performance", undefined, h("p", {}, "No usage data loaded. Fetch GA4 / Search Console or import CSV exports, then re-run the report."))];
  const t = p.totals ?? {};
  const out: HTMLElement[] = [];

  const sources = p.sources
    .map((s) => `${s.source.startsWith("ga4") ? "Google Analytics 4" : "Search Console"} (${s.detail ?? ""}${s.date_start ? `, ${s.date_start} → ${s.date_end}` : ""}, ${s.method === "api" ? "fetched" : "imported"})`)
    .join(" · ");
  const matched = p.matched && typeof p.matched === "object" ? (p.matched as Record<string, number>) : null;
  out.push(
    h("div", { class: "notice info" },
      h("div", {},
        h("strong", {}, "Real usage data. "), sources, ". ",
        matched && matched.ga_sessions ? `${fmt.pct(matched.share_matched)} of sessions land on pages in this crawl. ` : "",
        h("span", { class: "muted" }, p.caveats.join(" ")))),
  );

  out.push(
    h("div", { class: "kpis" },
      kpi("Sessions on crawled pages", num(t.sessions ?? null), `${num(t.pages_with_sessions ?? null)} of ${num(t.pages ?? null)} pages got visits`),
      kpi("Engagement rate", pct(t.engagement_rate ?? null), "engaged sessions / sessions"),
      kpi("Key events", num(t.key_events ?? null), `${rate(t.key_events_per_session ?? null)} of sessions`),
      kpi("Organic share", pct(t.organic_share ?? null), "sessions from organic search"),
      kpi("Search clicks", num(t.search_clicks ?? null), `${num(t.search_impressions ?? null)} impressions`),
    ),
  );

  // What works: by page type
  const tplBars: Datum[] = p.by_template.slice(0, 10).map((r) => ({
    label: r.label, value: r.sessions_per_page ?? 0, muted: r.label === "one-off pages",
    tip: [["sessions / page", (r.sessions_per_page ?? 0).toFixed(1)], ["pages", fmt.int(r.pages)], ["engagement", pct(r.engagement_rate)], ["key events / session", rate(r.key_events_per_session)]],
  }));
  out.push(
    h("div", { class: "grid-2" },
      section("Sessions per page, by template", "Which page types earn their keep. Totals favour big templates, so compare per page.", barChart(tplBars, { labelWidth: 280 })),
      section("Sessions per page, by click depth", "How visits fall off with clicks from the homepage.",
        columnChart(p.by_depth.map((r) => ({ label: r.label, value: r.sessions_per_page ?? 0, muted: r.label === "unreachable",
          tip: [["sessions / page", (r.sessions_per_page ?? 0).toFixed(1)], ["pages", fmt.int(r.pages)]] })), { xLabel: "clicks from homepage" })),
    ),
  );
  out.push(section("By template", "Engagement and key-event rates show what each page type does once visitors arrive.", groupTable(p.by_template, "Template")));
  if (p.by_topic.length) out.push(section("By topic", "Which subjects draw visits and convert.", groupTable(p.by_topic, "Topic")));
  if (p.by_section.length) {
    const cols: Column[] = [
      { label: "Section type" },
      { label: "Pages with", numeric: true },
      { label: "Pages without", numeric: true },
      { label: "Engagement with", numeric: true, render: pct },
      { label: "Engagement without", numeric: true, render: pct },
      { label: "Key events / session with", numeric: true, render: rate },
      { label: "Key events / session without", numeric: true, render: rate },
    ];
    const rows: Cell[][] = p.by_section.map((r) => [r.section_type, r.pages_with, r.pages_without, r.engagement_with, r.engagement_without, r.key_events_per_session_with, r.key_events_per_session_without]);
    out.push(section("Pages with vs without each section type", "Among pages with 20+ sessions. A hint for which blocks to keep in the new templates, not proof: pages with a section also differ in other ways.", dataTable(cols, rows, { pageSize: 20, search: false })));
  }

  // Opportunities
  const titles: Record<string, string> = {
    top_converters: "Top converting landing pages",
    hidden_winners: "Hidden winners",
    over_promoted: "Over-promoted pages",
    poor_engagement: "Traffic that doesn't engage",
    low_ctr: "Ranking, but not getting clicked",
    striking_distance: "Striking-distance queries",
    unmatched_traffic: "Traffic to URLs outside the crawl",
    no_traffic: "Pages with no traffic",
  };
  const order = ["top_converters", "unmatched_traffic", "low_ctr", "striking_distance", "hidden_winners", "poor_engagement", "over_promoted", "no_traffic"];
  for (const key of order) {
    const o = p.opportunities[key];
    if (!o || !o.count) continue;
    const columns = Object.keys(o.rows[0] ?? {});
    const numeric = columns.map((c) => o.rows.every((r) => r[c] == null || typeof r[c] === "number"));
    const cols: Column[] = columns.map((c, i) => ({
      label: c.replace(/_/g, " ").replace(/^./, (x) => x.toUpperCase()),
      numeric: numeric[i],
      render: c.includes("rate") || c.endsWith("ctr") ? pct : c.endsWith("per_session") ? rate : undefined,
    }));
    const rows: Cell[][] = o.rows.map((r) => columns.map((c) => r[c] ?? null));
    out.push(
      h("details", { class: "card issue", open: key === "top_converters" || key === "unmatched_traffic" },
        h("summary", {}, h("span", { class: "tag" }, "opportunity"), h("h3", {}, titles[key] ?? key), h("strong", {}, fmt.int(o.count))),
        h("p", { class: "desc" }, o.description),
        dataTable(cols, rows, { pageSize: 15 })),
    );
  }
  return out;
}
