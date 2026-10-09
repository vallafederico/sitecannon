---
name: ia-review
description: Review a website's information architecture with SiteCannon data and propose a new sitemap — compare how URLs, menus and click paths organise the site, check topics against sections, and recommend a target structure. Use when the user asks about IA, site structure, navigation, sitemap, menus, taxonomy, "how should the site be organised", or wants a proposed sitemap for a rebuild.
---

# Information architecture review

Output: a diagnosis of the current IA, a proposed sitemap with rationale, and (optionally) the brief
embedded in the HTML report. Tools: the `sitecannon` MCP server or `sitecannon tool <name> '<json>'`.
If the site isn't crawled yet, follow step 1 of the `site-audit` skill first.

## 1. Read the three structures

- `summary`: sections, navigation, depth, topics and caveats.
- `sitemap` `{"kind":"url","depth":2}`: how the CMS organises content.
- `sitemap` `{"kind":"nav","depth":3}`: what the site promises visitors (the persistent header and footer).
- `sitemap` `{"kind":"click","depth":3}`: how people and crawlers actually reach things.

## 2. Diagnose

Look for and quantify:
- **Navigation gaps:** large URL sections that the header/footer never link to, or menu items that point to the wrong place. Duplicate or placeholder targets inside menus are common.
- **Folders without landing pages:** `issues` `{"id":"folders_without_page"}`.
- **Depth:** content more than 3 clicks deep, orphans, and sections reachable only through pagination or related-post links.
- **Taxonomy:** `topics`. Does each topic live in one section, or is it scattered? Do sections mix unrelated topics? Are there parallel taxonomies (e.g. `/topic/type/` and `/type/`) competing for the same content?
- **Templates vs sections:** `templates`. Do URL shapes and layouts line up, or does one layout serve unrelated purposes?
- **Label quality:** menu labels, and whether anchors match the target pages' titles.

Verify each finding with an example (`page` or `query`) before you state it.

## 3. Propose the target sitemap

- Base it on user tasks and the content that exists (topics, templates and the value of top pages), not on the current menus.
- Keep high-equity pages (top `pagerank`/`inlinks`, see `summary.top_pages`) at shallow depth. Flag any proposed URL change on them as a migration risk.
- Show it as a nested markdown list, with page counts where they're known. Add a Mermaid `flowchart TD` if the user wants a diagram; keep it to 2–3 levels.
- For each top-level item: what it contains (existing sections and topics), what changes, and why.
- List explicitly: merges, removals, new landing pages, and URL changes that will need redirects.

## 4. Deliver

Chat: the diagnosis (5–8 bullets) and the proposed sitemap. If the user wants a client-facing
artefact, embed both via `report` with `notes`. Use the brief structure from the `client-report` skill.
Point them to the report's Structure tab, which has a visual card sitemap with SVG export, for the
current-state diagrams.
