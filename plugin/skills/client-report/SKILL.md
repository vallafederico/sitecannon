---
name: client-report
description: Turn SiteCannon findings into a polished client- or product-facing report — a concise strategy brief embedded in the single-file HTML report, written for decision-makers. Use when the user asks for a client report, pitch document, findings summary, executive summary, product report, or to "write this up" after analysing a site.
---

# Client / product report

The deliverable is the SiteCannon HTML report: one offline file with interactive sitemaps, the link
graph, content anatomy, templates and issues. It opens with your strategy brief. Your job is the
brief: the story the data supports, in language a client understands.

Tools: the `sitecannon` MCP server or `sitecannon tool <name> '<json>'`. If there's no analysis yet,
run the `site-audit` skill first. It gathers the evidence, and this skill shapes it for an audience.

## Know the audience

Ask, or infer from context:
- **Prospect / pitch:** what we noticed, why it matters for their goals, and what we'd do. Three strong findings beat ten weak ones. No blame.
- **Client discovery / kickoff:** a fuller baseline, scope implications (templates, components, migration risk) and the open questions.
- **Internal product team:** more technical, link to issue checks, and include the SQL or tool calls behind the numbers.

## Structure

```markdown
# <Site>: <purpose, e.g. "Rebuild discovery">
*Based on a crawl of <N> pages on <date>. <caveat in plain words, if any>.*

## Summary
2–4 sentences: the situation, the biggest opportunity, the recommended direction.

## What's working (when usage data is loaded)
2–4 bullets from the `performance-review` skill: what earns visits and conversions, with numbers.

## What we found
3–6 findings, most important first. Each one:
**Plain-language headline.** One or two sentences of evidence (numbers, an example).
*Why it matters:* the business or user consequence. *What we'd do:* the recommendation.

## What this means for the rebuild
Scope in their terms: page types to design, components, content to move/merge/retire, URL changes,
risks to manage.

## Next steps
3–5 concrete actions, with owners where known.
```

## Writing rules

- Lead with the conclusion; supporting numbers follow.
- Translate jargon: "pages Google sees without a title" rather than "missing `<title>` in SSR HTML". Keep one technical term where the client's developers will need it.
- Quantify ("1,868 of 1,877 listing pages") and give one example URL.
- Be honest about limits (capped crawl, no analytics, heuristic topics), stated once, briefly.
- Don't pad with generic best practice, and don't exaggerate. If something is minor, leave it out or label it minor.
- ~400–700 words. Use bold sparingly. Tables only where they compare things.

## Produce it

1. Draft the brief and show it to the user if they want to review it first.
2. Call `report` with `notes` set to the markdown (or `notes_file` with a path).
3. Return the file path and suggest which tabs to walk the client through: Overview → Performance
   (if usage data is loaded) → Structure (visual sitemap) → Content (page anatomy) → Issues.
