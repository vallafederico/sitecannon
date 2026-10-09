use std::path::Path;

use anyhow::Result;
use duckdb::types::{TimeUnit, Value};

use crate::store::open_readonly;

pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

pub fn run(project: &Path, sql: &str) -> Result<Table> {
    let conn = open_readonly(project)?;
    let mut stmt = conn.prepare(sql)?;
    let mut rows_iter = stmt.query([])?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next()? {
        let n = row.as_ref().column_count();
        rows.push(
            (0..n)
                .map(|i| row.get::<_, Value>(i).unwrap_or(Value::Null))
                .collect(),
        );
    }
    let columns = rows_iter.as_ref().map(|s| s.column_names()).unwrap_or_default();
    Ok(Table { columns, rows })
}

pub fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::json;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => json!(b),
        Value::TinyInt(n) => json!(n),
        Value::SmallInt(n) => json!(n),
        Value::Int(n) => json!(n),
        Value::BigInt(n) => json!(n),
        Value::HugeInt(n) => json!(n.to_string()),
        Value::UTinyInt(n) => json!(n),
        Value::USmallInt(n) => json!(n),
        Value::UInt(n) => json!(n),
        Value::UBigInt(n) => json!(n),
        Value::Float(n) => json!(n),
        Value::Double(n) => json!(n),
        Value::Decimal(d) => json!(d.to_string().parse::<f64>().unwrap_or(0.0)),
        Value::Text(s) => {
            // JSON columns come back as text; inline them as structured values.
            let t = s.trim_start();
            if t.starts_with('{') || t.starts_with('[') {
                serde_json::from_str(s).unwrap_or_else(|_| json!(s))
            } else {
                json!(s)
            }
        }
        Value::Timestamp(unit, n) => {
            let micros = match unit {
                TimeUnit::Second => n * 1_000_000,
                TimeUnit::Millisecond => n * 1_000,
                TimeUnit::Microsecond => *n,
                TimeUnit::Nanosecond => n / 1_000,
            };
            chrono::DateTime::from_timestamp_micros(micros)
                .map(|d| json!(d.to_rfc3339()))
                .unwrap_or(serde_json::Value::Null)
        }
        Value::List(items) | Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(to_json).collect())
        }
        other => json!(format!("{other:?}")),
    }
}

fn cell(v: &Value, max: usize) -> String {
    let s = match to_json(v) {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    let s = s.replace(['\n', '\r', '\t'], " ");
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    } else {
        s
    }
}

pub fn print_table(t: &Table, max_width: usize) {
    let cells: Vec<Vec<String>> = t
        .rows
        .iter()
        .map(|r| r.iter().map(|v| cell(v, max_width)).collect())
        .collect();
    let widths: Vec<usize> = t
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain([c.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |vals: Vec<&str>| {
        vals.iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:<w$}", w = *w))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("{}", line(t.columns.iter().map(String::as_str).collect()));
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for r in &cells {
        println!("{}", line(r.iter().map(String::as_str).collect()));
    }
    println!("({} rows)", t.rows.len());
}

pub fn print_json(t: &Table) {
    let rows: Vec<serde_json::Value> = t
        .rows
        .iter()
        .map(|r| serde_json::Value::Object(t.columns.iter().cloned().zip(r.iter().map(to_json)).collect()))
        .collect();
    println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
}

/// A first-look summary of a crawl. Each block is a plain SQL query so it doubles as
/// documentation of the schema.
pub const INFO_QUERIES: &[(&str, &str)] = &[
    (
        "Coverage",
        "SELECT (SELECT count(*) FROM pages) AS crawled, (config->>'max_pages')::INT AS max_pages, (SELECT count(DISTINCT url_norm) FROM sitemap_entries) AS sitemap_urls, (SELECT count(*) FROM pages) >= (config->>'max_pages')::INT AS capped, CASE WHEN (SELECT count(*) FROM pages) >= (config->>'max_pages')::INT THEN 'crawl hit --max-pages: link, orphan and sitemap findings are partial' ELSE 'complete' END AS note FROM crawl",
    ),
    (
        "Crawl",
        "SELECT seed, started_at, finished_at, robots_status, (SELECT count(*) FROM pages) AS urls, (SELECT count(*) FROM sitemap_entries) AS sitemap_urls FROM crawl",
    ),
    (
        "Status codes",
        "SELECT CASE WHEN blocked_by_robots THEN 'blocked by robots' WHEN status IS NULL THEN 'fetch error' ELSE (status // 100) || 'xx' END AS class, count(*) AS urls, string_agg(DISTINCT status::VARCHAR, ' ' ORDER BY status::VARCHAR) AS codes FROM pages GROUP BY 1 ORDER BY 1",
    ),
    (
        "Indexability",
        "SELECT indexability, count(*) AS urls FROM pages_v GROUP BY 1 ORDER BY 2 DESC",
    ),
    (
        "Discovered via",
        "SELECT discovered_via, count(*) AS urls FROM pages GROUP BY 1 ORDER BY 2 DESC",
    ),
    (
        "Platform & hosting",
        "SELECT platform, generator, CASE WHEN headers->>'x-vercel-id' IS NOT NULL THEN 'vercel' WHEN headers->>'cf-ray' IS NOT NULL THEN 'cloudflare' WHEN headers->>'x-nf-request-id' IS NOT NULL THEN 'netlify' WHEN headers->>'x-amz-cf-id' IS NOT NULL THEN 'cloudfront' ELSE headers->>'server' END AS hosting, count(*) AS pages FROM pages WHERE is_html GROUP BY ALL ORDER BY 4 DESC LIMIT 5",
    ),
    (
        "Crawl depth (HTML 200s)",
        "SELECT crawl_depth, count(*) AS pages FROM pages WHERE final_status = 200 AND is_html AND redirect_hops = 0 GROUP BY 1 ORDER BY 1",
    ),
    (
        "Links by position",
        "SELECT position, count(*) AS links, count(DISTINCT to_url) AS unique_targets FROM links WHERE internal GROUP BY 1 ORDER BY 2 DESC",
    ),
    (
        "On-page issues",
        r#"SELECT * FROM (
        SELECT 'missing title' AS issue, count(*) AS pages FROM pages_v WHERE indexability = 'indexable' AND (title IS NULL OR title = '')
        UNION ALL SELECT 'duplicate title', count(*) FROM (SELECT title FROM pages_v WHERE indexability = 'indexable' AND title <> '' GROUP BY title HAVING count(*) > 1)
        UNION ALL SELECT 'missing meta description', count(*) FROM pages_v WHERE indexability = 'indexable' AND (meta_description IS NULL OR meta_description = '')
        UNION ALL SELECT 'missing H1', count(*) FROM pages_v WHERE indexability = 'indexable' AND h1_count = 0
        UNION ALL SELECT 'multiple H1', count(*) FROM pages_v WHERE indexability = 'indexable' AND h1_count > 1
        UNION ALL SELECT 'thin (<200 words)', count(*) FROM pages_v WHERE indexability = 'indexable' AND word_count < 200
        UNION ALL SELECT 'no structured data', count(*) FROM pages_v WHERE indexability = 'indexable' AND jsonld_types = '[]'
        UNION ALL SELECT 'images missing alt', sum(images_missing_alt) FROM pages_v WHERE indexability = 'indexable'
        UNION ALL SELECT 'broken internal links', count(*) FROM broken_links
        UNION ALL SELECT 'links to redirects', count(*) FROM redirected_links
        UNION ALL SELECT 'in sitemap, not linked', count(*) FROM sitemap_only
        UNION ALL SELECT 'non-indexable in sitemap', count(*) FROM pages_v WHERE in_sitemap AND indexability NOT IN ('indexable', 'refused')
    ) ORDER BY pages DESC"#,
    ),
];

pub const ANALYSIS_QUERIES: &[(&str, &str)] = &[
    (
        "Click depth from homepage",
        "SELECT coalesce(click_depth::VARCHAR, 'unreachable') AS depth, count(*) AS pages FROM page_metrics GROUP BY click_depth ORDER BY click_depth NULLS LAST",
    ),
    (
        "Templates (one-off pages summarised last)",
        "SELECT * FROM (SELECT template_id::VARCHAR AS id, pages, pattern, example_url FROM templates WHERE pages > 1 ORDER BY pages DESC LIMIT 15) UNION ALL SELECT '-', count(*), 'one-off pages', NULL FROM templates WHERE pages = 1",
    ),
    (
        "Most linked-to pages (internal PageRank, 1.0 = average)",
        "SELECT round(pagerank, 2) AS pagerank, inlinks, click_depth AS depth, url FROM page_metrics ORDER BY pagerank DESC LIMIT 10",
    ),
    (
        "Header navigation (on ≥50% of pages)",
        "SELECT coalesce(menu_group, '') AS menu_group, anchor, url, coverage FROM global_nav WHERE region = 'header'",
    ),
    (
        "Footer navigation (on ≥50% of pages)",
        "SELECT coalesce(menu_group, '') AS menu_group, anchor, url, coverage FROM global_nav WHERE region = 'footer'",
    ),
    (
        "Top-level URL sections",
        "SELECT path, subtree_pages AS pages, url IS NOT NULL AS has_page FROM tree_url WHERE depth = 1 ORDER BY subtree_pages DESC LIMIT 15",
    ),
    (
        "Orphans (no internal links in)",
        "SELECT m.url, p.discovered_via FROM page_metrics m JOIN pages p USING (url) WHERE is_orphan LIMIT 10",
    ),
    (
        "Near-duplicate content",
        "SELECT dup_group, pages, urls FROM duplicate_content LIMIT 10",
    ),
];
