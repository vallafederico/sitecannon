---
name: content-audit
description: Audit a website's content and page anatomy with SiteCannon — templates, section types, topics, thin and duplicate content — and turn it into a content model and component list for a rebuild. Use when the user asks for a content audit or inventory, content model, component list, "what templates do we need", page anatomy, design-system scoping, or what to keep, merge or cut.
---

# Content audit

Output: a content inventory summary, a draft content model (page types → sections/fields), a
component list, and keep/merge/cut recommendations. Tools: the `sitecannon` MCP server or
`sitecannon tool <name> '<json>'`. Crawl first if needed (step 1 of the `site-audit` skill).

## 1. Inventory

- `templates`: every layout shared by 2+ pages, with URL shapes, median words and typical section order; the one-off pages and their sections; and `section_inventory` (how often each section type appears).
- `topics`: what the content is about and where it lives.
- `issues` with `{"id":"thin_content"}`, `{"id":"near_duplicates"}`, `{"id":"duplicate_titles"}`, `{"id":"missing_h1"}`.

## 2. Page types and content model

For each significant template, check 2–3 real pages with `page` (its `sections` show the order,
headings and word counts). Then describe the page type:

| Page type | URL shape | Pages | Sections (in order) | Fields the CMS needs | Notes |
|---|---|---|---|---|---|

Fields are inferred from the sections and headings, e.g. hero → title, intro, CTA; article → body,
author, date, related. Mark inferences as such. One-off pages: group the similar ones (e.g. several
service landing pages) and propose a shared, flexible template where it makes sense.

## 3. Component list

From `section_inventory` and the typical sections, list the components the rebuild needs (hero
variants, feature grid, testimonial, logo strip, pricing table, FAQ, CTA band, listing/cards,
article body…). Give each how many page types use it and an example URL. That's the starting point
for design-system scoping and estimates.

## 4. Keep, merge, cut

If usage data is loaded (`analytics_status`), base these calls on it: `performance` gives sessions,
engagement and key events per template and topic, plus `no_traffic`, `poor_engagement` and
`hidden_winners` lists. A thin page with steady traffic is a keep-and-improve, not a cut.

- **Thin pages** (under 150 words of main content): enrich, merge into a parent, or noindex. Say which per group.
- **Near-duplicates and duplicate titles:** consolidate, or differentiate.
- **Topics with many small pages:** candidates to merge into hubs.
- Never recommend cutting a page with high internal equity (`pagerank` > 1, many inlinks) without flagging the redirect it needs.

Section types and topics are heuristic; spot-check before stating anything as fact. Without
usage data, "low value" means low internal equity and little content, not low visits. Say so.

## 5. Deliver

Chat summary plus the tables. For a client artefact, embed them via `report` with `notes`.
