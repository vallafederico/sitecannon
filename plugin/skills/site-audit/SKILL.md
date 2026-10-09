---
name: site-audit
description: End-to-end audit of a website before a rebuild or redesign — crawl it with SiteCannon, read the analysis, investigate the important findings, write a strategy brief and produce the single-file client report. Use when the user gives a URL and asks to audit, analyse, review, assess or "look at" a site, prepare for a pitch or discovery, or asks for a site/SEO/IA report.
---

# Site audit

Goal: a short, evidence-backed strategy brief embedded in the SiteCannon HTML report, plus a
chat summary. The reader is an agency team or their client deciding what to rebuild and how.

Tools come from the `sitecannon` MCP server, or run `sitecannon tool <name> '<json>'` in a shell.
For tool details, tables and SQL, load the `sitecannon-data` skill.

## 1. Get the data

1. `list_projects`. If the site is already there and recent, reuse it; tell the user the crawl date.
2. Otherwise `crawl` with the URL the user gave. Defaults are deliberately polite (3 req/s, 500 pages).
   - Raise `max_pages` only if the user wants full coverage, and say how long it will take (pages ÷ rps).
   - Never crawl sites the user didn't ask about. Never raise rps above 5 on a site you don't own.
   - If the crawl stops early because the site refused requests, don't retry. Report what was collected and suggest the owner allowlists the crawler.
3. `summary`. Note every entry in `caveats`; they qualify everything you conclude.
4. **Real usage data:** `summary.usage_data` says whether GA4 / Search Console data is loaded. If it isn't, ask once whether the user can connect GA4 and Search Console, or send exports. If they can, follow the `analytics-setup` skill. If not, carry on with crawl data only and say that traffic wasn't available.

## 2. Investigate (depth over breadth)

Work through these, drilling down only where `summary` shows something worth explaining:

- **Structure**: `sitemap` with `kind` `url`, `nav` and `click`. Where do URLs, menus and click paths disagree? Look for sections missing from the navigation, folders without landing pages, deep or orphaned content.
- **Templates & content**: `templates` gives how many layouts the rebuild needs, the one-off pages, and typical section order. `topics` shows what the site is about and whether topics map cleanly onto sections.
- **Issues**: `issues`, then `issues` with `id` for the critical and serious ones. Find the *cause* (one template? a CMS field? old links in article bodies?), not just the count. Use `page` on a few examples to confirm.
- **Performance** (when usage data is loaded): run the `performance-review` skill. What earns visits, engagement and conversions should drive which findings matter most.
- **Spot-check** anything surprising with `page` or `query` before you state it. A finding you haven't verified gets "appears to" or is left out.

## 3. Write the brief (markdown)

Keep it under ~600 words. Lead with conclusions. Every claim needs a number or an example URL.

```markdown
# <Site> — rebuild audit
*Crawled <date>; <N> pages analysed. <coverage caveat if any>.*

## The short version
3–5 bullets: what matters most for the rebuild, in plain language.

## What's there today
Platform, size, templates (N templates + M one-offs), main sections and topics, navigation.

## Key findings
For each (3–6): **Finding** → evidence (numbers, 1–2 example URLs) → why it matters → recommendation.
Order by impact on the rebuild / search visibility, not by tool order.

## Rebuild implications
Templates and components to design, content to migrate/merge/drop, URL and redirect considerations.

## Next steps
Concrete actions, and what data would sharpen this (e.g. analytics, Search Console, owner allowlisting).
```

Tone: direct, specific and non-alarmist. Explain jargon once. Don't pad with generic SEO advice.

## 4. Produce the report

Call `report` with `notes` set to the brief. Give the user the report path (it's one self-contained
HTML file that opens offline) and a 5–8 line summary in chat.

For a migration-focused or content-focused engagement, follow up with the `seo-migration`,
`ia-review` or `content-audit` skills.
