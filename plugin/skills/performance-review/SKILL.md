---
name: performance-review
description: Find what works and what doesn't on a website by combining real usage data (GA4, Search Console) with SiteCannon's crawl — which templates, topics, sections and pages earn visits, engagement and conversions, where the opportunities are, and what to keep, fix or drop in a rebuild. Use when the user asks what's working, what isn't, which pages perform, where traffic or conversions come from, what to prioritise, or wants data-driven recommendations.
---

# Performance review: what works, what doesn't

Output: a short, evidence-based list of what works (keep and amplify), what doesn't (fix or drop),
and the specific opportunities. It goes in chat and, optionally, in the report brief.
Tools: the `sitecannon` MCP server or `sitecannon tool <name> '<json>'`.

## 0. Preconditions

`analytics_status`. If no usage data is loaded, run the `analytics-setup` skill first. Note the date
range, the sources (GA4, Search Console or both) and `matched.share_matched`.

## 1. Read the breakdown

Call `performance`. Work through it in this order:

1. **Totals:** sessions, engagement rate, key events per session, organic share, search clicks. These set the scale for everything else.
2. **By template:** compare per page (`sessions_per_page`) and by rate (`engagement_rate`, `key_events_per_session`), not by totals; big templates always win on totals. Which page types pull their weight? Which have many pages but few visits (`pages_with_sessions` far below `pages`)?
3. **By topic:** which subjects draw visits and convert, and which are written about a lot but read little?
4. **By click depth:** how fast visits fall with depth. A steep drop plus valuable deep pages means the structure is hiding content.
5. **By section type** (with vs without): hints for which blocks to keep in the new templates. Only mention differences that are large and backed by enough pages, and call them correlations.

## 2. Work the opportunities

Each list in `performance.opportunities` maps to an action:

| List | What it usually means | Typical action |
|---|---|---|
| `top_converters` | Pages that make the money | Protect: keep URL, content and conversion path; Tier 1 in any migration |
| `unmatched_traffic` | Old URLs still visited, broken URLs, pages beyond the crawl | Keep or fix redirects; never drop these in a migration |
| `low_ctr` | Ranks on page one but snippets don't get clicks | Rewrite title and meta description |
| `striking_distance` | Queries at positions 4–20 with real impressions | Improve the page and its internal links for that query |
| `hidden_winners` | Earn traffic despite being deep or weakly linked | Promote in nav, hubs and related links |
| `poor_engagement` | Visitors arrive and leave | Check intent match, above-the-fold content, speed |
| `over_promoted` | Lots of internal links, few visits | Rethink their prominence, or improve them |
| `no_traffic` | Indexable pages nobody visits | Merge, improve or retire, after checking they aren't new or seasonal |

Before you state a finding, verify one or two examples with `page` (and `query` on
`page_performance` if needed). Combine signals for stronger conclusions, e.g. a template with low
sessions per page *and* thin content (`main_word_count`) *and* deep placement.

## 3. Write it up

```markdown
## What works
- <page type / topic / page>: <metric vs site average>, e.g. "comparison pages get 153 sessions per page (site avg 101) and convert at 3×"
## What doesn't
- …
## Opportunities (in priority order)
1. <action> — <evidence> — <expected effect>
## Implications for the rebuild
Templates and sections to keep, content to merge or retire, URLs to protect (link to the seo-migration skill).
```

Rules:
- Every claim gets a number and a comparison, against the site average or another group.
- Always state the date range, the sources, and coverage gaps (`share_matched`, capped crawl).
- Say "associated with", not "causes", for section and topic comparisons.
- Analytics undercounts where consent banners block tracking; compare ratios between pages, not absolute totals against other tools.
- Search Console positions are averages; ranks for one query fluctuate.

For a client artefact, pass the write-up to the `client-report` skill (it embeds it in the report;
the report's Performance tab carries the supporting tables). For migrations, feed `top_converters`,
`unmatched_traffic` and high-traffic pages into the `seo-migration` tiers.
