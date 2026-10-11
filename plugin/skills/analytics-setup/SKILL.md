---
name: analytics-setup
description: Connect a SiteCannon project to real usage data — Google Analytics 4 and Search Console — by fetching with the user's Google credentials or importing CSV exports, then check the data matched the crawl. Use when the user mentions GA, GA4, Google Analytics, Search Console, GSC, traffic, sessions, conversions, key events, "real data" or "actual usage", or provides analytics exports.
---

# Connect analytics (GA4 + Search Console)

Goal: usage data loaded into the project and matched to crawled pages, so `performance`,
`summary` and the report can show what actually works. Tools: the `sitecannon` MCP server, or
`sitecannon tool <name> '<json>'` / `sitecannon analytics …` in a shell.

The data is aggregate (per page and per query, never per person). It stays in the local project
database. Only connect properties the user owns or has been given access to.

## 1. Make sure there's a crawl

`list_projects`. If the site isn't crawled yet, crawl it first (see the `site-audit` skill). Usage
data is joined to crawled pages by URL path.

## 2. Pick a route

Call `analytics_status` with the project. It reports what's loaded and whether Google credentials
are available on this machine (`google_auth.ok`).

**A. Fetch directly (best: full detail, including query-level Search Console data).**
Needs credentials with read access to the GA4 property and/or the Search Console site. Help the
user with whichever fits:
1. Their own Google login (simplest). Have them run, in their terminal:
   ```sh
   gcloud auth application-default login \
     --scopes=https://www.googleapis.com/auth/analytics.readonly,https://www.googleapis.com/auth/webmasters.readonly,https://www.googleapis.com/auth/cloud-platform
   ```
   If Google complains about a quota project, run `gcloud auth application-default set-quota-project <gcp-project>`, using a project with the "Google Analytics Data API" and "Search Console API" enabled.
2. A service account (good for agencies): set `GOOGLE_APPLICATION_CREDENTIALS` to its JSON key, then have the client add the service-account email as a **Viewer** on the GA4 property (Admin → Property access management) and as a user on the Search Console property (Settings → Users and permissions).
3. An existing access token in `SITECANNON_GOOGLE_TOKEN`.

Never ask the user to paste keys or tokens into the chat. They set them in their own environment.

Then find the identifiers:
- **GA4 property id:** a number like `123456789`, under GA → Admin → Property settings. It isn't the `G-XXXX` measurement id.
- **Search Console property:** `sc-domain:example.com` for a domain property, or the exact URL-prefix property such as `https://www.example.com/` (trailing slash included).

Call `fetch_analytics` with `{"project": "...", "ga4_property": "123456789", "gsc_site": "sc-domain:example.com", "days": 90}`.
Use 90 days by default; use 365+ for seasonal businesses or a migration baseline (Search Console keeps 16 months).

**B. Exports (no credentials; for when you can't connect any other way).**
1. Get the client-ready instructions: `analytics_export_guide` (or `sitecannon analytics guide`). It explains both granting read-only access and exporting exactly the right reports: GA4 landing pages and pages, and the Search Console performance zip. Draft the request email around it in the user's voice, adapted to the client.
2. When the files arrive, import them as they are: `import_analytics` with `{"project": "...", "file": "/path/to/folder-or-file"}`. It accepts single CSV/TSV files, Excel or Google Sheets downloads (.xlsx), Search Console's zip, or a whole folder. Each file is recognised from its columns, irrelevant ones (countries, devices, dates…) are skipped, and the result lists what was imported and skipped.
3. Read `limits` in the result. Search Console's web export holds at most 1,000 pages and 1,000 queries, and GA4 downloads at most 100,000 rows. When a cap is hit, say so and suggest read-only access for full data.

Exports don't include Search Console's page-by-query breakdown, so striking-distance queries are listed without the ranking page. Use `append: true` to add files to what's already loaded.

## 3. Check the match

The fetch/import result includes `matched.share_matched`, the share of sessions landing on crawled pages.
- **Above 85%:** good.
- **Lower:** look at `performance` → `opportunities.unmatched_traffic`. Usual causes: the crawl was capped (crawl more), old URLs that redirect (expected, and important for migrations), other hostnames in the same GA property (ignored automatically), or tracking parameters that GA doesn't strip.

State the date range and any match gaps whenever you use the data.

## 4. Next

Run the `performance-review` skill to turn the data into findings, or re-run `report` so the
report gains its **Performance** tab and the Pages table gains sessions and search clicks.
