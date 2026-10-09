# SiteCannon — Research & Proposed Architecture

> Toolkit for analysing a prospect/client website before a rebuild: crawl → model → analyse → plan migration → present.
> Research date: 2026-10-09. Versions/prices verified against live sources on that date unless flagged *(unverified)*.

---

## 0. Decisions & revised direction (2026-10-09)

- **Core = a Rust CLI** (`sitecannon`) that studies the current site only. Single binary, fast, distributable.
- **TypeScript** for the report app only. Its build output is embedded in the binary, so `sitecannon report` writes one HTML file.
- **No Screaming Frog.** **No third-party SEO data** in core: everything comes from what's on the site.
- **Paid/external features become add-ons later** (LLM, GSC/GA4, DataForSEO, Wayback, redirect mapping, FigJam export…).

### Why Rust, and the honest caveat
On a single site, crawl speed is capped by **politeness** (we'd run ~2–10 req/s against a client's server), not by the language. Rust's real wins are:
- **parsing and analysis at 10k–100k pages** with low memory
- **one static binary** with no Python/Node runtime to install
- a clean CLI that Claude skills and the Agent SDK can call

### Rust core stack (to verify crate versions at scaffold time)
| Concern | Crate |
|---|---|
| Crawl (HTTP-first, robots, sitemaps, concurrency, budgets) | **Own crawler on `reqwest` + `tokio`** (built). spider-rs was rejected: it only exposes a redirect's final destination, not each hop, and it's a sprawling crate. Headless rendering is planned via `chromiumoxide` (what spider uses under the hood) |
| HTML parsing / selectors | `scraper` (DOM queries); `lol_html` for streaming rewrites if needed |
| Main content → markdown | `<main>` element → `htmd`; else `dom_smoothie` (Readability port) → `htmd`; full-page fallback (built) |
| URL normalisation | `url` + custom canonicaliser (param allowlist) |
| Structured data | `serde_json` on `<script type="application/ld+json">` |
| Template / near-dup detection | DOM-skeleton shingles → MinHash/SimHash (small custom code or a minhash crate) |
| Link graph metrics (depth, PageRank, degree, betweenness, communities) | `petgraph` + `rustworkx-core` |
| Storage | **DuckDB** (`duckdb` crate, bundled) + Parquet export |
| CLI / progress | `clap`, `indicatif`, `tokio` |
| Local embeddings (add-on, no paid API) | `fastembed` (ONNX) |
| Report | TS/Vite single-file template, embedded with `include_str!`, data injected as JSON |

### Core vs add-ons
| **Core (on-site only, free)** | **Add-ons (later)** |
|---|---|
| Crawl + robots + XML sitemaps + JS rendering | LLM page classification / ROT / IA critique / exec summary (Claude) |
| Per-page on-site SEO: status, redirects, canonicals, hreflang, meta robots, titles/meta/H1–H6, schema types, images/alt, word count | GSC / GA4 / server-log value data |
| Links with anchor + position (nav/footer/main/breadcrumb) | Backlinks / est. traffic (DataForSEO etc.) |
| Three trees: URL / nav / click-depth + mismatches | Wayback / Common Crawl legacy URLs |
| Link graph: depth, internal PageRank, orphans (sitemap-only), dead-ends, hubs | Embeddings + topic map + cannibalisation |
| Templates + component inventory, near-duplicates, thin content | Redirect mapping old→new + validation |
| Content inventory (markdown per page), screenshots (opt-in) | FigJam / Octopus / Slickplan export |
| **On-site value proxy** (internal PageRank, depth, inlinks, nav presence, content depth, schema) | Post-launch monitoring (Agent SDK, scheduled) |
| Single-file HTML report + CSV/XLSX/Parquet export | |

### CLI shape (draft)
```
sitecannon crawl <url> [--render auto|never|always] [--max-pages N] [--rps N] [--out ./project]
sitecannon analyze  ./project            # graph, trees, templates, dupes, issues, value proxy
sitecannon report   ./project [--client] # → report.html
sitecannon export   ./project --format csv|xlsx|parquet|json
sitecannon query    ./project "SELECT …" # read-only SQL (what Claude skills use)
```

---

## 1. Original research: full picture (pre-decisions; Python stack superseded by §0)

```
                ┌──────────────────────── DISCOVERY ────────────────────────┐
  URL ─────────►│ robots.txt · XML sitemaps · link crawl · Wayback CDX ·    │
                │ Common Crawl · WP REST API · (client) GSC/GA4 · backlinks │
                └───────────────┬───────────────────────────────────────────┘
                                ▼
   sc-crawl  (Crawlee Python, adaptive HTTP/Playwright)  ──►  Parquet + DuckDB  ◄── sc-enrich (GSC, GA4,
   + trafilatura/Defuddle markdown, screenshots, links w/ position)        │          DataForSEO/Ahrefs)
   + optional Screaming Frog CLI pass (audit-grade SEO checks)             │
                                                                           ▼
   DETERMINISTIC ANALYSIS (code)                    LLM ANALYSIS (Claude)
   sc-graph     depth, PageRank, orphans, hubs      Haiku 5.5 (Batch)  page type / audience / funnel / topics
   sc-template  DOM-skeleton MinHash clusters       Sonnet 5.5         per-template analysis, ROT judgement,
   sc-dedupe    near-duplicate content                                 redirect adjudication
   sc-embed     embeddings → Lance                  Opus 5.5           IA critique, proposed sitemap,
   sc-score     URL value score                                        exec summary
   sc-redirects exact → fuzzy → semantic candidates
                                ▼
   sc-report → single-file interactive HTML (internal + --client build) · XLSX/CSV · PNG/PDF · FigJam · Octopus XML
```

Rule of thumb: **code does everything measurable; LLMs do judgement and narrative, working from aggregates (SQL), never raw HTML dumps.**

---

## 2. Layer 1 — Crawling & extraction

### Recommendation
Build our own **Python crawler on Crawlee for Python** (v1.10.x), with **trafilatura** for content and an **optional Screaming Frog CLI** pass for audit-grade SEO checks. Join both on normalised URL.

| Tool | Role | Why |
|---|---|---|
| **Crawlee Python** `AdaptivePlaywrightCrawler` | Primary crawler | HTTP-first, auto-falls back to browser only when rendering changes the page; persistent queues (resumable), autoscaling, sessions/proxies, screenshots |
| **trafilatura 2.3** | Main-content → markdown | Best open-source extractor in benchmarks; also has sitemap/feed helpers |
| **Defuddle** (npm) | Fallback extractor | Better on component-heavy marketing pages |
| **Screaming Frog 24.x CLI** (`--headless`) | Optional SEO reference crawl | hreflang/canonical/structured-data validation, crawl compare, now ships an MCP server. £199/user/yr. *Check licence terms for shared/server use.* |
| Scrapy + **advertools** | Reference only | advertools' SEO field set is a good checklist |
| crawl4ai | Library only, not backbone | Good markdown, but a run of RCE/SSRF CVEs in 0.8–0.9 |
| Firecrawl API / Cloudflare `/crawl` | Fallback for hard/blocked pages | Self-hosted Firecrawl lacks anti-bot; little SEO depth |
| Sitebulb | Skip for automation | No CLI |

**Important:** store **both** main-content markdown **and** full-page markdown. Extractors strip heroes, feature grids and CTAs, which is the content a rebuild must migrate.

### Discovery beyond link-following
- `robots.txt` (protego) → `Sitemap:` lines + disallow rules
- XML sitemaps (recursive index, keep `lastmod`, hreflang) → orphans = in sitemap but not linked
- **Wayback CDX**: `web.archive.org/cdx/search/cdx?url=domain&matchType=domain&collapse=urlkey&filter=mimetype:text/html&output=json` → legacy URLs that may hold backlinks
- **Common Crawl** index (`CC-MAIN-2026-39`, query last 3–4)
- **WordPress**: `/wp-json/wp/v2/{posts,pages,types}`, often the whole inventory without crawling
- Client data: GSC pages, GA4 landing pages, backlink targets

### Template / component detection
- DOM skeleton (tag + class tokens, text stripped) → shingles → **MinHash/LSH** (`datasketch`) → clusters = templates
- Section signatures (`main > section` structure hashes) → repeated hashes = **component inventory** (gold for rebuild scoping)
- Platform hints: WP `page-template-*` body classes, Webflow `w-*`/`data-wf-page`, Framer `framer-*` (hashed classes, so cluster on structure)

### Gotchas to handle
- **SPA/JS**: record raw vs rendered (links/word-count delta). JS-only links are themselves a finding
- **Facets/params**: canonicalise (w3lib), param allowlist, cap URLs per pattern, report params rather than crawl them. Webflow `?<hash>_page=` traps
- **Politeness**: 1–2 req/s per host, honour `Retry-After`, identifiable UA, obey robots for prospects
- **Cloudflare/bot protection**: ask clients to allowlist our IP/UA. For prospects, fall back to Firecrawl/CF crawl and **record "blocked" as a finding**. Don't build evasion.
- **Auth/staging**: Playwright `storageState`, basic auth, never follow logout/delete links

### Per-page record (schema sketch)
| Group | Fields |
|---|---|
| Identity | `url`, `url_normalized`, `discovered_via[]`, `depth`, `crawl_id`, `fetched_at` |
| HTTP | `status`, `redirect_chain[]`, `final_url`, `content_type`, `bytes`, `ttfb_ms`, `x_robots_tag`, `platform_fingerprint` |
| Indexability | `meta_robots`, `canonical`, `canonical_self`, `robots_allowed`, `in_sitemap`, `sitemap_lastmod`, `hreflang[]`, `indexable` |
| On-page | `title`, `meta_description`, `h1[]`, `headings_outline[]`, `lang`, `og{}`, `word_count` |
| Structured data | `jsonld[]`, `schema_types[]` (extruct) |
| Links | `links_out[{href, internal, anchor, rel, position: nav/header/footer/main/aside/breadcrumb, is_js_only}]` |
| Content | `main_markdown`, `full_markdown`, `simhash`, `images[{src,alt}]`, `forms[]`, `embeds[]`, `downloads[]` |
| Template | `dom_skeleton_hash`, `minhash_sig`, `section_signatures[]`, `template_cluster_id` |
| Rendering | `rendered`, `render_diff{}`, `console_errors[]`, `screenshot_desktop/mobile`, `third_party_domains[]` |

---

## 3. Layer 2 — Data store

- **DuckDB** single file per project, with raw HTML/markdown in **Parquet**, and views on top
- Vectors: **Lance via DuckDB `lance` extension** (vector + FTS + hybrid search, same engine). Alt: sqlite-vec. Avoid DuckDB `vss` (experimental persistence)
- Embeddings: Anthropic has none, so use **Voyage `voyage-4-lite`** (or a local model for zero-cost)
- Agents access data **read-only via `duckdb -readonly -json` in Bash**, enforced by a PreToolUse hook. A `site-model` skill holds the schema + SQL cookbook. No MCP needed for our own data.

---

## 4. Layer 3 — Analysis

### Deterministic (code)
- **Graph**: click depth (BFS from home), in/out-degree, internal PageRank ("link equity"), betweenness (bridge pages), orphans, dead-ends, Louvain/Leiden communities vs URL sections
- **Three structures of one site** (key insight for clients):
  1. **URL-path tree**, how the CMS stores it
  2. **Navigation tree** (menus/footer/breadcrumbs), the IA the site *claims*
  3. **Click-depth tree**, the IA users and crawlers *experience*

  The mismatches between them are the headline finding.
- Templates, near-dupes, thin content, staleness, status/canonical/redirect issues, schema coverage

### LLM (Claude), where it actually adds value
| Task | Model | Mode |
|---|---|---|
| Page type, audience, funnel stage, topics/entities | **Haiku 5.5** (`claude-haiku-5-5`) | Batch API + structured output |
| Per-template analysis, ROT judgement on rule-flagged ambiguous pages | Sonnet 5.5 | subagents |
| Redirect adjudication (low-confidence pairs only) | Sonnet 5.5 | subagent |
| IA critique, proposed new sitemap | Opus 5.5 | from SQL aggregates |
| Executive summary / client narrative | Opus 5.5 | |

### Cost control
- **Batch API** (−50%) + **prompt caching** (taxonomy + instructions + few-shots in cached prefix, 1h TTL)
- Send title, H1–H3, meta, URL and the first ~1.5k tokens of main text, **not HTML**
- Above ~5k pages: classify 20–50 per template, propagate labels via template + embedding neighbours, LLM only for outliers
- Structured outputs with enums, short `max_tokens`, cache by `content_hash` so re-crawls only pay for changed pages
- Ballpark: **10k pages ≈ $2 classification on Haiku 5.5 Batch** + a few $ of Opus synthesis. Data APIs for a prospect audit ≈ $1–5 (DataForSEO).

> Haiku 5.5 launched ~days ago: validate against a hand-labelled sample (~200 pages) before trusting it.

---

## 5. Layer 4 — SEO-safe migration

### Data sources
| Source | Gives us | Access / limits |
|---|---|---|
| GSC Search Analytics API | clicks/impr/position per page×query, 16 mo | 50k rows/day/site/type. Export early, history is lost after 16 mo |
| GSC URL Inspection API | index status, Google-chosen canonical | **2,000/day/site**, so top-value URLs only |
| GSC gen-AI performance report (Jun 2026) | AI Overviews / AI Mode impressions | Rolling out. API availability *(unverified)* |
| GA4 Data API | sessions/conversions/revenue by landing page | 200k tokens/day standard |
| **DataForSEO** | Prospect-stage estimates: ranked keywords, est. traffic per URL, backlinks | Pay-as-you-go, $50 min deposit; Backlinks $100/mo min |
| Ahrefs API v3 / Semrush API | backlinks / keywords | Ahrefs from $129/mo; Semrush needs Business plan *(prices from third-party write-ups)* |
| Bing Webmaster AI Performance | Copilot citations | CSV only |

**Prospect stage (no access):** our crawl + Wayback + DataForSEO, with all numbers labelled *estimated*. **Client stage:** swap in GSC/GA4/backlinks.

### URL value score (starting proposal; tune on past migrations)
Percentile-rank each metric within the site after `log1p`:
```
Value = 0.25·Clicks + 0.10·Impressions + 0.20·RefDomains(quality-weighted)
      + 0.20·Conversions + 0.10·InternalPageRank
      + 0.05·AI_visibility + 0.10·Quality/Trend
```
**Override:** any URL with any conversions, or a quality backlink (≥1 RD ≥ DR30, or ≥3 RDs), is never "remove". It gets at least a relevant redirect.
Prospect mode: est. traffic replaces clicks, conversions are dropped, and the remaining weights are rescaled.

| Value | Topical match in new IA | Action |
|---|---|---|
| High | yes | **Keep** (1:1) |
| High | no | **Recreate / merge**, flag to client |
| Medium | overlaps another page | **Merge** → 301 to stronger |
| Low | has links | **Redirect** to closest relevant parent |
| Low | nothing | **Remove** (410/404, *never* a homepage redirect) |

### Redirect-mapping pipeline
1. Normalise (case, tracking params, slashes, protocol), dedupe on canonical
2. **Rules**: exact path + regex patterns (`/blog/YYYY/MM/slug → /blog/slug`), conf 1.0
3. **Fuzzy**: rapidfuzz/PolyFuzz on slugs
4. **Semantic**: embed main content (old pages that are going away: use Wayback copy), top-3 cosine
5. **LLM adjudication** below threshold: Claude picks from top-5 or says "no equivalent → suggested parent". Homepage is never allowed as a fallback (Google treats irrelevant redirects as soft-404s)
6. `conf = max(rule, 0.25·fuzzy + 0.45·semantic + 0.30·llm_agree)`, penalised on page-type mismatch. ≥0.9 auto, 0.7–0.9 quick review, <0.7 manual, high-value unmatched → content team
7. Review sheet sorted by value desc
8. Export: nginx / Vercel / Netlify `_redirects` / Cloudflare bulk. Validate no loops/chains/self-refs

### Launch & post-launch (scriptable)
- Pre-launch: snapshot crawl + 16 mo GSC + GA4 + backlinks + AI visibility; test redirect map against staging
- **Redirect test**: every old URL → exactly one 301/308 hop → 200, indexable, self-canonical. Report % landing on homepage
- **Crawl diff** old vs new: titles, H1, meta, canonicals, word count, inlinks, schema types, hreflang
- Monitor daily ×2 wks then weekly ×3 mo: 404s, URL Inspection on top ~2k URLs, GSC clicks per old→new pair (alert >30% WoW drop)
- Keep redirects ≥1 year (Google)

### AI search (2026)
- Google's AI optimisation guide (May 2026): **no llms.txt, no special schema or chunking needed**; just indexable + snippet-eligible. llms.txt is optional/low priority.
- Migration checks that matter: main content present in **server HTML** (not JS-only), snippet controls preserved, robots.txt not accidentally blocking AI crawlers (client decision), structured data carried over 1:1, AI-cited pages weighted up in value score.

---

## 6. Layer 5 — Visualisation & presentation

### Recommendation
One **Vite app → single self-contained HTML file** (`vite-plugin-singlefile`), data inlined. Clients double-click it; nothing to host. Same codebase builds an **internal** version (raw tables, debug) and a **`--client`** version (branded, curated).

| View | Library |
|---|---|
| Sitemap trees (URL / nav / click-depth), icicle, sunburst, treemap | **d3-hierarchy** |
| Editable, card-style "Octopus-like" sitemap (thumbnails, template badges, status colour) | **React Flow (xyflow v12) + elkjs** (`mrtree`/`layered`) |
| Internal link graph, 1k–50k nodes | **sigma.js v3 + graphology** (WebGL; Louvain, PageRank, ForceAtlas2 built in). Bake layout positions at build time so it opens instantly |
| "Wow" overview, 100k+ nodes | cosmos.gl v3 (optional) |
| Analyst deep-dive | export GEXF → **Gephi Lite** |
| Topic map (embeddings → UMAP → HDBSCAN/BERTopic) | **datamapplot** (self-contained HTML) |
| Charts: template mix, depth histogram, freshness heatmap | ECharts or Observable Plot |
| Old → new migration flow | **Sankey** (ECharts / d3-sankey), with a "retired" sink |

### Export targets
- **FigJam**: Figma MCP `generate_diagram` turns Mermaid into native FigJam (proposed sitemap, migration flow), good for workshops
- **Octopus.do**: "Octopus XML" project import (Mar 2026) supports sections, colours, blocks, SEO data
- **Slickplan**: XML import
- **Slides**: Playwright screenshots of named report views → PNG/PDF
- **XLSX/CSV**: inventory, value scores, redirect map

### Report structure
1. **Executive summary**: pages, templates, orphans, median/p90 depth, top 5 issues & opportunities
2. **Inventory**: filterable table (status, indexability, template, value)
3. **Structure**: URL tree · Nav tree · Click-depth tree + mismatch list
4. **Link graph**: sigma map (colour by community/section/template), PageRank leaderboard, orphans, dead-ends
5. **Content landscape**: type/template treemap, freshness heatmap, depth histogram, ROT breakdown
6. **Topic map**: clusters, cannibalisation, gaps
7. **Templates & components**: inventory with screenshots, which helps rebuild scoping & estimation
8. **Proposed IA**: editable sitemap → FigJam / Octopus
9. **Migration**: Sankey + redirect map + risk register (high-value URLs without a home)
10. **Appendix**: method, crawl settings, data sources, "estimated" caveats

---

## 7. Layer 6 — Agentic packaging (Claude Code)

Package everything as a **Claude Code plugin** (`sitecannon`) in a private marketplace repo. Teammates: `claude plugin marketplace add <org>/<repo>` → `claude plugin install sitecannon@<marketplace>`.

```
sitecannon/
├── .claude-plugin/plugin.json
├── bin/                     # on PATH when plugin enabled → deterministic CLIs
│   sc-crawl  sc-enrich  sc-graph  sc-template  sc-dedupe  sc-embed
│   sc-classify  sc-score  sc-redirects  sc-validate  sc-report
├── skills/
│   ├── site-model/          # reference: DB schema + SQL cookbook (preloaded into every agent)
│   ├── crawl/               # /crawl <url> [--prospect|--client]
│   ├── classify-pages/
│   ├── content-audit/       # ROT
│   ├── ia-review/
│   ├── propose-sitemap/
│   ├── redirect-map/
│   ├── migration-validate/  # post-launch checks
│   ├── client-report/
│   └── audit/               # orchestrator (dynamic workflow: fan out per template, then synthesise)
├── agents/
│   ├── template-analyst.md      (sonnet, Bash(duckdb) read-only)
│   ├── content-auditor.md       (sonnet)
│   ├── redirect-adjudicator.md  (sonnet)
│   ├── ia-architect.md          (opus)
│   └── report-writer.md         (opus)
├── hooks/hooks.json         # SessionStart: venv setup · PreToolUse: host allowlist + read-only SQL
└── report-app/              # Vite single-file report
```

- **Skills are the slash commands now.** Use `disable-model-invocation` for side-effecting ones (crawl, API spend).
- **Bulk per-page classification bypasses the agent loop:** plain script → Messages Batches API.
- **Headless/scheduled runs:** Claude Agent SDK (Python) loads the plugin from a local path and runs `/sitecannon:audit`, with `output_format` json_schema for structured results. Good for "re-crawl client weekly post-launch".
- Per-project working dir: `projects/<client>/{crawl.duckdb, parquet/, screenshots/, exports/, report.html}`

### Open-source to borrow from
- **claude-seo** (AgricIDaniel): plugin structure, orchestrator + parallel agents
- **advertools**: SEO crawl field set
- **SEOmator seo-audit**: 251 deterministic rules to mine
- **Screaming Frog embeddings redirect-mapping tutorial**: matching method

---

## 8. Two operating modes

| | **Prospect** (pitch, no access) | **Client** (engaged) |
|---|---|---|
| Crawl | polite, robots-obeying, capped (~2–5k URLs) | full, allowlisted in WAF, staging access |
| SEO data | DataForSEO estimates + Wayback | GSC, GA4, backlinks, logs |
| LLM depth | sample-based | full classification + ROT |
| Output | short pitch deck: "here's what we see, here's the risk, here's the opportunity" | full report + redirect map + proposed IA + post-launch monitoring |
| Cost target | < $10 | < $50 (excl. tool subscriptions) |

---

## 9. Proposed build phases

1. **Foundation**: `sc-crawl` (Crawlee + trafilatura + discovery sources) → DuckDB/Parquet; `site-model` skill. *Milestone: crawl a real site, query it.*
2. **Structure**: `sc-graph`, `sc-template`, three-tree builder; first report-app views (trees + sigma graph + inventory). *Milestone: a clickable visual sitemap & link graph.*
3. **Content & IA**: `sc-embed`, `sc-classify` (Batch), topic map, ROT audit, `ia-review` + `propose-sitemap`. *Milestone: content landscape + proposed IA → FigJam.*
4. **Migration**: `sc-enrich` (GSC/GA4/DataForSEO), `sc-score`, `sc-redirects` + adjudicator, `sc-validate`. *Milestone: value-ranked redirect map.*
5. **Packaging**: plugin + marketplace, `/audit` orchestrator, `--client` branded build, Agent SDK scheduled monitoring.

---

## 10. Open questions / decisions for us

1. **Language**: Python-first (Crawlee/trafilatura/networkx/DuckDB) with a TS report app? (recommended) Or all-TS?
2. **Screaming Frog**: do we have licences, and is headless/server use within terms?
3. **Data vendor**: DataForSEO (cheap, PAYG) vs existing Ahrefs/Semrush subscription?
4. **Embeddings**: Voyage API vs local model (privacy for client content)?
5. **Sitemap tool of record**: FigJam, Octopus, Slickplan, or our own React Flow editor?
6. **Target platforms we usually rebuild into** (Webflow/Framer/Next/Astro)? This shapes redirect export formats and template→component mapping.
7. **Client-data handling**: where crawls/GSC exports live, retention policy.

---

## Sources (selected)
- Crawling: [Crawlee adaptive crawler](https://crawlee.dev/python/docs/guides/adaptive-playwright-crawler) · [crawl4ai releases](https://github.com/unclecode/crawl4ai/releases) · [Firecrawl self-host](https://github.com/firecrawl/firecrawl/blob/main/SELF_HOST.md) · [Screaming Frog CLI](https://www.screamingfrog.co.uk/guides/command-line) · [SF release history](https://www.screamingfrog.co.uk/seo-spider/release-history/) · [trafilatura](https://pypi.org/project/trafilatura/) · [Defuddle](https://npmjs.com/package/defuddle) · [Wayback CDX](https://github.com/internetarchive/wayback/blob/master/wayback-cdx-server/README.md) · [Common Crawl index](https://index.commoncrawl.org/) · [Cloudflare /crawl](https://developers.cloudflare.com/changelog/2026-03-10-br-crawl-endpoint)
- SEO migration: [Google site moves](https://developers.google.com/search/docs/crawling-indexing/site-move-with-url-changes) · [GSC API limits](https://developers.google.com/webmaster-tools/limits) · [GA4 quotas](https://developers.google.com/analytics/devguides/reporting/data/v1/quotas) · [Google AI optimisation guide](https://developers.google.com/search/docs/fundamentals/ai-optimization-guide) · [GSC gen-AI reports](https://developers.google.com/search/blog/2026/06/gen-ai-performance-reports) · [Irrelevant redirects = soft 404](https://gsqi.com/marketing-blog/redirects-less-relevant-pages-soft-404s) · [SF embeddings redirect mapping](https://www.screamingfrog.co.uk/seo-spider/tutorials/how-to-use-vector-embeddings-for-redirect-mapping/) · [DataForSEO Labs pricing](https://dataforseo.com/pricing/dataforseo-labs/dataforseo-google-api) · [SEL migration checklist](https://searchengineland.com/website-migration-checklist-383201)
- Visualisation: [sigma](https://npmjs.com/package/sigma) · [graphology std-lib](https://graphology.github.io/standard-library/) · [cosmos.gl v3](https://openjsf.org/blog/cosmos-gl-v3) · [React Flow + ELK](https://reactflow.dev/examples/layout/elkjs) · [Gephi Lite](https://lite.gephi.org) · [Octopus XML import](https://octopus.do/sitemap/changelog/project-import-introducing-octopus-xml) · [Slickplan XML](https://help.slickplan.com/articles/slickplan-xml-import) · [vite-plugin-singlefile](https://unpkg.com/vite-plugin-singlefile@2.3.3/README.md)
- Agentic: [Skills](https://code.claude.com/docs/en/skills) · [Subagents](https://code.claude.com/docs/en/sub-agents) · [Hooks](https://code.claude.com/docs/en/hooks) · [Plugins](https://code.claude.com/docs/en/plugins) · [Marketplaces](https://code.claude.com/docs/en/plugins/create-marketplace) · [Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) · [Models](https://platform.claude.com/docs/en/about-claude/models/overview) · [Batch](https://platform.claude.com/docs/en/build-with-claude/batch-processing) · [DuckDB + Lance](https://duckdb.org/2026/05/21/test-driving-lance) · [claude-seo](https://github.com/AgricIDaniel/claude-seo)
