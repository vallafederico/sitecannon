---
name: seo-migration
description: Prepare an SEO-safe site migration or replatform with SiteCannon — inventory current URLs, find the pages that carry the most internal value, surface redirect and indexing problems to fix first, and draft the redirect map and launch checklist. Use when the user mentions migration, replatforming, moving CMS (WordPress/Webflow/Framer/Next.js…), changing URLs or domains, redirects, or "not losing SEO" in a rebuild.
---

# SEO migration prep

Output: a risk summary, a prioritised URL inventory, a redirect-map skeleton (CSV) and a launch
checklist. Tools: the `sitecannon` MCP server or `sitecannon tool <name> '<json>'`. A full crawl
matters here: if `summary.caveats` says the crawl was capped, recommend a full crawl (with the site
owner's permission) before finalising.

## 1. Baseline and risks

From `summary` and `issues`:
- **Redirect debt:** `redirect_chains` and `links_to_redirects`. Old hosts or paths still linked internally will gain another hop at launch. Fix these first.
- **Indexing hygiene:** `sitemap_non_indexable`, `canonicalised`, `noindex`, and missing titles or descriptions. Don't migrate problems.
- **Broken links:** `broken_links`, fixed in the new build.
- **Rendering:** platform `streaming-ssr`, or titles missing in the server HTML (`missing_title`). The new stack should send titles, meta and main content in the server HTML.
- **Structured data:** `no_structured_data`. Carry over what exists and add what's missing.

## 2. Prioritise URLs

Prefer real usage data. Check `analytics_status`; if nothing is loaded, strongly recommend connecting
GA4 and Search Console (`analytics-setup` skill) before finalising a migration plan, ideally with
12+ months of data.

**With usage data**, rank by what the pages actually earn:

```sql
SELECT url, title, sessions, key_events, revenue, search_clicks, search_impressions,
       round(pagerank,2) AS pagerank, click_depth, template_pattern
FROM page_performance
ORDER BY key_events DESC, search_clicks DESC, sessions DESC
```

Also pull `performance` → `opportunities.unmatched_traffic`: URLs that still get visits but aren't
live pages (old redirects, broken URLs). Every one needs a working redirect in the new site.

**Without usage data**, fall back to internal signals and say so: PageRank, inlinks, sitemap
inclusion and click depth, from `pages_full`.

Tier them: **Tier 1** is the pages with key events or revenue, the top ~10% by search clicks or sessions, and hubs and landing pages (manual mapping and QA). **Tier 2** is templated content with traffic (pattern rules plus spot checks). **Tier 3** is everything else (pattern rules plus a sample check). Backlink data, if the user has it, can promote pages to Tier 1.

## 3. Redirect-map skeleton

Export every URL that needs a decision (`query`):

```sql
-- with usage data (otherwise use pages_full and drop the usage columns)
SELECT url AS old_url, '' AS new_url, template_pattern AS template, sessions, key_events, search_clicks,
       round(pagerank,2) AS pagerank, click_depth, '' AS tier, '' AS action, '' AS notes
FROM page_performance ORDER BY key_events DESC, search_clicks DESC, sessions DESC
```

Plus the existing redirects that must keep working: `SELECT url AS old_url, resolved_url FROM redirect_map`.
Write them as CSV files for the user. Where the target structure is known, propose pattern rules
(e.g. `/blog/:slug → /journal/:slug`) and fill in `new_url` for the matches. Never map unrelated pages
to the homepage: Google treats that like a missing page and the page's ranking value is lost. Use the
closest relevant parent, or 410.

## 4. Launch checklist (adapt to the stack)

Before: fix the redirect debt and broken links; freeze the URL inventory; benchmark (crawl kept as
the baseline). At launch: one-hop 301s, self-canonicals, robots.txt and noindex removed from
production, new XML sitemap, titles, meta and structured data in the server HTML. After: re-crawl
with SiteCannon into a new project and compare titles, status codes and redirects against the
baseline. Re-fetch analytics weekly and compare sessions and search clicks for Tier 1 URLs (old →
new) against the pre-launch baseline. Watch Search Console daily for 2 weeks, then weekly.

## 5. Deliver

Risk summary and tiers in chat; CSV files on disk; optional brief via `report` with `notes`.
