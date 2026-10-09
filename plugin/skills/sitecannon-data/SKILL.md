---
name: sitecannon-data
description: Reference for SiteCannon's tools and project database — what each tool returns, the tables behind them, a SQL cookbook, and the caveats to respect. Load this before answering detailed questions about a crawled site, writing SQL for the `query` tool, or when another SiteCannon skill needs data it doesn't cover.
---

# SiteCannon data reference

SiteCannon crawls a website into a project (a DuckDB database) and analyses it. You reach it through
tools. Prefer the MCP tools (server `sitecannon`); if they aren't available, the same tools run in a
shell and print JSON:

```sh
sitecannon tool <name> '<json args>'      # e.g. sitecannon tool summary '{"project":"example.com"}'
sitecannon tools                          # list tools with input schemas
```

Projects live under `$SITECANNON_HOME` (default `./projects`). A project is named after the host.

## Tools

| Tool | Use it for |
|---|---|
| `list_projects` | What's been crawled, when, and whether there's a report |
| `crawl` | New site → project (crawls then analyses). Slow: ~max_pages / rps seconds |
| `summary` | **Read first.** Site facts, coverage caveats, structure, nav, templates + typical sections, topics, issue counts, top pages |
| `sitemap` | Nested tree: `kind` = `url` (URL folders), `nav` (persistent menus), `click` (shortest click path) |
| `templates` | Layout clusters with URL shapes, median words, typical section order; one-off pages; section inventory |
| `topics` | Topics with key terms, where they live (sections, templates), example pages |
| `issues` | Checks with severity and counts; pass `id` for affected URLs and the SQL behind it |
| `page` | One page in full: SEO fields, metrics, sections, topic, inlinks, broken links on it |
| `schema` / `query` | Read-only DuckDB SQL for anything the other tools don't answer (≤500 rows) |
| `analyze` | Recompute analysis (rarely needed; crawl already does it) |
| `report` | Write the single-file HTML report; `notes` embeds your markdown brief at the top |
| `analytics_status` | Which usage data is loaded (GA4, Search Console), date range, match rate, credentials available |
| `fetch_analytics` | Pull GA4 and/or Search Console data with the user's Google credentials |
| `import_analytics` | Load GA4 / Search Console CSV exports (kind detected from columns) |
| `performance` | What works and what doesn't: by template, topic, depth, section type, plus opportunity lists |

## Concepts

- **Content page**: an HTML page that answered 200 without redirecting. Metrics, templates, topics and sections cover these.
- **Click depth**: shortest number of clicks from the homepage along internal links. `click_parent` is the most-linked page one click closer, which forms the click tree.
- **PageRank** (`pagerank`): internal link equity scaled so the site average is 1.0. Sitewide footer/nav links inflate legal and utility pages; read it alongside `inlinks` and link position.
- **Template**: pages whose main-content layout matches (site chrome excluded). `templates.pages = 1` is a one-off page. URL shapes look like `/{analysis|news|interview…}/*`: braces list the common values at that position.
- **Sections**: each page's top-level content blocks in order, typed `hero, features, listing, article, testimonials, logos, pricing, faq, cta, form, stats, team, video, media, content, minor`. They're rule-based, so treat them as good guesses and spot-check with `page`.
- **Topics**: TF-IDF + k-means over each page's own content sections. Labels are the most distinctive terms, not human names, so rename them in your own words when you report.
- **Link position**: `nav, header, footer, breadcrumb, aside, main, body`. `menu_group` is the dropdown or footer column label.
- **Indexability**: `indexable, redirect, noindex, canonicalised, client_error, server_error, refused, blocked_by_robots, not_html, fetch_error`.

## Caveats you must carry into any conclusion

`summary.caveats` lists them. Always restate the relevant ones.
- **Capped crawl** (`urls_crawled >= max_pages`): orphan, inlink and sitemap findings are partial.
- **Refused URLs** (401/403/429): bot protection, not missing pages. They're kept out of broken links.
- **JavaScript-built content**: the crawler reads server HTML. Pages assembled client-side may show missing titles or few sections. Platform `streaming-ssr` means content arrives in chunks after the footer.
- **Usage data is optional**: without GA4 / Search Console, "value" means internal link equity, depth and content, not visits. With it, state the date range, sources and `share_matched`. Analytics undercounts where consent banners block tracking, so compare ratios, not totals.

## SQL cookbook

Start from `pages_full` (content pages + metrics + template pattern). JSON columns support `->>`.

```sql
-- Biggest sections and whether they have a landing page
SELECT path, subtree_pages, url IS NOT NULL AS has_page FROM tree_url WHERE depth = 1 ORDER BY 2 DESC;

-- Most important pages by internal equity, excluding sitewide utility links
SELECT url, title, round(pagerank,2) pr, inlinks, click_depth FROM pages_full ORDER BY pagerank DESC LIMIT 25;

-- Pages with contextual (in-content) inlinks only from few pages: weakly integrated content
SELECT f.url, i.contextual_inlinks FROM pages_full f LEFT JOIN inlinks i USING (url)
WHERE coalesce(i.contextual_inlinks,0) <= 1 ORDER BY f.click_depth DESC;

-- Section types per template (component inventory)
SELECT m.template_id, s.section_type, count(DISTINCT s.url) pages
FROM page_sections s JOIN page_metrics m USING (url) GROUP BY ALL ORDER BY 1, 3 DESC;

-- Where a topic lives
SELECT '/' || split_part(regexp_replace(url,'^https?://[^/]+/?',''),'/',1) section, count(*)
FROM page_topics WHERE topic_id = 1 GROUP BY 1 ORDER BY 2 DESC;

-- Menu items and how widely they appear
SELECT region, menu_group, anchor, url, coverage FROM nav_items WHERE coverage >= 0.5 ORDER BY region, typical_ordinal;

-- Every redirecting URL and where it ends (migration input)
SELECT url, resolved_url FROM redirect_map;

-- Old-host or old-path links still used internally
SELECT to_url, count(*) links, count(DISTINCT from_url) pages FROM redirected_links GROUP BY 1 ORDER BY 2 DESC;

-- With usage data: page types that earn their keep (per page, not totals)
SELECT template_pattern, count(*) pages, round(sum(sessions)/count(*),1) sessions_per_page,
       round(sum(engaged_sessions)/nullif(sum(sessions),0),3) engagement, sum(key_events) key_events
FROM page_performance GROUP BY 1 ORDER BY sessions_per_page DESC;

-- Queries a page almost ranks for
SELECT query, page_url, impressions, position FROM gsc_queries WHERE position BETWEEN 4 AND 20 ORDER BY impressions DESC;
```

Usage tables (after `fetch_analytics` / `import_analytics`): `page_performance` (one row per
content page with sessions, engaged_sessions, engagement_rate, key_events, revenue, organic_sessions,
views, search_clicks/impressions/ctr/position), raw `ga_landing`, `ga_channels`, `ga_pages`,
`gsc_pages`, `gsc_queries`, `analytics_unmatched` and `analytics_meta`.

Use `DESCRIBE <table>` through `query` for exact columns.
