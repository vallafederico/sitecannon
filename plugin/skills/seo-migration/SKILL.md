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

There's no traffic or backlink data in SiteCannon, so use internal signals and say that's what you
did. Rank content pages by internal PageRank, inlinks, sitemap inclusion and click depth:

```sql
SELECT url, title, round(pagerank,2) AS pagerank, inlinks, click_depth, in_sitemap, template_pattern
FROM pages_full ORDER BY pagerank DESC
```

Tier them: **Tier 1** is the top ~10% plus hubs and landing pages (manual mapping and QA); **Tier 2**
is templated content (pattern rules); **Tier 3** is everything else (pattern rules plus a sample
check). Recommend adding Search Console clicks and backlink data before launch to confirm Tier 1.

## 3. Redirect-map skeleton

Export every URL that needs a decision (`query`):

```sql
SELECT url AS old_url, '' AS new_url, template_pattern AS template, round(pagerank,2) AS pagerank,
       click_depth, in_sitemap, '' AS action, '' AS notes
FROM pages_full ORDER BY pagerank DESC
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
baseline; watch Search Console daily for 2 weeks, then weekly.

## 5. Deliver

Risk summary and tiers in chat; CSV files on disk; optional brief via `report` with `notes`.
