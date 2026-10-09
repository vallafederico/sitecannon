//! Builds the single-file HTML report: the prebuilt report app (report/dist/index.html)
//! with the project's data embedded as JSON.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::query::{self, Table};

const TEMPLATE: &str = include_str!("../report/dist/index.html");
const PLACEHOLDER: &str = "\"__SITECANNON_DATA__\"";
const ISSUE_ROW_LIMIT: usize = 1000;

pub struct Issue {
    pub id: &'static str,
    pub title: &'static str,
    pub severity: &'static str,
    pub description: &'static str,
    pub sql: &'static str,
}

/// Every check the report lists. Plain SQL over the project database, so each one can be
/// reproduced with `sitecannon query`.
pub const ISSUES: &[Issue] = &[
    Issue {
        id: "broken_links",
        title: "Broken internal links",
        severity: "critical",
        description: "Links between pages of this site that end in an error. Visitors hit dead ends and crawlers waste budget.",
        sql: "SELECT from_url AS found_on, to_url AS broken_url, coalesce(final_status, status) AS status, anchor, position FROM broken_links ORDER BY to_url, from_url",
    },
    Issue {
        id: "missing_title",
        title: "Indexable pages without a title",
        severity: "critical",
        description: "No <title> in the HTML the server sends. Search engines and link previews fall back to guessing.",
        sql: "SELECT url, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND coalesce(title, '') = '' ORDER BY pagerank DESC",
    },
    Issue {
        id: "refused",
        title: "URLs the server refused (401/403/429)",
        severity: "info",
        description: "Usually bot protection or rate limiting reacting to the crawl, not missing pages, so they are kept out of the broken-link findings. Check a few in a browser; for a full audit, ask the site owner to allowlist the crawler.",
        sql: "SELECT url, status, fetched_at FROM refused_urls ORDER BY fetched_at",
    },
    Issue {
        id: "sitemap_non_indexable",
        title: "Non-indexable URLs in the XML sitemap",
        severity: "serious",
        description: "Sitemaps should list only canonical, indexable 200 pages; anything else sends mixed signals.",
        sql: "SELECT url, indexability, status FROM pages_v WHERE in_sitemap AND indexability NOT IN ('indexable', 'refused') ORDER BY indexability, url",
    },
    Issue {
        id: "redirect_chains",
        title: "Redirect chains",
        severity: "serious",
        description: "URLs that take more than one hop to resolve. Each hop costs time and dilutes signals; a migration adds one more.",
        sql: "SELECT url, final_url, redirect_hops AS hops, final_status FROM pages WHERE redirect_hops > 1 ORDER BY redirect_hops DESC, url",
    },
    Issue {
        id: "links_to_redirects",
        title: "Internal links pointing at redirects",
        severity: "warning",
        description: "Links that should point straight at the final URL. Cheap to fix and worth doing before a migration adds another hop.",
        sql: "SELECT to_url AS linked_url, any_value(final_url) AS resolves_to, count(*) AS links, count(DISTINCT from_url) AS pages FROM redirected_links GROUP BY 1 ORDER BY links DESC",
    },
    Issue {
        id: "missing_meta_description",
        title: "Missing meta description",
        severity: "warning",
        description: "Indexable pages with no (or an empty) meta description; search engines will improvise the snippet.",
        sql: "SELECT url, title, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND coalesce(meta_description, '') = '' ORDER BY pagerank DESC",
    },
    Issue {
        id: "duplicate_titles",
        title: "Duplicate titles",
        severity: "warning",
        description: "Different indexable pages sharing one title compete with each other and are hard to tell apart in results.",
        sql: "SELECT title, count(*) AS pages, list(url ORDER BY pagerank DESC)[1:5] AS examples FROM pages_full WHERE indexability = 'indexable' AND coalesce(title, '') <> '' GROUP BY title HAVING count(*) > 1 ORDER BY pages DESC",
    },
    Issue {
        id: "missing_h1",
        title: "Missing H1",
        severity: "warning",
        description: "Indexable pages without a main heading.",
        sql: "SELECT url, title, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND h1_count = 0 ORDER BY pagerank DESC",
    },
    Issue {
        id: "multiple_h1",
        title: "Multiple H1s",
        severity: "info",
        description: "Not an error, but usually a sign that heading levels are used for styling rather than structure.",
        sql: "SELECT url, h1_count, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND h1_count > 1 ORDER BY h1_count DESC",
    },
    Issue {
        id: "thin_content",
        title: "Thin pages (under 150 words of main content)",
        severity: "warning",
        description: "Indexable pages with little unique content once navigation and footer are removed. Candidates to merge, enrich or noindex.",
        sql: "SELECT url, main_word_count AS words, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND main_word_count < 150 ORDER BY main_word_count, url",
    },
    Issue {
        id: "near_duplicates",
        title: "Near-duplicate content",
        severity: "warning",
        description: "Groups of pages whose main content is almost identical.",
        sql: "SELECT dup_group AS group_id, pages, urls FROM duplicate_content",
    },
    Issue {
        id: "deep_pages",
        title: "Pages four or more clicks from the homepage",
        severity: "warning",
        description: "Deep pages get crawled less often and receive less internal link equity.",
        sql: "SELECT url, click_depth AS depth, template_pattern AS template FROM pages_full WHERE click_depth >= 4 ORDER BY click_depth DESC, pagerank",
    },
    Issue {
        id: "sitemap_only",
        title: "Sitemap URLs that nothing links to",
        severity: "warning",
        description: "Listed in the XML sitemap but not linked from any crawled page: orphans, or pages the crawl didn't reach if it was capped.",
        sql: "SELECT url, lastmod, sitemap FROM sitemap_only ORDER BY url",
    },
    Issue {
        id: "images_missing_alt",
        title: "Images without alt text",
        severity: "warning",
        description: "Images missing the alt attribute entirely (empty alt for decorative images is fine).",
        sql: "SELECT page_url AS url, count(*) AS images FROM images WHERE alt_missing AND page_url IN (SELECT url FROM page_metrics) GROUP BY 1 ORDER BY images DESC",
    },
    Issue {
        id: "folders_without_page",
        title: "URL folders without a landing page",
        severity: "info",
        description: "Paths like /resources/… with pages beneath but no page at /resources itself. Often a gap in the information architecture.",
        sql: "SELECT path, subtree_pages AS pages_below FROM tree_url WHERE url IS NULL AND depth > 0 AND subtree_pages >= 2 ORDER BY subtree_pages DESC",
    },
    Issue {
        id: "canonicalised",
        title: "Canonicalised pages",
        severity: "info",
        description: "Pages that declare a different URL as canonical. Expected for variants, worth checking otherwise.",
        sql: "SELECT url, canonical FROM pages_v WHERE indexability = 'canonicalised' ORDER BY url",
    },
    Issue {
        id: "noindex",
        title: "Noindex pages",
        severity: "info",
        description: "Pages excluded from search via meta robots or X-Robots-Tag.",
        sql: "SELECT url, meta_robots, x_robots_tag FROM pages_v WHERE indexability = 'noindex' ORDER BY url",
    },
    Issue {
        id: "no_structured_data",
        title: "Pages without structured data",
        severity: "info",
        description: "No JSON-LD found. Structured data drives rich results and helps machines understand page type.",
        sql: "SELECT url, template_pattern AS template FROM pages_full WHERE indexability = 'indexable' AND jsonld_types = '[]' ORDER BY pagerank DESC",
    },
];

fn rows(t: &Table) -> Vec<Value> {
    t.rows
        .iter()
        .map(|r| Value::Array(r.iter().map(query::to_json).collect()))
        .collect()
}

fn column(t: &Table, name: &str) -> Vec<Value> {
    let i = t.columns.iter().position(|c| c == name).unwrap_or(usize::MAX);
    t.rows
        .iter()
        .map(|r| r.get(i).map(query::to_json).unwrap_or(Value::Null))
        .collect()
}

/// Writes the report (or, with `json_only`, just its data) and returns the output path.
pub fn run(project: &Path, out: Option<PathBuf>, json_only: bool, notes: Option<&str>) -> Result<PathBuf> {
    let q = |sql: &str| query::run(project, sql).with_context(|| format!("report query failed: {sql}"));

    let pages = q("SELECT m.url, p.title, p.final_status AS status, p.indexability, m.template_id AS template, pt.topic_id AS topic,
                m.click_depth AS depth, m.click_parent, round(m.pagerank, 3) AS pagerank, m.inlinks, m.outlinks,
                p.word_count AS words, p.main_word_count AS main_words, p.in_sitemap
         FROM page_metrics m JOIN pages_v p USING (url) LEFT JOIN page_topics pt USING (url)
         ORDER BY m.pagerank DESC, m.url")?;
    let urls: Vec<String> = column(&pages, "url")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let index: HashMap<&str, usize> = urls.iter().enumerate().map(|(i, u)| (u.as_str(), i)).collect();
    let idx = |v: &Value| -> i64 {
        v.as_str()
            .and_then(|u| index.get(u))
            .map(|&i| i as i64)
            .unwrap_or(-1)
    };

    let parent: Vec<i64> = column(&pages, "click_parent").iter().map(idx).collect();
    let mut page_cols = serde_json::Map::new();
    for c in [
        "url",
        "title",
        "status",
        "indexability",
        "template",
        "topic",
        "depth",
        "pagerank",
        "inlinks",
        "outlinks",
        "words",
        "main_words",
        "in_sitemap",
    ] {
        page_cols.insert(c.to_string(), Value::Array(column(&pages, c)));
    }
    page_cols.insert("parent".into(), json!(parent));

    // Edge kind: 2 = editorial (in the content, to a page that isn't linked from everywhere),
    // 1 = other in-page links (sidebars, widgets, sitewide promos), 0 = menu/footer only.
    let edges_t = q("WITH e AS (
             SELECT l.from_url, coalesce(r.resolved_url, l.to_url) AS target, l.position
             FROM links l LEFT JOIN redirect_map r ON r.url = l.to_url
             WHERE l.internal AND l.to_url IS NOT NULL AND l.from_url IN (SELECT url FROM page_metrics)),
         share AS (
             SELECT target, count(DISTINCT from_url) / (SELECT count(*) FROM page_metrics) AS s FROM e GROUP BY 1)
         SELECT e.from_url, e.target,
             CASE WHEN bool_or(e.position IN ('main', 'body', 'breadcrumb')) AND any_value(share.s) < 0.1 THEN 2
                  WHEN bool_or(e.position NOT IN ('nav', 'header', 'footer')) THEN 1 ELSE 0 END AS kind
         FROM e JOIN share USING (target) GROUP BY 1, 2")?;
    let mut edges: Vec<i64> = Vec::new();
    for r in &edges_t.rows {
        let (a, b) = (idx(&query::to_json(&r[0])), idx(&query::to_json(&r[1])));
        if a >= 0 && b >= 0 && a != b {
            edges.extend([a, b, query::to_json(&r[2]).as_i64().unwrap_or(0)]);
        }
    }

    let topics = q("SELECT topic_id AS id, label, terms, pages FROM topics ORDER BY topic_id")?;
    let topics_json: Vec<Value> = topics
        .rows
        .iter()
        .map(|r| {
            let v: Vec<Value> = r.iter().map(query::to_json).collect();
            json!({"id": v[0], "label": v[1], "terms": v[2], "pages": v[3]})
        })
        .collect();

    // Sections, columnar: one entry per section, `page` indexes into `pages`.
    let secs = q(
        "SELECT url, ordinal, section_type, section_group, heading, words, topic_id FROM page_sections ORDER BY url, ordinal",
    )?;
    let mut sec = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for r in &secs.rows {
        let v: Vec<Value> = r.iter().map(query::to_json).collect();
        let page = idx(&v[0]);
        if page < 0 {
            continue;
        }
        sec.0.push(json!(page));
        sec.1.push(v[2].clone());
        sec.2.push(v[3].clone());
        sec.3.push(v[4].clone());
        sec.4.push(v[5].clone());
        sec.5.push(v[6].clone());
    }
    let sections_json = json!({"page": sec.0, "kind": sec.1, "group": sec.2, "heading": sec.3, "words": sec.4, "topic": sec.5});

    let tree =
        q("SELECT node_id, parent_id, segment, path, url, subtree_pages FROM tree_url ORDER BY node_id")?;
    let url_tree = json!({
        "parent": column(&tree, "parent_id").iter().map(|v| v.as_i64().unwrap_or(-1)).collect::<Vec<_>>(),
        "segment": column(&tree, "segment"),
        "path": column(&tree, "path"),
        "page": column(&tree, "url").iter().map(idx).collect::<Vec<_>>(),
        "subtree": column(&tree, "subtree_pages"),
    });

    let nav = q(
        "SELECT region, menu_group AS \"group\", anchor, url, coverage FROM nav_items
         WHERE coverage >= 0.02 ORDER BY region DESC, typical_ordinal, anchor LIMIT 600",
    )?;
    let nav_json: Vec<Value> = nav
        .rows
        .iter()
        .map(|r| {
            let v: Vec<Value> = r.iter().map(query::to_json).collect();
            json!({"region": v[0], "group": v[1], "anchor": v[2], "url": v[3], "page": idx(&v[3]), "coverage": v[4]})
        })
        .collect();

    let templates =
        q("SELECT template_id AS id, pages, pattern, example_url FROM templates ORDER BY pages DESC, id")?;
    let templates_json: Vec<Value> = templates
        .rows
        .iter()
        .map(|r| {
            let v: Vec<Value> = r.iter().map(query::to_json).collect();
            json!({"id": v[0], "pages": v[1], "pattern": v[2], "example": idx(&v[3])})
        })
        .collect();

    let site = q("SELECT seed, started_at, robots_status, stop_reason,
                (SELECT count(*) FROM refused_urls) AS refused, (config->>'max_pages')::INT AS max_pages,
                (SELECT count(*) FROM pages) AS crawled,
                (SELECT count(DISTINCT url_norm) FROM sitemap_entries) AS sitemap_urls,
                (SELECT platform FROM pages WHERE is_html AND final_status = 200 GROUP BY 1 ORDER BY count(*) DESC LIMIT 1) AS platform,
                (SELECT CASE WHEN headers->>'x-vercel-id' IS NOT NULL THEN 'Vercel' WHEN headers->>'cf-ray' IS NOT NULL THEN 'Cloudflare'
                        WHEN headers->>'x-nf-request-id' IS NOT NULL THEN 'Netlify' WHEN headers->>'x-amz-cf-id' IS NOT NULL THEN 'CloudFront'
                        ELSE headers->>'server' END FROM pages WHERE is_html GROUP BY 1 ORDER BY count(*) DESC LIMIT 1) AS hosting
         FROM crawl")?;
    let s: Vec<Value> = site
        .rows
        .first()
        .map(|r| r.iter().map(query::to_json).collect())
        .unwrap_or_default();
    let get = |name: &str| {
        site.columns
            .iter()
            .position(|c| c == name)
            .and_then(|i| s.get(i))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let crawled = get("crawled").as_i64().unwrap_or(0);
    let max_pages = get("max_pages").as_i64().unwrap_or(i64::MAX);

    let mut issues = Vec::new();
    for issue in ISSUES {
        let total = q(&format!("SELECT count(*) FROM ({})", issue.sql))?;
        let total = total
            .rows
            .first()
            .map(|r| query::to_json(&r[0]))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let t = q(&format!("{} LIMIT {ISSUE_ROW_LIMIT}", issue.sql))?;
        issues.push(json!({
            "id": issue.id, "title": issue.title, "severity": issue.severity,
            "description": issue.description, "total": total, "columns": t.columns, "rows": rows(&t),
        }));
    }

    let data = json!({
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "site": {
            "seed": get("seed"), "started_at": get("started_at"), "robots_status": get("robots_status"),
            "crawled": crawled, "max_pages": max_pages, "capped": crawled >= max_pages,
            "sitemap_urls": get("sitemap_urls"), "platform": get("platform"), "hosting": get("hosting"),
            "stop_reason": get("stop_reason"), "refused": get("refused"),
        },
        "pages": page_cols,
        "edges": edges,
        "url_tree": url_tree,
        "nav": nav_json,
        "templates": templates_json,
        "topics": topics_json,
        "sections": sections_json,
        "issues": issues,
        // Agent- or analyst-written strategy brief (markdown), shown first on the overview.
        "notes": notes,
    });

    let default_name = if json_only { "report.json" } else { "report.html" };
    let out = out.unwrap_or_else(|| {
        let dir = if project.is_dir() {
            project.to_path_buf()
        } else {
            project.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        dir.join(default_name)
    });
    if json_only {
        std::fs::write(&out, serde_json::to_string(&data)?)
            .with_context(|| format!("writing {}", out.display()))?;
        return Ok(out);
    }

    // `<` only ever appears inside JSON strings, where < is equivalent; this keeps
    // page content from closing the <script> tag the data lives in.
    let payload = serde_json::to_string(&data)?.replace('<', "\\u003c");
    anyhow::ensure!(
        TEMPLATE.contains(PLACEHOLDER),
        "report template is missing its data placeholder"
    );
    let html = TEMPLATE.replacen(PLACEHOLDER, &payload, 1);

    std::fs::write(&out, html).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}
