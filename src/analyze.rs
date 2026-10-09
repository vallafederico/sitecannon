//! Structural analysis over a finished crawl: link graph metrics, click-depth tree,
//! URL tree, template clusters and near-duplicate content. Everything is written back
//! into the project database as tables/views so reports and agents can query it.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::{Context, Result};
use duckdb::{Connection, params};
use url::Url;

use crate::content;
use crate::extract;
use crate::store::DB_FILE;
use crate::templates;
use crate::urlnorm::normalize;

const PAGERANK_DAMPING: f64 = 0.85;
/// Content SimHash comparisons ignore pages with less main content than this.
const DUP_MIN_WORDS: i32 = 50;
const SKELETON_FALLBACK_BITS: u32 = 6;

pub struct Options {
    /// Min layout similarity (Jaccard, 0–1) for a page to join a template.
    pub template_similarity: f64,
    /// Max differing bits between content SimHashes for two pages to count as near-duplicates.
    pub duplicate_bits: u32,
}

pub struct Summary {
    pub homepage: String,
    pub pages: usize,
    pub unreachable: usize,
    pub max_depth: u32,
    pub orphans: usize,
    pub dead_ends: usize,
    pub templates: usize,
    pub templated_pages: usize,
    pub one_off_pages: usize,
    pub dup_groups: usize,
    pub topics: usize,
}

struct Page {
    url: String,
    skeleton: Option<u64>,
    content: Option<u64>,
    indexable: bool,
    word_count: i32,
}

const ANALYSIS_SQL: &str = r#"
DROP TABLE IF EXISTS page_metrics;
DROP TABLE IF EXISTS templates;
DROP TABLE IF EXISTS redirect_map;
DROP TABLE IF EXISTS tree_url;
CREATE TABLE redirect_map (url VARCHAR, resolved_url VARCHAR);
CREATE TABLE page_metrics (
    url VARCHAR, click_depth INTEGER, click_parent VARCHAR,
    inlinks INTEGER, outlinks INTEGER, pagerank DOUBLE,
    template_id INTEGER, dup_group INTEGER, is_orphan BOOLEAN, is_dead_end BOOLEAN
);
CREATE TABLE templates (template_id INTEGER, pages INTEGER, pattern VARCHAR, example_url VARCHAR, skeleton_hash BIGINT);
CREATE TABLE tree_url (
    node_id INTEGER, parent_id INTEGER, segment VARCHAR, path VARCHAR, url VARCHAR,
    depth INTEGER, pages INTEGER, subtree_pages INTEGER
);
"#;

const ANALYSIS_VIEWS: &str = r#"
CREATE OR REPLACE VIEW nav_items AS
WITH l AS (
    SELECT l.from_url, coalesce(r.resolved_url, l.to_url) AS url, l.anchor, l.menu_group, l.ordinal,
        CASE WHEN l.position IN ('nav', 'header') THEN 'header' ELSE 'footer' END AS region
    FROM links l LEFT JOIN redirect_map r ON r.url = l.to_url
    WHERE l.internal AND l.to_url IS NOT NULL AND l.position IN ('nav', 'header', 'footer')
      AND l.from_url IN (SELECT url FROM page_metrics)
)
SELECT region, menu_group, anchor, url,
    count(DISTINCT from_url) AS pages,
    round(count(DISTINCT from_url) / (SELECT count(*) FROM page_metrics), 3) AS coverage,
    median(ordinal) AS typical_ordinal
FROM l GROUP BY ALL;

-- Links present on at least half the pages: the site's persistent header and footer navigation.
CREATE OR REPLACE VIEW global_nav AS
SELECT * FROM nav_items WHERE coverage >= 0.5 ORDER BY region DESC, typical_ordinal;

CREATE OR REPLACE VIEW pages_full AS
SELECT p.*, m.* EXCLUDE (url), t.pattern AS template_pattern
FROM pages_v p JOIN page_metrics m USING (url) LEFT JOIN templates t USING (template_id);

CREATE OR REPLACE VIEW duplicate_content AS
SELECT dup_group, count(*) AS pages, list(url ORDER BY pagerank DESC) AS urls
FROM page_metrics WHERE dup_group IS NOT NULL GROUP BY 1 ORDER BY 2 DESC;
"#;

pub fn run(project: &Path, opts: &Options) -> Result<Summary> {
    let path = if project.is_dir() {
        project.join(DB_FILE)
    } else {
        project.to_path_buf()
    };
    anyhow::ensure!(path.exists(), "no crawl database at {}", path.display());
    let conn = Connection::open(&path)?;
    // Bring crawls made by older versions up to the current schema and view definitions.
    conn.execute_batch("ALTER TABLE crawl ADD COLUMN IF NOT EXISTS stop_reason VARCHAR;")?;
    conn.execute_batch(crate::store::VIEWS_SQL)?;

    let seed: String = conn.query_row("SELECT seed FROM crawl", [], |r| r.get(0))?;

    // Redirects: requested URL → normalised destination.
    let mut redirects: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT url, final_url FROM pages WHERE redirect_hops > 0 AND final_url IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (from, to) = row?;
            if let Some(n) = Url::parse(&to).ok().and_then(|u| normalize(&u)) {
                redirects.insert(from, n.to_string());
            }
        }
    }
    let resolve = |u: &str| -> String {
        let mut cur = u.to_string();
        for _ in 0..10 {
            match redirects.get(&cur) {
                Some(next) if *next != cur => cur = next.clone(),
                _ => break,
            }
        }
        cur
    };

    // Content pages: HTML that answered 200 without redirecting. These are the graph's nodes.
    let mut pages: Vec<Page> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT p.url, p.skeleton_hash, c.main_markdown, p.indexability = 'indexable', coalesce(p.main_word_count, 0)
             FROM pages_v p LEFT JOIN page_content c USING (url)
             WHERE p.final_status = 200 AND p.is_html AND p.redirect_hops = 0 ORDER BY p.url",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Page {
                url: r.get(0)?,
                skeleton: r.get::<_, Option<i64>>(1)?.map(|v| v as u64),
                content: r
                    .get::<_, Option<String>>(2)?
                    .map(|md| extract::content_simhash(&md)),
                indexable: r.get(3)?,
                word_count: r.get(4)?,
            })
        })?;
        for p in rows {
            pages.push(p?);
        }
    }
    anyhow::ensure!(!pages.is_empty(), "no HTML pages with status 200 in this crawl");
    let index: HashMap<&str, usize> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.url.as_str(), i))
        .collect();
    let n = pages.len();

    let homepage = Url::parse(&seed)
        .ok()
        .and_then(|u| normalize(&u))
        .map(|u| resolve(u.as_str()));
    let home = homepage
        .as_deref()
        .and_then(|h| index.get(h).copied())
        .with_context(|| format!("homepage {seed} was not crawled as an HTML 200 page"))?;

    // Deduplicated internal edges between content pages, redirects resolved.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut inn: Vec<Vec<usize>> = vec![Vec::new(); n];
    {
        let mut seen_edges = HashSet::new();
        let mut stmt =
            conn.prepare("SELECT from_url, to_url FROM links WHERE internal AND to_url IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (from, to) = row?;
            let (Some(&a), Some(&b)) = (index.get(from.as_str()), index.get(resolve(&to).as_str())) else {
                continue;
            };
            if a != b && seen_edges.insert((a, b)) {
                out[a].push(b);
                inn[b].push(a);
            }
        }
    }

    let depth = bfs_depth(&out, home);
    let pr = pagerank(&out, n);
    let click_parent: Vec<Option<usize>> = (0..n)
        .map(|v| {
            let d = depth[v]?;
            if d == 0 {
                return None;
            }
            inn[v]
                .iter()
                .copied()
                .filter(|&u| depth[u] == Some(d - 1))
                .max_by(|&a, &b| pr[a].total_cmp(&pr[b]).then(b.cmp(&a)))
        })
        .collect();

    let htmls = load_htmls(&conn, &pages)?;
    let templates = detect_templates(&htmls, &pages, &pr, opts.template_similarity);
    let dup_input: Vec<Option<u64>> = pages
        .iter()
        .map(|p| p.content.filter(|_| p.indexable && p.word_count >= DUP_MIN_WORDS))
        .collect();
    let dup_clusters: Vec<Vec<usize>> = cluster(&dup_input, opts.duplicate_bits, &pr)
        .into_iter()
        .filter(|c| c.len() > 1)
        .collect();

    let mut template_of = vec![None; n];
    for (tid, members) in templates.iter().enumerate() {
        for &m in members {
            template_of[m] = Some(tid as i32 + 1);
        }
    }
    let mut dup_of = vec![None; n];
    for (gid, members) in dup_clusters.iter().enumerate() {
        for &m in members {
            dup_of[m] = Some(gid as i32 + 1);
        }
    }

    conn.execute_batch(ANALYSIS_SQL)?;
    {
        let mut app = conn.appender("redirect_map")?;
        for from in redirects.keys() {
            app.append_row(params![from, resolve(from)])?;
        }
    }
    let (mut orphans, mut dead_ends) = (0, 0);
    {
        let mut app = conn.appender("page_metrics")?;
        for (i, p) in pages.iter().enumerate() {
            let is_orphan = i != home && inn[i].is_empty();
            let is_dead_end = out[i].is_empty();
            orphans += is_orphan as usize;
            dead_ends += is_dead_end as usize;
            app.append_row(params![
                p.url,
                depth[i].map(|d| d as i32),
                click_parent[i].map(|j| pages[j].url.clone()),
                inn[i].len() as i32,
                out[i].len() as i32,
                pr[i] * n as f64,
                template_of[i],
                dup_of[i],
                is_orphan,
                is_dead_end,
            ])?;
        }
    }
    {
        let mut app = conn.appender("templates")?;
        for (tid, members) in templates.iter().enumerate() {
            let urls: Vec<&str> = members.iter().map(|&m| pages[m].url.as_str()).collect();
            let example = members[0];
            app.append_row(params![
                tid as i32 + 1,
                members.len() as i32,
                url_pattern(&urls),
                pages[example].url,
                pages[example].skeleton.map(|h| h as i64),
            ])?;
        }
    }
    write_url_tree(&conn, &pages)?;
    let urls: Vec<&str> = pages.iter().map(|p| p.url.as_str()).collect();
    let topics = content::run(&conn, &urls, &htmls, &template_of)?;
    conn.execute_batch(ANALYSIS_VIEWS)?;
    conn.execute_batch("CHECKPOINT")?;

    Ok(Summary {
        homepage: pages[home].url.clone(),
        pages: n,
        unreachable: depth.iter().filter(|d| d.is_none()).count(),
        max_depth: depth.iter().flatten().copied().max().unwrap_or(0),
        orphans,
        dead_ends,
        templates: templates.iter().filter(|t| t.len() > 1).count(),
        templated_pages: templates.iter().filter(|t| t.len() > 1).map(Vec::len).sum(),
        one_off_pages: templates.iter().filter(|t| t.len() == 1).count(),
        dup_groups: dup_clusters.len(),
        topics,
    })
}

/// Clusters pages by main-content layout from stored HTML; falls back to the crawl-time
/// skeleton SimHash when the crawl was run with `--no-html`.
/// Stored HTML for each page (aligned with `pages`; empty when the crawl used `--no-html`).
fn load_htmls(conn: &Connection, pages: &[Page]) -> Result<Vec<String>> {
    let mut html_by_url: HashMap<String, String> = HashMap::new();
    // Crawls before zstd storage kept plain text in `html`.
    let compressed: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.columns WHERE table_name = 'page_html' AND column_name = 'html_zst'",
        [],
        |r| r.get(0),
    )?;
    if compressed {
        let mut stmt = conn.prepare("SELECT url, html_zst FROM page_html")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (u, z) = row?;
            let html = zstd::decode_all(&z[..])
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            html_by_url.insert(u, html);
        }
    } else {
        let mut stmt = conn.prepare("SELECT url, html FROM page_html")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (u, h) = row?;
            html_by_url.insert(u, h);
        }
    }
    Ok(pages
        .iter()
        .map(|p| html_by_url.remove(&p.url).unwrap_or_default())
        .collect())
}

/// Clusters pages by main-content layout from stored HTML; falls back to the crawl-time
/// skeleton SimHash when the crawl was run with `--no-html`.
fn detect_templates(htmls: &[String], pages: &[Page], pr: &[f64], similarity: f64) -> Vec<Vec<usize>> {
    if htmls.iter().all(String::is_empty) {
        let hashes: Vec<Option<u64>> = pages.iter().map(|p| p.skeleton).collect();
        return cluster(&hashes, SKELETON_FALLBACK_BITS, pr);
    }
    let features = templates::layout_features_parallel(htmls);
    // Most-linked pages found templates first, so each template's reference page is a prominent one.
    let mut order: Vec<usize> = (0..pages.len()).collect();
    order.sort_by(|&a, &b| pr[b].total_cmp(&pr[a]).then(a.cmp(&b)));
    let mut clusters = templates::cluster(&features, &order, similarity);
    for c in &mut clusters {
        c.sort_by(|&a, &b| pr[b].total_cmp(&pr[a]).then(a.cmp(&b)));
    }
    clusters
}

fn bfs_depth(out: &[Vec<usize>], start: usize) -> Vec<Option<u32>> {
    let mut depth = vec![None; out.len()];
    depth[start] = Some(0);
    let mut q = VecDeque::from([start]);
    while let Some(u) = q.pop_front() {
        let d = depth[u].unwrap_or(0);
        for &v in &out[u] {
            if depth[v].is_none() {
                depth[v] = Some(d + 1);
                q.push_back(v);
            }
        }
    }
    depth
}

/// Internal PageRank. Dangling pages spread their score evenly, so the result sums to 1.
fn pagerank(out: &[Vec<usize>], n: usize) -> Vec<f64> {
    let mut pr = vec![1.0 / n as f64; n];
    for _ in 0..100 {
        let mut next = vec![(1.0 - PAGERANK_DAMPING) / n as f64; n];
        let mut dangling = 0.0;
        for (i, targets) in out.iter().enumerate() {
            if targets.is_empty() {
                dangling += pr[i];
            } else {
                let share = PAGERANK_DAMPING * pr[i] / targets.len() as f64;
                for &j in targets {
                    next[j] += share;
                }
            }
        }
        let spread = PAGERANK_DAMPING * dangling / n as f64;
        next.iter_mut().for_each(|v| *v += spread);
        let delta: f64 = next.iter().zip(&pr).map(|(a, b)| (a - b).abs()).sum();
        pr = next;
        if delta < 1e-10 {
            break;
        }
    }
    pr
}

/// Groups items whose hashes differ by at most `max_bits` (single-linkage over distinct hashes).
/// Clusters come back largest first; members within a cluster by descending PageRank.
fn cluster(hashes: &[Option<u64>], max_bits: u32, pr: &[f64]) -> Vec<Vec<usize>> {
    let mut distinct: Vec<u64> = hashes.iter().flatten().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    let pos: HashMap<u64, usize> = distinct.iter().enumerate().map(|(i, &h)| (h, i)).collect();

    let mut parent: Vec<usize> = (0..distinct.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for i in 0..distinct.len() {
        for j in (i + 1)..distinct.len() {
            if (distinct[i] ^ distinct[j]).count_ones() <= max_bits {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (item, h) in hashes.iter().enumerate() {
        if let Some(h) = h {
            let root = find(&mut parent, pos[h]);
            groups.entry(root).or_default().push(item);
        }
    }
    let mut clusters: Vec<Vec<usize>> = groups.into_values().collect();
    for c in &mut clusters {
        c.sort_by(|&a, &b| pr[b].total_cmp(&pr[a]).then(a.cmp(&b)));
    }
    clusters.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    clusters
}

fn segments(u: &str) -> Vec<String> {
    Url::parse(u)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .map(|s| s.filter(|x| !x.is_empty()).map(String::from).collect())
        })
        .unwrap_or_default()
}

/// Describes a set of URLs by their path shapes, most common first, e.g.
/// `/*/{analysis|interview|news…}/* ×637 · /* ×116`. Positions with a handful of distinct
/// values list them (that's usually a content type or section); many values become `*`.
fn url_pattern(urls: &[&str]) -> String {
    const MAX_LISTED: usize = 20;
    const SHOWN: usize = 3;
    let mut by_len: BTreeMap<usize, Vec<Vec<String>>> = BTreeMap::new();
    for u in urls {
        let segs = segments(u);
        by_len.entry(segs.len()).or_default().push(segs);
    }
    let mut shapes: Vec<(String, usize)> = by_len
        .into_iter()
        .map(|(len, paths)| {
            let shape: Vec<String> = (0..len)
                .map(|i| {
                    let mut values: Vec<&str> = paths.iter().map(|p| p[i].as_str()).collect();
                    values.sort_unstable();
                    values.dedup();
                    match values.len() {
                        1 => values[0].to_string(),
                        n if n <= MAX_LISTED && n < paths.len() => {
                            let mut counts: Vec<(&str, usize)> = values
                                .iter()
                                .map(|v| (*v, paths.iter().filter(|p| p[i] == *v).count()))
                                .collect();
                            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                            let shown: Vec<&str> = counts.iter().take(SHOWN).map(|(v, _)| *v).collect();
                            let more = if n > SHOWN { "…" } else { "" };
                            format!("{{{}{more}}}", shown.join("|"))
                        }
                        _ => "*".to_string(),
                    }
                })
                .collect();
            (format!("/{}", shape.join("/")), paths.len())
        })
        .collect();
    shapes.sort_by_key(|s| std::cmp::Reverse(s.1));
    if shapes.len() == 1 {
        return shapes.remove(0).0;
    }
    let mut parts: Vec<String> = shapes
        .iter()
        .take(SHOWN)
        .map(|(s, n)| format!("{s} ×{n}"))
        .collect();
    if shapes.len() > SHOWN {
        parts.push(format!("+{} more shapes", shapes.len() - SHOWN));
    }
    parts.join(" · ")
}

struct TreeNode {
    parent: Option<usize>,
    segment: String,
    path: String,
    url: Option<String>,
    depth: usize,
    pages: usize,
    children: BTreeMap<String, usize>,
}

/// The site as its URLs describe it: one node per path segment, with intermediate
/// folders that have no page of their own (a common IA smell) kept as empty nodes.
fn write_url_tree(conn: &Connection, pages: &[Page]) -> Result<()> {
    let mut nodes = vec![TreeNode {
        parent: None,
        segment: "/".into(),
        path: "/".into(),
        url: None,
        depth: 0,
        pages: 0,
        children: BTreeMap::new(),
    }];
    for p in pages {
        let mut cur = 0;
        for seg in segments(&p.url) {
            cur = match nodes[cur].children.get(&seg) {
                Some(&c) => c,
                None => {
                    let id = nodes.len();
                    let path = format!("{}{}/", nodes[cur].path, seg);
                    let depth = nodes[cur].depth + 1;
                    nodes.push(TreeNode {
                        parent: Some(cur),
                        segment: seg.clone(),
                        path,
                        url: None,
                        depth,
                        pages: 0,
                        children: BTreeMap::new(),
                    });
                    nodes[cur].children.insert(seg, id);
                    id
                }
            };
        }
        nodes[cur].pages += 1;
        // Prefer the query-less URL as the node's representative page.
        let has_query = p.url.contains('?');
        if nodes[cur]
            .url
            .as_deref()
            .is_none_or(|u| u.contains('?') && !has_query)
        {
            nodes[cur].url = Some(p.url.clone());
        }
    }
    // Children are always created after their parent, so a reverse pass accumulates subtrees.
    let mut subtree: Vec<usize> = nodes.iter().map(|n| n.pages).collect();
    for i in (1..nodes.len()).rev() {
        if let Some(p) = nodes[i].parent {
            subtree[p] += subtree[i];
        }
    }
    let mut app = conn.appender("tree_url")?;
    for (i, n) in nodes.iter().enumerate() {
        app.append_row(params![
            i as i32,
            n.parent.map(|p| p as i32),
            n.segment,
            n.path,
            n.url,
            n.depth as i32,
            n.pages as i32,
            subtree[i] as i32,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerank_favours_linked_pages_and_sums_to_one() {
        // 0 → 1, 0 → 2, 1 → 2, 2 → 0
        let out = vec![vec![1, 2], vec![2], vec![0]];
        let pr = pagerank(&out, 3);
        assert!((pr.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(pr[2] > pr[1]);
    }

    #[test]
    fn clusters_by_hamming_distance() {
        let hashes = [
            Some(0b0000u64),
            Some(0b0001),
            Some(0b1111_0000),
            None,
            Some(0b0011),
        ];
        let c = cluster(&hashes, 1, &[0.0; 5]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], vec![0, 1, 4]);
        assert_eq!(c[1], vec![2]);
    }

    #[test]
    fn patterns_describe_url_sets() {
        assert_eq!(
            url_pattern(&["https://a.com/blog/one", "https://a.com/blog/two"]),
            "/blog/*"
        );
        assert_eq!(
            url_pattern(&[
                "https://a.com/ai/news/x",
                "https://a.com/hr/news/y",
                "https://a.com/ai/opinion/z",
                "https://a.com/ai",
            ]),
            "/{ai|hr}/{news|opinion}/* ×3 · /ai ×1"
        );
    }
}
