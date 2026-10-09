# SiteCannon

Study a website before you rebuild it. A single Rust binary that crawls a site and stores
everything into a queryable DuckDB database: SEO signals, link graph, content and templates.

See [RESEARCH.md](RESEARCH.md) for the research and roadmap behind it.

## Build

```sh
pnpm -C report install && pnpm -C report build   # report app → report/dist/index.html (embedded in the binary)
cargo build --release                             # first build compiles DuckDB, takes a few minutes
```

## Use

```sh
sitecannon crawl example.com                 # → ./projects/example.com/site.duckdb
sitecannon analyze projects/example.com      # link graph, click depth, templates, duplicates, nav
sitecannon info  projects/example.com        # status, indexability, issues + analysis summary
sitecannon report projects/example.com       # → projects/example.com/report.html (single file, works offline)
sitecannon query projects/example.com "SELECT url, title, word_count FROM pages_v WHERE indexability = 'indexable'"
sitecannon query projects/example.com "SELECT * FROM broken_links" --json
```

Crawl options: `--max-pages` (1000), `--max-depth` (50), `--concurrency` (8), `--rps` (5),
`--max-query-variants` (25), `--no-sitemaps`, `--no-html`, `--ignore-robots` (only with the owner's permission), `--force`.

Analyze options: `--template-similarity` (0.7, Jaccard of main-content layout), `--duplicate-bits` (3).

The crawler is polite by default: it obeys robots.txt (including `Crawl-delay`), rate-limits per site,
and backs off on 429/503.

## What gets collected

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

## The report

`sitecannon report` writes one self-contained HTML file (data embedded, no server needed):

- **Overview**: headline numbers, click-depth distribution, template mix, issues by severity
- **Structure**: a visual card sitemap (by URLs, menus or click paths; SVG export) and the same three trees as lists, plus which URL sections the header/footer never reach
- **Content**: topics found from page text, where each topic lives (by URL section or template), page anatomy (each page's sections typed hero / features / testimonials / FAQ / pricing… and tagged by topic, plus each template's typical page), and a section inventory
- **Link graph**: every page and editorial link (WebGL, sigma.js), coloured by template, section or depth, sized by internal PageRank
- **Templates**: each layout the rebuild must design once, plus the one-off pages
- **Issues**: every check with its affected URLs; each check is plain SQL in `src/report.rs`
- **Pages**: searchable inventory

`--json` writes just the data, e.g. for `pnpm -C report dev` (save it as `report/public/sample.json`).

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
  urlnorm.rs   URL normalisation and site scope
report/        TypeScript report app (Vite, sigma.js), built to a single HTML file
```
