# SiteCannon

Study a website before you rebuild it. SiteCannon crawls a site, analyses its structure, content
and SEO, and produces a single-file HTML report you can hand to a client. It ships as one Rust
binary with a CLI, an **MCP server** and an **HTTP API** for agents, plus **Claude Code skills**
for running audits end to end.

- **Crawl:** a polite crawler (robots.txt, XML sitemaps, rate limits, stops if the site starts refusing). It records full redirect chains, on-page SEO data, links with their position on the page, and markdown of each page.
- **Analyse:**
  - click depth and internal PageRank
  - three sitemaps (URL folders, menus, click paths)
  - templates and one-off pages
  - page sections typed as hero, features, FAQ and so on
  - topics found from the page text
  - near-duplicates and 19 SEO/IA checks
- **Report:** one HTML file that works offline:
  - a visual card sitemap with SVG export
  - content anatomy and the interactive link graph
  - templates, issues and a page inventory
  - an optional strategy brief written by you or your agent

## Set it up with your agent

Copy this into Claude Code (or any coding agent with a shell) and let it do the setup:

```text
Set up SiteCannon (https://github.com/vallafederico/sitecannon) for me:

1. Install the `sitecannon` binary if `sitecannon --version` fails:
   curl -fsSL https://raw.githubusercontent.com/vallafederico/sitecannon/main/install.sh | sh
   (It downloads a release binary, or builds from source with cargo; install Rust from https://rustup.rs if needed.)
   Make sure the install directory is on my PATH.
2. If you are Claude Code, install the plugin (it brings the MCP server and the skills):
   claude plugin marketplace add vallafederico/sitecannon
   claude plugin install sitecannon@sitecannon
   Otherwise register an MCP server named "sitecannon" with command `sitecannon` and args ["mcp"]
   in your MCP configuration, and read the skills in plugin/skills/*/SKILL.md from the repo.
3. Verify: `sitecannon tool list_projects` should print JSON. Then tell me the skills you can use
   (site-audit, ia-review, content-audit, seo-migration, client-report, sitecannon-data).

Don't crawl anything yet. I'll give you a site.
```

Then ask for something like *"audit https://example.com for a rebuild and write me the client report"*.

### Manual setup

```sh
curl -fsSL https://raw.githubusercontent.com/vallafederico/sitecannon/main/install.sh | sh

# Claude Code: MCP server + skills
claude plugin marketplace add vallafederico/sitecannon
claude plugin install sitecannon@sitecannon
```

Other MCP clients (Claude Desktop, Cursor, …) use this server entry:

```json
{ "mcpServers": { "sitecannon": { "command": "sitecannon", "args": ["mcp"] } } }
```

Projects are stored in `$SITECANNON_HOME`, or `./projects` relative to where the server runs.

## Using it

### As a person (CLI)

```sh
sitecannon crawl example.com                 # → ./projects/example.com/site.duckdb
sitecannon analyze projects/example.com      # sitemaps, templates, sections, topics, link metrics
sitecannon info projects/example.com         # summary in the terminal
sitecannon report projects/example.com       # → projects/example.com/report.html
sitecannon report projects/example.com --notes brief.md   # embed a strategy brief
sitecannon query projects/example.com "SELECT * FROM broken_links"
sitecannon reextract projects/example.com    # re-run extraction on stored HTML after upgrading
```

Crawl options: `--max-pages` (1000), `--max-depth` (50), `--concurrency` (8), `--rps` (5),
`--max-query-variants` (25), `--no-sitemaps`, `--no-html`, `--ignore-robots` (only with the owner's permission), `--force`.

### As an agent (tools)

The same 12 tools are available three ways. All take JSON arguments and return JSON.

| Tool | What it returns |
|---|---|
| `list_projects` | Crawled projects, dates, whether analysed, report path |
| `crawl` | Crawls and analyses a new site (polite defaults: 500 pages, 3 req/s) |
| `summary` | A 10–15 KB digest: site facts, coverage caveats, structure, navigation, templates and their typical sections, topics, issues, top pages |
| `sitemap` | A nested tree by URL folders, menus or click paths |
| `templates` | Layouts, one-off pages, typical section order, section inventory |
| `topics` | Topics, their key terms, where they live, example pages |
| `issues` | Checks with severity and counts, or the affected URLs for one check |
| `page` | One page in full: SEO, metrics, sections, topic, who links to it |
| `schema`, `query` | Table guide, and read-only SQL (DuckDB) |
| `analyze` | Recompute the analysis |
| `report` | Write the HTML report, optionally with a markdown brief (`notes`) |

```sh
sitecannon tools                                             # list with JSON schemas
sitecannon tool summary '{"project":"example.com"}'          # CLI
sitecannon mcp                                               # MCP over stdio
sitecannon serve --port 7878                                 # HTTP: GET /tools, POST /tools/{name}
curl -s localhost:7878/tools/issues -d '{"project":"example.com"}' -H 'content-type: application/json'
```

### Skills (Claude Code plugin)

| Skill | Use it to |
|---|---|
| `site-audit` | Go from a URL to a client report: crawl, investigate, write the brief, build the report |
| `ia-review` | Compare URL, menu and click structures, diagnose the IA, propose a new sitemap |
| `content-audit` | Produce the page types, content model, component list, and what to keep, merge or cut |
| `seo-migration` | Prepare a migration: redirect debt, URL tiers, redirect-map CSV, launch checklist |
| `client-report` | Write findings up for a prospect, client or product team and embed them in the report |
| `sitecannon-data` | Reference: tools, tables, SQL cookbook and caveats |

## The report

`sitecannon report` writes one self-contained HTML file. The data is embedded and no server is needed.

- **Overview:** the strategy brief (if given), headline numbers, click depth, template mix, and issues by severity.
- **Structure:**
  - a visual card sitemap built from URLs, menus or click paths, with SVG export
  - the same three trees as lists
  - which URL sections the header and footer never reach
- **Content:**
  - topics, and where they live by URL section or template
  - page anatomy: each page's sections, and each template's typical page
  - the section inventory
- **Link graph:** every page and editorial link (WebGL), colored by template, section or depth, and sized by internal PageRank.
- **Templates:** each layout the rebuild must design once, plus the one-off pages.
- **Issues:** each check with its affected URLs. Every check is plain SQL in `src/report.rs`.
- **Pages:** a searchable inventory.

## Data model

| Table / view | Contents |
|---|---|
| `pages` | One row per URL: status, full redirect chain, headers, title, meta, robots, canonical, hreflang, headings, OG, JSON-LD types, platform fingerprint, word counts, template (`skeleton_hash`) and content (`content_hash`) SimHashes |
| `pages_v` | `pages` + `in_sitemap`, `canonical_is_self`, `indexability` |
| `page_content` | Main-content markdown (+ how it was found: `main_element` / `readability` / `full_page`), full-page markdown with site chrome and hidden elements stripped, raw JSON-LD |
| `page_html` | Raw HTML as served, zstd-compressed (skip with `--no-html`), so analysis can be re-run without re-crawling |
| `links` | Every `<a href>`: target, anchor, rel/nofollow, internal, **position** (nav / header / footer / breadcrumb / aside / main / body) |
| `images` | src + alt (missing vs empty) |
| `sitemap_entries` | URLs and lastmod from robots-declared or default XML sitemaps (recursive, gzip ok) |
| `inlinks` | Inlink counts per URL, split contextual / nav / footer |
| `broken_links`, `redirected_links`, `sitemap_only` | Ready-made issue lists |

After `analyze`:

| Table / view | Contents |
|---|---|
| `page_metrics` | Per content page: click depth from the homepage, click-tree parent, inlinks/outlinks, internal PageRank (1.0 = average), template, duplicate group, orphan / dead-end flags |
| `templates` | Template clusters with a URL pattern (`/blog/*`) and example page; single-page clusters are one-off pages |
| `tree_url` | The URL hierarchy, including folders that have no page of their own |
| `nav_items`, `global_nav` | Header and footer links grouped by dropdown/column label, with how many pages carry them |
| `redirect_map` | Every redirecting URL and where it finally resolves |
| `duplicate_content` | Groups of pages with near-identical main content |
| `pages_full` | `pages_v` + `page_metrics` + template pattern in one view |
| `topics`, `page_topics` | Topics (TF-IDF + k-means over each page's own content sections, labelled by distinctive terms) and each page's topic and keywords |
| `page_sections` | Every page split into top-level sections: type, role group, heading, words, links, images, repeated cards, topic |

Templates are detected from the layout containers inside the main content region (tag + class
paths, rich-text bodies and site chrome excluded), clustered by Jaccard similarity.

## Layout

```
src/
  main.rs      CLI
  crawl.rs     frontier, rate limiting, manual redirect following
  discover.rs  robots.txt + XML sitemaps
  extract.rs   HTML → SEO fields, links with position, markdown, SimHashes
  store.rs     DuckDB schema, writer thread, views
  analyze.rs   link graph, PageRank, click depth, URL tree, duplicates, nav
  templates.rs template detection from main-content layout
  content.rs   page sections (typed by role) and topic discovery
  report.rs    report data + issue checks, embeds report/dist/index.html
  query.rs     read-only SQL + `info`
  tools.rs     agent tool registry (shared by CLI `tool`, MCP and HTTP)
  mcp.rs       MCP server over stdio
  serve.rs     HTTP API
  urlnorm.rs   URL normalisation and site scope
report/        TypeScript report app (Vite, sigma.js, d3), built to a single HTML file
plugin/        Claude Code plugin: MCP server config + skills
.claude-plugin/marketplace.json   lets `claude plugin marketplace add vallafederico/sitecannon` work
```

## Development

```sh
pnpm -C report install && pnpm -C report build   # report app → report/dist/index.html (embedded in the binary)
cargo build --release                             # first build compiles DuckDB, takes a few minutes
cargo test
claude plugin validate ./plugin && claude plugin validate .
```

Releases: push a tag like `v0.1.0`; GitHub Actions builds macOS and Linux binaries for `install.sh`.

See [RESEARCH.md](RESEARCH.md) for the research and roadmap behind the project.

## License

MIT, see [LICENSE](LICENSE).
