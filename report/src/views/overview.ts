import { barChart, columnChart, type Datum } from "../charts";
import type { ReportData, Severity } from "../types";
import { fmt, h, section, severityBadge } from "../ui";

const SEVERITY_ORDER: Severity[] = ["critical", "serious", "warning", "info"];

function kpi(label: string, value: string, detail?: string) {
  return h("div", { class: "kpi" }, h("div", { class: "label" }, label), h("div", { class: "value" }, value), detail ? h("div", { class: "detail" }, detail) : null);
}

export function overview(d: ReportData): HTMLElement[] {
  const p = d.pages;
  const n = p.url.length;
  const depths = p.depth.filter((x): x is number => x != null);
  const sortedDepths = [...depths].sort((a, b) => a - b);
  const median = sortedDepths.length ? sortedDepths[Math.floor(sortedDepths.length / 2)] : 0;
  const maxDepth = sortedDepths.length ? sortedDepths[sortedDepths.length - 1] : 0;
  const indexable = p.indexability.filter((x) => x === "indexable").length;
  const multi = d.templates.filter((t) => t.pages > 1);
  const oneOff = d.templates.filter((t) => t.pages === 1).length;
  const issueCount = (sev: Severity) => d.issues.filter((i) => i.severity === sev && i.total > 0).length;

  const out: HTMLElement[] = [];

  const chips = h("div", { class: "chips" });
  for (const pl of d.site.platform ?? []) chips.append(h("span", { class: "chip" }, pl));
  if (d.site.hosting) chips.append(h("span", { class: "chip" }, `hosted on ${d.site.hosting}`));
  chips.append(h("span", { class: "chip" }, `crawled ${new Date(d.site.started_at).toLocaleDateString("en-GB", { dateStyle: "medium" })}`));
  out.push(h("div", {}, chips));

  if (d.site.refused > 0 && d.site.refused >= d.site.crawled * 0.02) {
    out.push(
      h("div", { class: "notice" }, severityBadge("serious"),
        h("div", {},
          `The server refused ${fmt.int(d.site.refused)} requests (HTTP 401/403/429)${d.site.stop_reason ? " and the crawl stopped early" : ""}. `,
          "This is typically bot protection reacting to the crawl. Those URLs are listed separately and not counted as broken; findings cover the pages fetched before the refusals.")),
    );
  }
  if (d.site.capped) {
    out.push(
      h("div", { class: "notice" }, severityBadge("warning"),
        h("div", {},
          `The crawl stopped at its ${fmt.int(d.site.max_pages)}-URL limit, while the XML sitemaps list ${fmt.int(d.site.sitemap_urls)} URLs. `,
          "Link counts, orphans and sitemap findings describe the crawled part of the site only.")),
    );
  }

  out.push(
    h("div", { class: "kpis" },
      kpi("Pages analysed", fmt.int(n), `${fmt.int(d.site.crawled)} URLs requested`),
      kpi("Templates", fmt.int(multi.length), `${fmt.int(multi.reduce((a, t) => a + t.pages, 0))} pages · ${fmt.int(oneOff)} one-off pages`),
      kpi("Click depth", `${median} / ${maxDepth}`, "median / deepest page"),
      kpi("Indexable", fmt.pct(n ? indexable / n : 0), `${fmt.int(indexable)} of ${fmt.int(n)} pages`),
      kpi("Issues found", fmt.int(issueCount("critical") + issueCount("serious")), `${issueCount("critical")} critical · ${issueCount("serious")} serious · ${issueCount("warning")} warnings`),
    ),
  );

  // Click depth distribution
  const byDepth = new Map<number, number>();
  for (const x of depths) byDepth.set(x, (byDepth.get(x) ?? 0) + 1);
  const depthData: Datum[] = [];
  for (let i = 0; i <= maxDepth; i++) {
    const v = byDepth.get(i) ?? 0;
    depthData.push({ label: String(i), value: v, tip: [["pages", fmt.int(v)], ["share", fmt.pct(n ? v / n : 0)]] });
  }
  const unreachable = n - depths.length;
  if (unreachable > 0) depthData.push({ label: "unreachable", value: unreachable, muted: true });

  // Template mix: largest templates, then the rest folded into one bar, one-offs last.
  const top = multi.slice(0, 8);
  const restPages = multi.slice(8).reduce((a, t) => a + t.pages, 0);
  const templateData: Datum[] = top.map((t) => ({
    label: t.pattern.split(" · ")[0],
    value: t.pages,
    tip: [["pages", fmt.int(t.pages)], ["template", `#${t.id}`], ["shapes", t.pattern]],
  }));
  if (restPages) templateData.push({ label: `${multi.length - 8} smaller templates`, value: restPages, muted: true });
  if (oneOff) templateData.push({ label: "One-off pages", value: oneOff, muted: true });

  out.push(
    h("div", { class: "grid-2" },
      section("Pages by click depth", "Clicks from the homepage along internal links. Deeper pages are crawled less and receive less link equity.",
        columnChart(depthData, { xLabel: "clicks from homepage" })),
      section("Pages by template", "Pages grouped by the layout of their main content.", barChart(templateData)),
    ),
  );

  // Issue summary
  const list = h("ul", { class: "issue-list" });
  const issues = [...d.issues].filter((i) => i.total > 0)
    .sort((a, b) => SEVERITY_ORDER.indexOf(a.severity) - SEVERITY_ORDER.indexOf(b.severity) || b.total - a.total);
  for (const i of issues) {
    list.append(h("li", {}, severityBadge(i.severity), h("a", { href: `#issues/${i.id}` }, i.title), h("span", { class: "count" }, fmt.int(i.total))));
  }
  out.push(section("Issues", "Ordered by severity, then by how many URLs are affected.", issues.length ? list : h("p", {}, "No issues found.")));
  return out;
}
