//! The agent-facing tool registry. Every tool takes JSON arguments and returns JSON, and
//! is exposed identically through the CLI (`sitecannon tool`), the MCP server
//! (`sitecannon mcp`) and the HTTP API (`sitecannon serve`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use url::Url;

use crate::query::{self, Table};
use crate::{analytics, analyze, crawl, report, store};

pub const DEFAULT_UA: &str = concat!("SiteCannon/", env!("CARGO_PKG_VERSION"), " (site audit crawler)");
const QUERY_ROW_CAP: usize = 500;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
}

fn project_prop() -> Value {
    json!({"type": "string", "description": "Project name (a folder under the projects root, e.g. \"example.com\") or a path to a project directory"})
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "list_projects",
        description: "List crawled projects under the projects root with their seed URL, page count and whether they have been analysed. Start here to see what's available.",
        schema: || json!({"type": "object", "properties": {}}),
    },
    Tool {
        name: "crawl",
        description: "Crawl a website into a new project, then analyse it. Polite by default (obeys robots.txt, rate-limited, stops if the site starts refusing). Takes roughly max_pages / rps seconds. Only crawl sites the user asked about.",
        schema: || {
            json!({"type": "object", "required": ["url"], "properties": {
                "url": {"type": "string", "description": "Start URL, usually the homepage"},
                "max_pages": {"type": "integer", "default": 500, "description": "Stop after this many URLs"},
                "rps": {"type": "number", "default": 3, "description": "Requests per second"},
                "project": {"type": "string", "description": "Project name; defaults to the host"},
                "force": {"type": "boolean", "default": false, "description": "Replace an existing crawl of the same project"}
            }})
        },
    },
    Tool {
        name: "analyze",
        description: "(Re)compute link metrics, click depth, URL tree, navigation, templates, duplicates, page sections and topics for a project. Crawl already does this; use after re-extraction or to tune template similarity.",
        schema: || {
            json!({"type": "object", "required": ["project"], "properties": {
                "project": project_prop(),
                "template_similarity": {"type": "number", "default": 0.7}
            }})
        },
    },
    Tool {
        name: "summary",
        description: "Compact digest of a project sized for an agent's context: site facts, coverage caveats, structure, navigation, templates with their typical section order, topics, issue counts and the most-linked pages. Read this before anything else.",
        schema: || json!({"type": "object", "required": ["project"], "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "schema",
        description: "Tables and views in the project database with column names and what each table means. Read before writing SQL for `query`.",
        schema: || json!({"type": "object", "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "query",
        description: "Run read-only SQL (DuckDB dialect) against a project. Returns up to 500 rows as objects. JSON columns come back parsed.",
        schema: || {
            json!({"type": "object", "required": ["project", "sql"], "properties": {
                "project": project_prop(),
                "sql": {"type": "string"},
                "limit": {"type": "integer", "default": 200}
            }})
        },
    },
    Tool {
        name: "page",
        description: "Everything known about one page: SEO fields, metrics, template, topic and keywords, its sections in order, and who links to it.",
        schema: || {
            json!({"type": "object", "required": ["project", "url"], "properties": {
                "project": project_prop(),
                "url": {"type": "string", "description": "Full URL or path (e.g. /pricing)"}
            }})
        },
    },
    Tool {
        name: "issues",
        description: "SEO and IA checks with severity and counts. Pass `id` to get the affected URLs for one check.",
        schema: || {
            json!({"type": "object", "required": ["project"], "properties": {
                "project": project_prop(),
                "id": {"type": "string", "description": "Check id from the list, e.g. broken_links"},
                "limit": {"type": "integer", "default": 50}
            }})
        },
    },
    Tool {
        name: "sitemap",
        description: "The site as a nested tree: by URL structure (`url`), by persistent menus (`nav`) or by shortest click path from the homepage (`click`). Large branches are truncated with a count of what's hidden.",
        schema: || {
            json!({"type": "object", "required": ["project"], "properties": {
                "project": project_prop(),
                "kind": {"type": "string", "enum": ["url", "nav", "click"], "default": "url"},
                "depth": {"type": "integer", "default": 3},
                "max_children": {"type": "integer", "default": 25}
            }})
        },
    },
    Tool {
        name: "templates",
        description: "Page templates (layouts shared by several pages) and one-off pages, each with URL shapes, example pages, median word count and typical section order. The basis for rebuild scoping.",
        schema: || json!({"type": "object", "required": ["project"], "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "topics",
        description: "Topics discovered from page text, with key terms, page counts, where they live (URL sections and templates) and example pages.",
        schema: || json!({"type": "object", "required": ["project"], "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "analytics_status",
        description: "What usage data (GA4, Search Console) a project has, its date range, how much of it matches crawled pages, and whether Google credentials are available on this machine.",
        schema: || json!({"type": "object", "required": ["project"], "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "fetch_analytics",
        description: "Pull real usage data into a project with the user's Google credentials: GA4 (sessions, engagement, key events, revenue, channels by landing page; views by page) and/or Search Console (clicks, impressions, CTR, position by page and by query). Read-only. Only for properties the user owns or has been given access to.",
        schema: || {
            json!({"type": "object", "required": ["project"], "properties": {
                "project": project_prop(),
                "ga4_property": {"type": "string", "description": "GA4 property id, e.g. 123456789 (Admin → Property settings)"},
                "gsc_site": {"type": "string", "description": "Search Console property: sc-domain:example.com or https://www.example.com/"},
                "days": {"type": "integer", "default": 90}
            }})
        },
    },
    Tool {
        name: "import_analytics",
        description: "Import a GA4 or Search Console CSV export into a project (no credentials needed). The kind is detected from the columns: GA4 landing pages (sessions…), GA4 pages (views…), Search Console pages or queries (clicks, impressions…).",
        schema: || {
            json!({"type": "object", "required": ["project", "file"], "properties": {
                "project": project_prop(),
                "file": {"type": "string", "description": "Path to the CSV file"},
                "kind": {"type": "string", "enum": ["ga4_landing", "ga4_pages", "gsc_pages", "gsc_queries"], "description": "Override detection"},
                "append": {"type": "boolean", "default": false, "description": "Add to existing rows instead of replacing them"}
            }})
        },
    },
    Tool {
        name: "performance",
        description: "What works and what doesn't, from real usage data joined to the crawl: performance by template, topic, click depth and section type, plus opportunity lists (hidden winners, over-promoted pages, top converters, poor engagement, traffic to unmatched URLs, zero-traffic pages, low-CTR snippets, striking-distance queries).",
        schema: || json!({"type": "object", "required": ["project"], "properties": {"project": project_prop()}}),
    },
    Tool {
        name: "report",
        description: "Write the single-file HTML client report. Optionally embed a strategy brief in markdown (shown first on the overview). Returns the file path.",
        schema: || {
            json!({"type": "object", "required": ["project"], "properties": {
                "project": project_prop(),
                "notes": {"type": "string", "description": "Markdown brief to embed (executive summary, findings, recommendations)"},
                "notes_file": {"type": "string", "description": "Path to a markdown file to embed instead of `notes`"}
            }})
        },
    },
];

pub fn list() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": (t.schema)()}))
            .collect(),
    )
}

/// Where projects live: `$SITECANNON_HOME`, else `./projects`.
pub fn projects_root() -> PathBuf {
    std::env::var_os("SITECANNON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("projects"))
}

pub fn resolve_project(name: &str) -> Result<PathBuf> {
    let direct = PathBuf::from(name);
    for candidate in [direct.clone(), projects_root().join(name)] {
        if candidate.join(store::DB_FILE).exists()
            || (candidate.is_file() && candidate.extension().is_some_and(|e| e == "duckdb"))
        {
            return Ok(candidate);
        }
    }
    bail!(
        "no project `{name}` (looked in {} and {}); use list_projects or crawl first",
        direct.display(),
        projects_root().join(name).display()
    )
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn project_arg(args: &Value) -> Result<PathBuf> {
    resolve_project(arg_str(args, "project").ok_or_else(|| anyhow!("missing `project`"))?)
}

fn table_json(t: &Table) -> Vec<Value> {
    t.rows
        .iter()
        .map(|r| {
            Value::Object(
                t.columns
                    .iter()
                    .cloned()
                    .zip(r.iter().map(query::to_json))
                    .collect(),
            )
        })
        .collect()
}

fn q(project: &Path, sql: &str) -> Result<Vec<Value>> {
    Ok(table_json(&query::run(project, sql)?))
}

fn q1(project: &Path, sql: &str) -> Result<Value> {
    Ok(q(project, sql)?.into_iter().next().unwrap_or(Value::Null))
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn analysed(project: &Path) -> bool {
    query::run(
        project,
        "SELECT 1 FROM information_schema.tables WHERE table_name = 'page_metrics'",
    )
    .is_ok_and(|t| !t.rows.is_empty())
}

fn ensure_analysed(project: &Path) -> Result<()> {
    if !analysed(project) {
        analyze::run(
            project,
            &analyze::Options {
                template_similarity: 0.7,
                duplicate_bits: 3,
            },
        )?;
    }
    Ok(())
}

/// Runs a tool. Blocking; call from a blocking context.
pub fn call(name: &str, args: &Value) -> Result<Value> {
    match name {
        "list_projects" => list_projects(),
        "crawl" => crawl_tool(args),
        "analyze" => {
            let p = project_arg(args)?;
            let sim = args
                .get("template_similarity")
                .and_then(Value::as_f64)
                .unwrap_or(0.7);
            let s = analyze::run(
                &p,
                &analyze::Options {
                    template_similarity: sim,
                    duplicate_bits: 3,
                },
            )?;
            Ok(
                json!({"pages": s.pages, "templates": s.templates, "one_off_pages": s.one_off_pages, "topics": s.topics,
                      "max_depth": s.max_depth, "orphans": s.orphans, "duplicate_groups": s.dup_groups}),
            )
        }
        "summary" => summary(&project_arg(args)?),
        "schema" => Ok(schema()),
        "query" => {
            let p = project_arg(args)?;
            let sql = arg_str(args, "sql").ok_or_else(|| anyhow!("missing `sql`"))?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(QUERY_ROW_CAP as u64) as usize;
            let t = query::run(&p, sql)?;
            let total = t.rows.len();
            let mut rows = table_json(&t);
            rows.truncate(limit);
            Ok(json!({"columns": t.columns, "rows": rows, "row_count": total, "truncated": total > limit}))
        }
        "page" => page(
            &project_arg(args)?,
            arg_str(args, "url").ok_or_else(|| anyhow!("missing `url`"))?,
        ),
        "issues" => issues(
            &project_arg(args)?,
            arg_str(args, "id"),
            args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize,
        ),
        "sitemap" => sitemap(
            &project_arg(args)?,
            arg_str(args, "kind").unwrap_or("url"),
            args.get("depth").and_then(Value::as_u64).unwrap_or(3) as usize,
            args.get("max_children").and_then(Value::as_u64).unwrap_or(25) as usize,
        ),
        "templates" => templates(&project_arg(args)?),
        "topics" => topics(&project_arg(args)?),
        "analytics_status" => analytics::status(&project_arg(args)?),
        "fetch_analytics" => {
            let p = project_arg(args)?;
            ensure_analysed(&p)?;
            let days = args.get("days").and_then(Value::as_u64).unwrap_or(90) as u32;
            analytics::fetch(&p, arg_str(args, "ga4_property"), arg_str(args, "gsc_site"), days)
        }
        "import_analytics" => {
            let p = project_arg(args)?;
            ensure_analysed(&p)?;
            let file = arg_str(args, "file").ok_or_else(|| anyhow!("missing `file`"))?;
            let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
            analytics::import(&p, Path::new(file), arg_str(args, "kind"), append)
        }
        "performance" => {
            let p = project_arg(args)?;
            ensure_analysed(&p)?;
            analytics::performance(&p)
        }
        "report" => {
            let p = project_arg(args)?;
            ensure_analysed(&p)?;
            let notes = match (arg_str(args, "notes"), arg_str(args, "notes_file")) {
                (Some(n), _) => Some(n.to_string()),
                (None, Some(f)) => Some(std::fs::read_to_string(f).with_context(|| format!("reading {f}"))?),
                _ => None,
            };
            let path = report::run(&p, None, false, notes.as_deref())?;
            Ok(json!({"path": path.canonicalize().unwrap_or(path).display().to_string()}))
        }
        other => bail!(
            "unknown tool `{other}`; available: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn list_projects() -> Result<Value> {
    let root = projects_root();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.join(store::DB_FILE).exists() {
                continue;
            }
            let info = q1(
                &dir,
                "SELECT seed, started_at, (SELECT count(*) FROM pages) AS urls FROM crawl",
            )
            .unwrap_or(Value::Null);
            out.push(json!({
                "project": e.file_name().to_string_lossy(),
                "path": dir.display().to_string(),
                "seed": info.get("seed"), "crawled_at": info.get("started_at"), "urls": info.get("urls"),
                "analysed": analysed(&dir),
                "report": dir.join("report.html").exists().then(|| dir.join("report.html").display().to_string()),
            }));
        }
    }
    Ok(json!({"root": root.display().to_string(), "projects": out}))
}

fn crawl_tool(args: &Value) -> Result<Value> {
    let raw = arg_str(args, "url").ok_or_else(|| anyhow!("missing `url`"))?;
    let raw = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let seed = Url::parse(&raw).with_context(|| format!("invalid URL {raw}"))?;
    let host = seed.host_str().context("URL has no host")?.to_string();
    let name = arg_str(args, "project").map(String::from).unwrap_or(host);
    let out = projects_root().join(&name);
    let db = out.join(store::DB_FILE);
    if db.exists() {
        if !args.get("force").and_then(Value::as_bool).unwrap_or(false) {
            bail!("project `{name}` already exists; pass force=true to re-crawl, or use summary to read it");
        }
        std::fs::remove_file(&db)?;
        let _ = std::fs::remove_file(db.with_extension("duckdb.wal"));
    }
    let cfg = crawl::CrawlConfig {
        seed,
        max_pages: args.get("max_pages").and_then(Value::as_u64).unwrap_or(500) as usize,
        max_depth: 50,
        concurrency: 4,
        rps: args
            .get("rps")
            .and_then(Value::as_f64)
            .unwrap_or(3.0)
            .clamp(0.2, 10.0),
        user_agent: DEFAULT_UA.to_string(),
        robots_agent: "SiteCannon".into(),
        respect_robots: true,
        use_sitemaps: true,
        max_query_variants: 25,
        timeout: Duration::from_secs(20),
        keep_html: true,
    };
    // Called from blocking threads of the CLI/MCP/HTTP runtimes, or standalone.
    let stats = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(crawl::run(cfg, &out))?,
        Err(_) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(crawl::run(cfg, &out))?,
    };
    let s = analyze::run(
        &out,
        &analyze::Options {
            template_similarity: 0.7,
            duplicate_bits: 3,
        },
    )?;
    Ok(json!({
        "project": name, "path": out.display().to_string(),
        "urls_crawled": stats.pages, "sitemap_urls": stats.sitemap_urls, "seconds": stats.elapsed.as_secs(),
        "pages": s.pages, "templates": s.templates, "one_off_pages": s.one_off_pages, "topics": s.topics,
        "next": "call summary for the digest"
    }))
}

fn summary(p: &Path) -> Result<Value> {
    ensure_analysed(p)?;
    let site = q1(p, "SELECT seed, started_at, stop_reason,
            (SELECT count(*) FROM pages) AS urls_crawled,
            (config->>'max_pages')::INT AS max_pages,
            (SELECT count(*) FROM page_metrics) AS pages,
            (SELECT count(DISTINCT url_norm) FROM sitemap_entries) AS sitemap_urls,
            (SELECT count(*) FROM refused_urls) AS refused_urls,
            (SELECT platform FROM pages WHERE is_html AND final_status = 200 GROUP BY 1 ORDER BY count(*) DESC LIMIT 1) AS platform
        FROM crawl")?;
    let mut caveats = Vec::new();
    if site["urls_crawled"].as_i64() >= site["max_pages"].as_i64() {
        caveats.push(format!(
            "Crawl hit its {}-URL limit; the sitemaps list {} URLs. Orphan, link and sitemap findings are partial.",
            site["max_pages"], site["sitemap_urls"]
        ));
    }
    if site["refused_urls"].as_i64().unwrap_or(0) > 0 {
        caveats.push(format!("{} URLs were refused (401/403/429), likely bot protection; they are excluded from broken-link counts.", site["refused_urls"]));
    }
    if let Some(r) = site["stop_reason"].as_str() {
        caveats.push(format!("Crawl ended early: {r}"));
    }

    let structure = json!({
        "click_depth": q(p, "SELECT click_depth AS depth, count(*) AS pages FROM page_metrics GROUP BY 1 ORDER BY 1 NULLS LAST")?,
        "orphans": q1(p, "SELECT count(*) AS n FROM page_metrics WHERE is_orphan")?["n"],
        "sections": q(p, "SELECT path, subtree_pages AS pages, url IS NOT NULL AS has_landing_page FROM tree_url WHERE depth = 1 ORDER BY subtree_pages DESC LIMIT 20")?,
    });
    let navigation = json!({
        "header": q(p, "SELECT menu_group AS \"group\", anchor, url FROM global_nav WHERE region = 'header' AND anchor <> '' LIMIT 40")?,
        "footer": q(p, "SELECT menu_group AS \"group\", anchor, url FROM global_nav WHERE region = 'footer' AND anchor <> '' LIMIT 40")?,
    });
    let templates = templates_rows(p, 12)?;
    let one_offs = q1(p, "SELECT count(*) AS n FROM templates WHERE pages = 1")?["n"].clone();
    let topics = q(
        p,
        "SELECT topic_id AS id, label, pages FROM topics ORDER BY topic_id LIMIT 24",
    )
    .unwrap_or_default();
    let issues = issue_counts(p)?;
    let top_pages = q(
        p,
        "SELECT url, title, round(pagerank, 2) AS pagerank, inlinks, click_depth AS depth FROM pages_full ORDER BY pagerank DESC LIMIT 10",
    )?;
    Ok(json!({
        "site": site, "caveats": caveats, "structure": structure, "navigation": navigation,
        "templates": templates, "one_off_pages": one_offs, "topics": topics,
        "issues": issues.into_iter().filter(|i| i["count"].as_i64().unwrap_or(0) > 0).collect::<Vec<_>>(),
        "top_pages": top_pages,
        "usage_data": usage_highlights(p),
    }))
}

/// Templates with their URL shapes and most common section at each position.
/// Short performance digest when GA4/Search Console data is loaded; a hint otherwise.
fn usage_highlights(p: &Path) -> Value {
    if !analytics::has_data(p) {
        return json!("none loaded; use fetch_analytics or import_analytics for real usage data");
    }
    match analytics::performance(p) {
        Ok(perf) => json!({
            "sources": perf["sources"], "totals": perf["totals"], "matched": perf["matched"],
            "by_template": perf["by_template"].as_array().map(|a| a.iter().take(8).cloned().collect::<Vec<_>>()),
            "opportunity_counts": perf["opportunities"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v["count"].clone())).collect::<serde_json::Map<_, _>>()),
            "next": "call performance for the full breakdown and opportunity lists",
        }),
        Err(e) => json!({"error": format!("{e:#}")}),
    }
}

fn templates_rows(p: &Path, limit: usize) -> Result<Vec<Value>> {
    let mut rows = q(p, &format!(
        "SELECT t.template_id AS id, t.pages, t.pattern AS url_shapes, t.example_url AS example,
                (SELECT median(main_word_count) FROM pages_full f WHERE f.template_id = t.template_id) AS median_words
         FROM templates t WHERE t.pages > 1 ORDER BY t.pages DESC LIMIT {limit}"
    ))?;
    let typical = q(p, "WITH n AS (SELECT m.template_id, median(c) AS n FROM (SELECT url, count(*) AS c FROM page_sections GROUP BY url) s JOIN page_metrics m USING (url) GROUP BY 1),
             k AS (SELECT m.template_id, s.ordinal, s.section_type, count(*) AS c FROM page_sections s JOIN page_metrics m USING (url) GROUP BY ALL)
         SELECT k.template_id, list(top ORDER BY ordinal) AS sections FROM (
             SELECT template_id, ordinal, arg_max(section_type, c) AS top FROM k GROUP BY 1, 2) k
         JOIN n USING (template_id) WHERE k.ordinal < n.n GROUP BY 1").unwrap_or_default();
    for r in &mut rows {
        let id = r["id"].clone();
        let secs = typical
            .iter()
            .find(|t| t["template_id"] == id)
            .map(|t| t["sections"].clone())
            .unwrap_or(Value::Null);
        r.as_object_mut().unwrap().insert("typical_sections".into(), secs);
    }
    Ok(rows)
}

fn templates(p: &Path) -> Result<Value> {
    ensure_analysed(p)?;
    Ok(json!({
        "templates": templates_rows(p, 100)?,
        "one_off_pages": q(p, "SELECT f.url, f.title, round(f.pagerank, 2) AS pagerank,
                (SELECT list(section_type ORDER BY ordinal) FROM page_sections s WHERE s.url = f.url) AS sections
             FROM pages_full f JOIN templates t USING (template_id) WHERE t.pages = 1 ORDER BY f.pagerank DESC LIMIT 100")?,
        "section_inventory": q(p, "SELECT section_type, count(*) AS sections, count(DISTINCT url) AS pages FROM page_sections GROUP BY 1 ORDER BY pages DESC")?,
    }))
}

fn topics(p: &Path) -> Result<Value> {
    ensure_analysed(p)?;
    let mut list = q(
        p,
        "SELECT topic_id AS id, label, terms, pages FROM topics ORDER BY topic_id",
    )?;
    for t in &mut list {
        let id = t["id"].as_i64().unwrap_or(0);
        let o = t.as_object_mut().unwrap();
        o.insert("sections".into(), json!(q(p, &format!(
            "SELECT '/' || split_part(regexp_replace(url, '^https?://[^/]+/?', ''), '/', 1) AS section, count(*) AS pages
             FROM page_topics WHERE topic_id = {id} GROUP BY 1 ORDER BY 2 DESC LIMIT 6"))?));
        o.insert(
            "templates".into(),
            json!(q(
                p,
                &format!(
            "SELECT m.template_id, any_value(t.pattern) AS pattern, count(*) AS pages FROM page_topics pt
             JOIN page_metrics m USING (url) JOIN templates t USING (template_id)
             WHERE pt.topic_id = {id} GROUP BY 1 ORDER BY 3 DESC LIMIT 4")
            )?),
        );
        o.insert("example_pages".into(), json!(q(p, &format!(
            "SELECT url, title FROM page_topics JOIN pages USING (url) WHERE topic_id = {id} ORDER BY score DESC LIMIT 5"))?));
    }
    Ok(
        json!({"topics": list, "method": "TF-IDF over each page's own content sections, spherical k-means; labels are the most distinctive terms"}),
    )
}

fn page(p: &Path, url: &str) -> Result<Value> {
    ensure_analysed(p)?;
    let target = if url.starts_with("http") {
        url.to_string()
    } else {
        let seed: String = q1(p, "SELECT seed FROM crawl")?["seed"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        Url::parse(&seed)?.join(url)?.to_string()
    };
    let found = q1(
        p,
        &format!(
            "SELECT url FROM pages WHERE url = '{0}' OR url = '{0}/' OR url = rtrim('{0}', '/') OR final_url = '{0}' ORDER BY redirect_hops LIMIT 1",
            esc(&target)
        ),
    )?;
    let url = found["url"]
        .as_str()
        .ok_or_else(|| anyhow!("{target} was not crawled"))?
        .to_string();
    let u = esc(&url);
    Ok(json!({
        "page": q1(p, &format!("SELECT url, status, final_status, final_url, redirect_hops, indexability, title, meta_description,
                canonical, meta_robots, lang, h1, h1_count, word_count, main_word_count, jsonld_types, in_sitemap,
                click_depth, click_parent, round(pagerank, 3) AS pagerank, inlinks, outlinks, template_id, template_pattern
             FROM pages_full WHERE url = '{u}'"))?,
        "fetch": q1(p, &format!("SELECT status, final_status, final_url, redirect_chain, content_type, ttfb_ms, headers FROM pages WHERE url = '{u}'"))?,
        "topic": q1(p, &format!("SELECT pt.topic_id, t.label, pt.keywords FROM page_topics pt LEFT JOIN topics t USING (topic_id) WHERE url = '{u}'")).unwrap_or(Value::Null),
        "sections": q(p, &format!("SELECT ordinal, section_type, heading, words, links, images, cards, topic_id FROM page_sections WHERE url = '{u}' ORDER BY ordinal")).unwrap_or_default(),
        "headings": q1(p, &format!("SELECT headings FROM pages WHERE url = '{u}'"))?["headings"].clone(),
        "linked_from": q(p, &format!(
            "SELECT from_url, any_value(anchor) AS anchor, string_agg(DISTINCT position, ',') AS positions
             FROM links l LEFT JOIN redirect_map r ON r.url = l.to_url
             WHERE coalesce(r.resolved_url, l.to_url) = '{u}' AND from_url <> '{u}' GROUP BY 1 LIMIT 30"))?,
        "broken_links_on_page": q(p, &format!("SELECT to_url, anchor, status FROM broken_links WHERE from_url = '{u}'"))?,
    }))
}

fn issue_counts(p: &Path) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for i in report::ISSUES {
        let n = query::run(p, &format!("SELECT count(*) FROM ({})", i.sql))
            .ok()
            .and_then(|t| t.rows.first().map(|r| query::to_json(&r[0])))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        out.push(json!({"id": i.id, "title": i.title, "severity": i.severity, "count": n}));
    }
    Ok(out)
}

fn issues(p: &Path, id: Option<&str>, limit: usize) -> Result<Value> {
    ensure_analysed(p)?;
    match id {
        None => Ok(json!({"checks": issue_counts(p)?, "hint": "pass id to list affected URLs"})),
        Some(id) => {
            let i = report::ISSUES
                .iter()
                .find(|i| i.id == id)
                .ok_or_else(|| anyhow!("unknown check `{id}`"))?;
            let t = query::run(p, i.sql)?;
            let total = t.rows.len();
            let mut rows = table_json(&t);
            rows.truncate(limit);
            Ok(
                json!({"id": i.id, "title": i.title, "severity": i.severity, "description": i.description,
                      "count": total, "rows": rows, "sql": i.sql}),
            )
        }
    }
}

/// (id, parent id, label, url, page count) for building nested sitemap trees.
type TreeRow = (String, Option<String>, String, Option<String>, i64);

fn sitemap(p: &Path, kind: &str, depth: usize, max_children: usize) -> Result<Value> {
    ensure_analysed(p)?;
    // Build (id, parent, label, url, count) rows, then nest.
    let rows: Vec<TreeRow> = match kind {
        "url" => q(p, "SELECT node_id::VARCHAR AS id, parent_id::VARCHAR AS parent, path AS label, url, subtree_pages AS n FROM tree_url")?
            .into_iter()
            .map(|r| (r["id"].as_str().unwrap_or_default().into(), r["parent"].as_str().map(String::from),
                      r["label"].as_str().unwrap_or_default().into(), r["url"].as_str().map(String::from), r["n"].as_i64().unwrap_or(0)))
            .collect(),
        "click" => q(p, "WITH RECURSIVE sub(url, root) AS (SELECT url, url FROM page_metrics UNION ALL
                SELECT m.url, s.root FROM page_metrics m JOIN sub s ON m.click_parent = s.url)
             SELECT m.url AS id, m.click_parent AS parent, coalesce(p.title, m.url) AS label, m.url,
                    (SELECT count(*) FROM sub WHERE sub.root = m.url) AS n
             FROM page_metrics m JOIN pages p USING (url) WHERE m.click_depth IS NOT NULL")?
            .into_iter()
            .map(|r| (r["id"].as_str().unwrap_or_default().into(), r["parent"].as_str().map(String::from),
                      r["label"].as_str().unwrap_or_default().into(), r["url"].as_str().map(String::from), r["n"].as_i64().unwrap_or(1)))
            .collect(),
        "nav" => {
            let items = q(p, "SELECT region, coalesce(menu_group, '') AS grp, anchor, url, coverage FROM global_nav WHERE anchor <> ''")?;
            let mut rows = vec![("root".to_string(), None, "Homepage".to_string(), None, 0)];
            for r in &items {
                let region = r["region"].as_str().unwrap_or_default();
                let grp = r["grp"].as_str().unwrap_or_default();
                if !rows.iter().any(|x| x.0 == region) {
                    rows.push((region.into(), Some("root".into()), region.into(), None, 0));
                }
                let parent = if grp.is_empty() {
                    region.to_string()
                } else {
                    let gid = format!("{region}/{grp}");
                    if !rows.iter().any(|x| x.0 == gid) {
                        rows.push((gid.clone(), Some(region.into()), grp.into(), None, 0));
                    }
                    gid
                };
                rows.push((format!("{parent}/{}", rows.len()), Some(parent), r["anchor"].as_str().unwrap_or_default().into(),
                           r["url"].as_str().map(String::from), 1));
            }
            rows
        }
        other => bail!("kind must be url, nav or click (got {other})"),
    };
    let root = rows
        .iter()
        .find(|r| r.1.is_none())
        .map(|r| r.0.clone())
        .ok_or_else(|| anyhow!("empty tree"))?;
    let mut children: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some(par) = &r.1 {
            children.entry(par.clone()).or_default().push(i);
        }
    }
    let sort_by_size = kind != "nav";
    fn nest(
        id: &str,
        rows: &[TreeRow],
        children: &std::collections::HashMap<String, Vec<usize>>,
        depth: usize,
        max: usize,
        sort: bool,
    ) -> Value {
        let r = rows.iter().find(|r| r.0 == id).unwrap();
        let mut o = Map::new();
        o.insert("label".into(), json!(r.2));
        if let Some(u) = &r.3 {
            o.insert("url".into(), json!(u));
        }
        if r.4 > 1 {
            o.insert("pages".into(), json!(r.4));
        }
        let mut kids = children.get(id).cloned().unwrap_or_default();
        if sort {
            kids.sort_by(|a, b| rows[*b].4.cmp(&rows[*a].4));
        }
        if !kids.is_empty() {
            if depth == 0 {
                o.insert("children_hidden".into(), json!(kids.len()));
            } else {
                let shown: Vec<Value> = kids
                    .iter()
                    .take(max)
                    .map(|&k| nest(&rows[k].0, rows, children, depth - 1, max, sort))
                    .collect();
                o.insert("children".into(), Value::Array(shown));
                if kids.len() > max {
                    o.insert("more_children".into(), json!(kids.len() - max));
                }
            }
        }
        Value::Object(o)
    }
    Ok(json!({"kind": kind, "tree": nest(&root, &rows, &children, depth, max_children, sort_by_size)}))
}

/// Table descriptions for agents writing SQL.
const TABLES: &[(&str, &str)] = &[
    (
        "crawl",
        "One row: seed URL, timings, robots.txt, crawl config, stop_reason",
    ),
    (
        "pages",
        "Every requested URL: status, redirect chain, headers, SEO fields, word counts, platform, hashes",
    ),
    (
        "pages_v",
        "pages + in_sitemap, canonical_is_self, indexability (indexable/redirect/noindex/canonicalised/client_error/server_error/refused/…)",
    ),
    (
        "pages_full",
        "Content pages (HTML 200) joined with page_metrics and template_pattern; the usual starting point",
    ),
    (
        "page_metrics",
        "Per content page: click_depth, click_parent, inlinks, outlinks, pagerank (1.0 = site average), template_id, dup_group, is_orphan, is_dead_end",
    ),
    (
        "page_content",
        "main_markdown (+ main_source), full_markdown, raw JSON-LD",
    ),
    (
        "page_sections",
        "Each page's top-level sections in order: section_type (hero/features/listing/article/testimonials/logos/pricing/faq/cta/form/stats/…), section_group, heading, words, links, images, cards, topic_id",
    ),
    ("page_topics", "Each page's topic_id, score and keywords"),
    ("topics", "Discovered topics: label, terms, pages"),
    (
        "templates",
        "Layout clusters: pages, pattern (URL shapes), example_url; pages = 1 means a one-off page",
    ),
    (
        "links",
        "Every <a href>: from_url, to_url (normalised), anchor, rel, nofollow, internal, position (nav/header/footer/breadcrumb/aside/main/body), menu_group",
    ),
    ("images", "page_url, src, alt, alt_missing"),
    ("sitemap_entries", "XML sitemap URLs with lastmod"),
    (
        "tree_url",
        "URL hierarchy: node_id, parent_id, path, url (NULL = folder without a page), subtree_pages",
    ),
    (
        "nav_items",
        "Header/footer links aggregated across pages with coverage (share of pages carrying them)",
    ),
    (
        "global_nav",
        "nav_items with coverage >= 0.5: the persistent menus",
    ),
    ("redirect_map", "Redirecting URL → final resolved URL"),
    ("inlinks", "Inlink counts per URL split by contextual/nav/footer"),
    (
        "broken_links",
        "Internal links to 4xx/5xx/error URLs (refusals excluded)",
    ),
    ("redirected_links", "Internal links that point at a redirect"),
    ("sitemap_only", "Sitemap URLs no crawled page links to"),
    ("refused_urls", "URLs answered 401/403/429 (bot protection, auth)"),
    (
        "duplicate_content",
        "Groups of pages with near-identical main content",
    ),
    ("page_html", "Raw HTML (zstd BLOB); not useful from SQL"),
    (
        "page_performance",
        "After loading usage data: each content page with sessions, engaged_sessions, engagement_rate, key_events, revenue, organic_sessions, views, search_clicks/impressions/ctr/position, plus template, topic, depth, pagerank",
    ),
    (
        "ga_landing, ga_channels, ga_pages",
        "Raw GA4 rows (host, path, metrics); channels = sessions per landing page per default channel group",
    ),
    (
        "gsc_pages, gsc_queries",
        "Raw Search Console rows by page, and by page + query",
    ),
    (
        "analytics_unmatched",
        "Landing paths with sessions that aren't live crawled pages (old/redirecting/broken/not crawled)",
    ),
    (
        "analytics_meta",
        "Which usage sources are loaded, date ranges, method (api/csv)",
    ),
];

fn schema() -> Value {
    json!({
        "dialect": "DuckDB SQL. JSON columns support ->> and json_extract; lists support list functions.",
        "tables": TABLES.iter().map(|(t, d)| json!({"name": t, "description": d})).collect::<Vec<_>>(),
        "tip": "Use `query` with `DESCRIBE <table>` for exact columns.",
    })
}
