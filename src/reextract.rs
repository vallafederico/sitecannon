//! Re-runs HTML extraction over a project's stored HTML, so extractor improvements apply to
//! existing crawls without hitting the site again. HTTP facts (status, headers, redirects,
//! timings) are kept as crawled; everything derived from the HTML is rebuilt.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use duckdb::{Connection, params};
use url::Url;

use crate::extract;
use crate::model::{Hop, PageRecord};
use crate::store::{self, DB_FILE};
use crate::urlnorm::Scope;

const BATCH: usize = 400;

fn via(s: &str) -> &'static str {
    match s {
        "seed" => "seed",
        "sitemap" => "sitemap",
        "redirect" => "redirect",
        "canonical" => "canonical",
        "hreflang" => "hreflang",
        "pagination" => "pagination",
        _ => "link",
    }
}

struct Row {
    record: PageRecord,
    html: String,
}

pub fn run(project: &Path) -> Result<usize> {
    let path = if project.is_dir() {
        project.join(DB_FILE)
    } else {
        project.to_path_buf()
    };
    anyhow::ensure!(path.exists(), "no crawl database at {}", path.display());
    let conn = Connection::open(&path)?;

    let seed: String = conn.query_row("SELECT seed FROM crawl", [], |r| r.get(0))?;
    let seed_url = Url::parse(&seed)?;
    let scope = Scope::new(&seed_url);
    let compressed: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.columns WHERE table_name = 'page_html' AND column_name = 'html_zst'",
        [],
        |r| r.get(0),
    )?;
    let html_expr = if compressed {
        "h.html_zst"
    } else {
        "encode(h.html)"
    };
    // Older crawls stored plain text; rebuild into the compressed table and swap at the end.
    let html_table = if compressed { "page_html" } else { "page_html_new" };
    if !compressed {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS page_html_new; {}",
            store::PAGE_HTML_SCHEMA.replace("page_html", "page_html_new")
        ))?;
    }

    let urls: Vec<String> = {
        let mut stmt = conn.prepare("SELECT url FROM page_html ORDER BY url")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let total = urls.len();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);

    for (b, chunk) in urls.chunks(BATCH).enumerate() {
        conn.execute_batch("CREATE OR REPLACE TEMP TABLE batch_urls (url VARCHAR)")?;
        {
            let mut app = conn.appender("batch_urls")?;
            for u in chunk {
                app.append_row(params![u])?;
            }
        }
        let rows = load_batch(&conn, html_expr)?;

        // Extraction is CPU-bound; spread the batch across cores.
        let per = rows.len().div_ceil(threads).max(1);
        let records: Vec<PageRecord> = std::thread::scope(|s| {
            let handles: Vec<_> = rows
                .chunks(per)
                .map(|part| {
                    let (scope, seed_url) = (&scope, &seed_url);
                    s.spawn(move || {
                        part.iter()
                            .map(|row| {
                                let base = Url::parse(&row.record.url).unwrap_or_else(|_| seed_url.clone());
                                let mut x = extract::extract(&row.html, &base, scope);
                                x.html = Some(row.html.clone());
                                PageRecord {
                                    extracted: Some(x),
                                    ..row.record.clone()
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        });

        conn.execute_batch(&format!(
            "DELETE FROM links WHERE from_url IN (SELECT url FROM batch_urls);
             DELETE FROM images WHERE page_url IN (SELECT url FROM batch_urls);
             DELETE FROM page_content WHERE url IN (SELECT url FROM batch_urls);
             DELETE FROM pages WHERE url IN (SELECT url FROM batch_urls);
             {}",
            if compressed {
                "DELETE FROM page_html WHERE url IN (SELECT url FROM batch_urls);"
            } else {
                ""
            }
        ))?;
        store::append_pages(&conn, &records, html_table)?;
        eprint!(
            "\rre-extracted {} / {total}",
            (b * BATCH + chunk.len()).min(total)
        );
    }
    eprintln!();

    if !compressed {
        conn.execute_batch("DROP TABLE page_html; ALTER TABLE page_html_new RENAME TO page_html;")?;
    }
    conn.execute_batch(store::VIEWS_SQL)?;
    conn.execute_batch("CHECKPOINT")?;
    Ok(total)
}

fn load_batch(conn: &Connection, html_expr: &str) -> Result<Vec<Row>> {
    let sql = format!(
        "SELECT p.url, p.crawl_depth, p.discovered_via, p.found_on, p.fetched_at, p.error,
                p.status, p.final_status, p.final_url, p.redirect_chain, p.content_type, p.bytes,
                p.ttfb_ms, p.total_ms, p.headers, {html_expr}
         FROM pages p JOIN page_html h USING (url)
         WHERE p.url IN (SELECT url FROM batch_urls)"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let blob: Vec<u8> = r.get(15)?;
        Ok((
            PageRecord {
                url: r.get(0)?,
                crawl_depth: r.get::<_, Option<i32>>(1)?.unwrap_or(0) as u32,
                discovered_via: via(&r.get::<_, String>(2)?),
                found_on: r.get(3)?,
                fetched_at: r.get::<_, DateTime<Utc>>(4)?,
                blocked_by_robots: false,
                error: r.get(5)?,
                status: r.get::<_, Option<i32>>(6)?.map(|v| v as u16),
                final_status: r.get::<_, Option<i32>>(7)?.map(|v| v as u16),
                final_url: r.get(8)?,
                redirect_chain: r
                    .get::<_, Option<String>>(9)?
                    .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|h| {
                        Some(Hop {
                            url: h.get("url")?.as_str()?.to_string(),
                            status: h.get("status")?.as_u64()? as u16,
                        })
                    })
                    .collect(),
                content_type: r.get(10)?,
                bytes: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                ttfb_ms: r.get::<_, Option<i32>>(12)?.map(|v| v as u32),
                total_ms: r.get::<_, Option<i32>>(13)?.map(|v| v as u32),
                headers: r
                    .get::<_, Option<String>>(14)?
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                extracted: None,
            },
            blob,
        ))
    })?;
    let compressed = html_expr.contains("zst");
    let mut out = Vec::new();
    for row in rows {
        let (record, blob) = row?;
        let html = if compressed {
            String::from_utf8(zstd::decode_all(&blob[..]).context("decompressing stored HTML")?)?
        } else {
            String::from_utf8(blob)?
        };
        out.push(Row { record, html });
    }
    Ok(out)
}
