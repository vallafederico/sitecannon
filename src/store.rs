use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use duckdb::{Connection, params};

use crate::model::{PageRecord, SitemapEntry};

pub const DB_FILE: &str = "site.duckdb";

const SCHEMA: &str = r#"
CREATE TABLE crawl (
    seed VARCHAR, user_agent VARCHAR, started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ,
    robots_status INTEGER, robots_txt VARCHAR, config JSON, stop_reason VARCHAR
);
CREATE TABLE pages (
    url VARCHAR, crawl_depth INTEGER, discovered_via VARCHAR, found_on VARCHAR,
    fetched_at TIMESTAMPTZ, blocked_by_robots BOOLEAN, error VARCHAR,
    status INTEGER, final_status INTEGER, final_url VARCHAR,
    redirect_hops INTEGER, redirect_chain JSON,
    content_type VARCHAR, bytes BIGINT, ttfb_ms INTEGER, total_ms INTEGER, headers JSON,
    is_html BOOLEAN,
    title VARCHAR, title_count INTEGER, meta_description VARCHAR, meta_robots VARCHAR, x_robots_tag VARCHAR,
    canonical VARCHAR, canonical_count INTEGER, lang VARCHAR,
    h1 VARCHAR, h1_count INTEGER, headings JSON, hreflang JSON,
    pagination_next VARCHAR, pagination_prev VARCHAR,
    og JSON, jsonld_types JSON, generator VARCHAR, platform JSON, has_viewport BOOLEAN,
    word_count INTEGER, main_word_count INTEGER, forms INTEGER, iframes JSON,
    links_out_internal INTEGER, links_out_external INTEGER, images INTEGER, images_missing_alt INTEGER,
    skeleton_hash BIGINT, content_hash BIGINT
);
CREATE TABLE page_content (url VARCHAR, main_source VARCHAR, main_markdown VARCHAR, full_markdown VARCHAR, jsonld JSON);
CREATE TABLE links (
    from_url VARCHAR, href VARCHAR, to_url VARCHAR, scheme VARCHAR, internal BOOLEAN,
    anchor VARCHAR, is_image_link BOOLEAN, rel VARCHAR, nofollow BOOLEAN, position VARCHAR, menu_group VARCHAR,
    ordinal INTEGER
);
-- zstd-compressed: raw marketing-site HTML is often 300 KB+ per page.
CREATE TABLE page_html (url VARCHAR, html_zst BLOB);
CREATE TABLE images (page_url VARCHAR, src VARCHAR, alt VARCHAR, alt_missing BOOLEAN);
CREATE TABLE sitemap_entries (url VARCHAR, url_norm VARCHAR, lastmod VARCHAR, sitemap VARCHAR);
"#;

/// Derived views computed at the end of a crawl. Kept as views so they stay correct
/// if tables are later patched (e.g. by a re-crawl of a subset).
const VIEWS: &str = r#"
CREATE OR REPLACE VIEW pages_v AS
SELECT p.*,
    p.url IN (SELECT url_norm FROM sitemap_entries WHERE url_norm IS NOT NULL) AS in_sitemap,
    (p.canonical IS NULL OR p.canonical = p.url) AS canonical_is_self,
    CASE
        WHEN p.blocked_by_robots THEN 'blocked_by_robots'
        WHEN p.status IS NULL THEN 'fetch_error'
        WHEN p.status BETWEEN 300 AND 399 THEN 'redirect'
        WHEN p.final_status IN (401, 403, 429) THEN 'refused'
        WHEN p.status BETWEEN 400 AND 499 THEN 'client_error'
        WHEN p.status >= 500 THEN 'server_error'
        WHEN p.status <> 200 THEN 'non_200'
        WHEN NOT p.is_html THEN 'not_html'
        WHEN lower(coalesce(p.meta_robots, '') || ' ' || coalesce(p.x_robots_tag, '')) LIKE '%noindex%' THEN 'noindex'
        WHEN p.canonical IS NOT NULL AND p.canonical <> p.url THEN 'canonicalised'
        ELSE 'indexable'
    END AS indexability
FROM pages p;

CREATE OR REPLACE VIEW inlinks AS
SELECT to_url AS url,
    count(*) AS inlinks,
    count(DISTINCT from_url) AS unique_inlinks,
    count(DISTINCT from_url) FILTER (WHERE position IN ('main', 'body')) AS contextual_inlinks,
    count(DISTINCT from_url) FILTER (WHERE position IN ('nav', 'header')) AS nav_inlinks,
    count(DISTINCT from_url) FILTER (WHERE position = 'footer') AS footer_inlinks
FROM links WHERE internal AND to_url IS NOT NULL AND to_url <> from_url
GROUP BY 1;

CREATE OR REPLACE VIEW broken_links AS
SELECT l.from_url, l.to_url, l.anchor, l.position, p.status, p.final_status, p.error
FROM links l JOIN pages p ON p.url = l.to_url
WHERE l.internal AND ((p.final_status >= 400 AND p.final_status NOT IN (401, 403, 429)) OR p.error IS NOT NULL);

-- 401/403/429: the server refused the crawler (auth, bot protection, rate limits), not a missing page.
CREATE OR REPLACE VIEW refused_urls AS
SELECT url, final_status AS status, fetched_at FROM pages WHERE final_status IN (401, 403, 429);

CREATE OR REPLACE VIEW redirected_links AS
SELECT l.from_url, l.to_url, p.final_url, p.redirect_hops, l.anchor, l.position
FROM links l JOIN pages p ON p.url = l.to_url
WHERE l.internal AND p.redirect_hops > 0;

CREATE OR REPLACE VIEW sitemap_only AS
SELECT s.url_norm AS url, s.lastmod, s.sitemap
FROM sitemap_entries s
WHERE s.url_norm IS NOT NULL
  AND s.url_norm NOT IN (SELECT to_url FROM links WHERE internal AND to_url IS NOT NULL);
"#;

pub enum Msg {
    Page(Box<PageRecord>),
    Sitemap(Vec<SitemapEntry>),
    /// Why the crawl ended early (e.g. the site started refusing requests).
    StopReason(String),
}

pub struct CrawlMeta {
    pub seed: String,
    pub user_agent: String,
    pub started_at: DateTime<Utc>,
    pub robots_status: Option<u16>,
    pub robots_txt: Option<String>,
    pub config: serde_json::Value,
}

/// Owns the DuckDB connection on a dedicated thread; the async crawler just sends records.
pub struct Writer {
    tx: mpsc::Sender<Msg>,
    handle: JoinHandle<Result<()>>,
}

impl Writer {
    pub fn create(dir: &Path, meta: CrawlMeta) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(DB_FILE);
        let conn = Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT INTO crawl VALUES (?, ?, ?, NULL, ?, ?, ?, NULL)",
            params![
                meta.seed,
                meta.user_agent,
                meta.started_at,
                meta.robots_status.map(i32::from),
                meta.robots_txt,
                meta.config.to_string()
            ],
        )?;

        let (tx, rx) = mpsc::channel::<Msg>();
        let handle = std::thread::spawn(move || write_loop(conn, rx));
        Ok(Self { tx, handle })
    }

    pub fn send(&self, msg: Msg) {
        // The writer only stops on error, which `finish` reports.
        let _ = self.tx.send(msg);
    }

    pub fn finish(self) -> Result<()> {
        drop(self.tx);
        self.handle
            .join()
            .map_err(|_| anyhow::anyhow!("writer thread panicked"))?
    }
}

fn write_loop(conn: Connection, rx: mpsc::Receiver<Msg>) -> Result<()> {
    {
        let mut pages = conn.appender("pages")?;
        let mut content = conn.appender("page_content")?;
        let mut links = conn.appender("links")?;
        let mut images = conn.appender("images")?;
        let mut html = conn.appender("page_html")?;
        let mut sitemap = conn.appender("sitemap_entries")?;
        let mut since_flush = 0usize;

        for msg in rx {
            match msg {
                Msg::Sitemap(entries) => {
                    for e in entries {
                        sitemap.append_row(params![e.url, e.url_norm, e.lastmod, e.sitemap])?;
                    }
                    sitemap.flush()?;
                }
                Msg::StopReason(r) => {
                    conn.execute("UPDATE crawl SET stop_reason = ?", params![r])?;
                }
                Msg::Page(p) => {
                    append_page(&p, &mut pages, &mut content, &mut links, &mut images, &mut html)?;
                    since_flush += 1;
                    if since_flush >= 200 {
                        pages.flush()?;
                        content.flush()?;
                        links.flush()?;
                        images.flush()?;
                        html.flush()?;
                        since_flush = 0;
                    }
                }
            }
        }
    }
    conn.execute_batch(VIEWS)?;
    conn.execute("UPDATE crawl SET finished_at = ?", params![Utc::now()])?;
    conn.execute_batch("CHECKPOINT")?;
    Ok(())
}

/// Appends finished page records outside a crawl (used by `reextract`).
/// `html_table` names the table raw HTML goes into.
pub fn append_pages(conn: &Connection, records: &[PageRecord], html_table: &str) -> Result<()> {
    let mut pages = conn.appender("pages")?;
    let mut content = conn.appender("page_content")?;
    let mut links = conn.appender("links")?;
    let mut images = conn.appender("images")?;
    let mut html = conn.appender(html_table)?;
    for p in records {
        append_page(p, &mut pages, &mut content, &mut links, &mut images, &mut html)?;
    }
    Ok(())
}

pub const PAGE_HTML_SCHEMA: &str = "CREATE TABLE page_html (url VARCHAR, html_zst BLOB);";
pub const VIEWS_SQL: &str = VIEWS;

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn append_page(
    p: &PageRecord,
    pages: &mut duckdb::Appender,
    content: &mut duckdb::Appender,
    links: &mut duckdb::Appender,
    images: &mut duckdb::Appender,
    html: &mut duckdb::Appender,
) -> Result<()> {
    let x = p.extracted.as_ref();
    let is_html = p
        .content_type
        .as_deref()
        .is_some_and(|c| c.contains("text/html") || c.contains("application/xhtml"));
    let x_robots = p
        .headers
        .get("x-robots-tag")
        .and_then(|v| v.as_str())
        .map(String::from);
    let opt_i32 = |v: Option<usize>| v.map(|n| n as i32);

    pages.append_row(params![
        p.url,
        p.crawl_depth as i32,
        p.discovered_via,
        p.found_on,
        p.fetched_at,
        p.blocked_by_robots,
        p.error,
        p.status.map(i32::from),
        p.final_status.map(i32::from),
        p.final_url,
        p.redirect_chain.len().saturating_sub(1) as i32,
        (!p.redirect_chain.is_empty()).then(|| json(&p.redirect_chain)),
        p.content_type,
        p.bytes.map(|b| b as i64),
        p.ttfb_ms.map(|v| v as i32),
        p.total_ms.map(|v| v as i32),
        json(&p.headers),
        is_html,
        x.and_then(|x| x.title.clone()),
        opt_i32(x.map(|x| x.title_count)),
        x.and_then(|x| x.meta_description.clone()),
        x.and_then(|x| x.meta_robots.clone()),
        x_robots,
        x.and_then(|x| x.canonical.clone()),
        opt_i32(x.map(|x| x.canonical_count)),
        x.and_then(|x| x.lang.clone()),
        x.and_then(|x| x.h1.first().cloned()),
        opt_i32(x.map(|x| x.h1.len())),
        x.map(|x| json(
            &x.headings
                .iter()
                .map(|(l, t)| serde_json::json!({"level": l, "text": t}))
                .collect::<Vec<_>>()
        )),
        x.map(|x| json(
            &x.hreflang
                .iter()
                .map(|(l, h)| serde_json::json!({"lang": l, "href": h}))
                .collect::<Vec<_>>()
        )),
        x.and_then(|x| x.pagination_next.clone()),
        x.and_then(|x| x.pagination_prev.clone()),
        x.map(|x| json(&x.og)),
        x.map(|x| json(&x.jsonld_types)),
        x.and_then(|x| x.generator.clone()),
        x.map(|x| json(&x.platform)),
        x.map(|x| x.has_viewport),
        opt_i32(x.map(|x| x.word_count)),
        opt_i32(x.map(|x| x.main_word_count)),
        opt_i32(x.map(|x| x.forms)),
        x.map(|x| json(&x.iframes)),
        opt_i32(x.map(|x| x.links.iter().filter(|l| l.internal).count())),
        opt_i32(x.map(|x| {
            x.links
                .iter()
                .filter(|l| !l.internal && l.target.is_some())
                .count()
        })),
        opt_i32(x.map(|x| x.images.len())),
        opt_i32(x.map(|x| x.images.iter().filter(|i| i.alt.is_none()).count())),
        x.map(|x| x.skeleton_hash as i64),
        x.map(|x| x.content_hash as i64),
    ])?;

    let Some(x) = x else { return Ok(()) };
    content.append_row(params![
        p.url,
        x.main_source,
        x.main_markdown,
        x.full_markdown,
        (!x.jsonld.is_empty()).then(|| json(&x.jsonld)),
    ])?;
    for (i, l) in x.links.iter().enumerate() {
        links.append_row(params![
            p.url,
            l.href,
            l.target,
            l.scheme,
            l.internal,
            l.anchor,
            l.is_image_link,
            l.rel,
            l.nofollow,
            l.position.as_str(),
            l.menu_group,
            i as i32,
        ])?;
    }
    if let Some(h) = &x.html {
        html.append_row(params![p.url, zstd::encode_all(h.as_bytes(), 3)?])?;
    }
    for img in &x.images {
        images.append_row(params![p.url, img.src, img.alt, img.alt.is_none()])?;
    }
    Ok(())
}

pub fn open_readonly(dir: &Path) -> Result<Connection> {
    let path = if dir.is_dir() {
        dir.join(DB_FILE)
    } else {
        dir.to_path_buf()
    };
    anyhow::ensure!(path.exists(), "no crawl database at {}", path.display());
    let cfg = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
    Ok(Connection::open_with_flags(&path, cfg)?)
}
