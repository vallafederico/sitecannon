use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use tokio::task::JoinSet;
use url::Url;

use crate::discover;
use crate::extract;
use crate::model::{Extracted, Hop, PageRecord};
use crate::store::{CrawlMeta, Msg, Writer};
use crate::urlnorm::{Scope, normalize};

const MAX_REDIRECTS: usize = 10;
const MAX_HTML_BYTES: u64 = 15 * 1024 * 1024;
const MAX_RETRIES: u8 = 2;
const MAX_INTERVAL: Duration = Duration::from_secs(10);
/// Consecutive refusals (401/403/429) after which the crawl stops: the site is blocking us.
const REFUSAL_STREAK_LIMIT: usize = 20;

/// Response headers worth keeping: indexing directives, caching, and stack/CDN fingerprints.
const KEPT_HEADERS: &[&str] = &[
    "x-robots-tag",
    "last-modified",
    "cache-control",
    "etag",
    "age",
    "server",
    "x-powered-by",
    "content-language",
    "link",
    "via",
    "x-cache",
    "cf-cache-status",
    "cf-ray",
    "x-vercel-id",
    "x-nf-request-id",
    "x-served-by",
    "x-amz-cf-id",
    "x-generator",
    "strict-transport-security",
];

pub struct CrawlConfig {
    pub seed: Url,
    pub max_pages: usize,
    pub max_depth: u32,
    pub concurrency: usize,
    pub rps: f64,
    pub user_agent: String,
    pub robots_agent: String,
    pub respect_robots: bool,
    pub use_sitemaps: bool,
    pub max_query_variants: usize,
    pub timeout: Duration,
    pub keep_html: bool,
}

#[derive(Clone)]
struct Task {
    url: Url,
    depth: u32,
    via: &'static str,
    found_on: Option<String>,
    retries: u8,
}

struct Fetched {
    task: Task,
    fetched_at: chrono::DateTime<Utc>,
    hops: Vec<Hop>,
    final_url: Url,
    final_status: Option<u16>,
    content_type: Option<String>,
    bytes: Option<u64>,
    ttfb_ms: Option<u32>,
    total_ms: u32,
    headers: serde_json::Map<String, serde_json::Value>,
    error: Option<String>,
    extracted: Option<Extracted>,
}

pub struct CrawlStats {
    pub pages: usize,
    pub sitemap_urls: usize,
    pub elapsed: Duration,
}

pub async fn run(cfg: CrawlConfig, out_dir: &std::path::Path) -> Result<CrawlStats> {
    let started = Instant::now();
    let client = reqwest::Client::builder()
        .user_agent(&cfg.user_agent)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(cfg.timeout)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(cfg.concurrency)
        .build()?;
    // robots.txt and sitemaps are commonly redirected (www → apex, http → https); follow those.
    let discovery_client = reqwest::Client::builder()
        .user_agent(&cfg.user_agent)
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(cfg.timeout)
        .build()?;
    let scope = Arc::new(Scope::new(&cfg.seed));

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner} {elapsed_precise} {msg}")?);
    pb.enable_steady_tick(Duration::from_millis(120));

    pb.set_message("reading robots.txt");
    let robots = discover::fetch_robots(&discovery_client, &cfg.seed, &cfg.robots_agent).await;

    let mut interval = Duration::from_secs_f64(1.0 / cfg.rps.max(0.01));
    if let Some(delay) = robots.robot.as_ref().and_then(|r| r.delay)
        && cfg.respect_robots
    {
        interval = interval.max(Duration::from_secs_f32(delay));
    }

    let base_interval = interval;

    let writer = Writer::create(
        out_dir,
        CrawlMeta {
            seed: cfg.seed.to_string(),
            user_agent: cfg.user_agent.clone(),
            started_at: Utc::now(),
            robots_status: robots.status,
            robots_txt: robots.raw.clone(),
            config: serde_json::json!({
                "max_pages": cfg.max_pages, "max_depth": cfg.max_depth,
                "concurrency": cfg.concurrency, "rps": cfg.rps,
                "respect_robots": cfg.respect_robots, "use_sitemaps": cfg.use_sitemaps,
                "max_query_variants": cfg.max_query_variants,
                "effective_interval_ms": interval.as_millis() as u64,
            }),
        },
    )?;

    let mut sitemap_q: VecDeque<Task> = VecDeque::new();
    let mut sitemap_urls = 0;
    if cfg.use_sitemaps {
        pb.set_message("reading XML sitemaps");
        let entries = discover::fetch_sitemaps(&discovery_client, &cfg.seed, &robots.sitemaps).await;
        sitemap_urls = entries.len();
        for e in &entries {
            let Some(u) = e.url_norm.as_deref().and_then(|s| Url::parse(s).ok()) else {
                continue;
            };
            if scope.is_internal(&u) {
                sitemap_q.push_back(Task {
                    url: u,
                    depth: 0,
                    via: "sitemap",
                    found_on: Some(e.sitemap.clone()),
                    retries: 0,
                });
            }
        }
        writer.send(Msg::Sitemap(entries));
    }

    let robot = if cfg.respect_robots { robots.robot } else { None };
    let mut seen: HashSet<String> = HashSet::new();
    let mut query_variants: HashMap<String, usize> = HashMap::new();
    let mut link_q: VecDeque<Task> = VecDeque::new();
    let seed = normalize(&cfg.seed).unwrap_or(cfg.seed.clone());
    seen.insert(seed.to_string());
    link_q.push_back(Task {
        url: seed,
        depth: 0,
        via: "seed",
        found_on: None,
        retries: 0,
    });

    let mut inflight: JoinSet<Fetched> = JoinSet::new();
    let mut written = 0usize;
    let mut refusal_streak = 0usize;
    let mut stop_reason: Option<String> = None;
    let mut started_count = 0usize;
    let mut next_at = tokio::time::Instant::now();

    loop {
        while inflight.len() < cfg.concurrency && started_count < cfg.max_pages && stop_reason.is_none() {
            // Sitemap URLs are only pulled once link discovery is exhausted, so pages
            // reachable by links are attributed to links (and orphans stand out).
            let task = match link_q.pop_front() {
                Some(t) => t,
                None if inflight.is_empty() => match sitemap_q.pop_front() {
                    Some(t) if seen.insert(t.url.to_string()) => t,
                    Some(_) => continue,
                    None => break,
                },
                None => break,
            };

            if let Some(r) = &robot
                && !r.allowed(task.url.as_str())
            {
                writer.send(Msg::Page(Box::new(blocked_record(&task))));
                written += 1;
                started_count += 1;
                continue;
            }

            tokio::time::sleep_until(next_at).await;
            next_at = tokio::time::Instant::now() + interval;
            started_count += 1;
            inflight.spawn(fetch(client.clone(), scope.clone(), task, cfg.keep_html));
        }

        let Some(joined) = inflight.join_next().await else {
            break;
        };
        let f = match joined {
            Ok(f) => f,
            Err(e) => {
                pb.println(format!("worker failed: {e}"));
                continue;
            }
        };

        // Back off and retry when the server asks us to slow down.
        let first_status = f.hops.first().map(|h| h.status).or(f.final_status);

        // A run of refusals means bot protection has kicked in. Stop rather than keep
        // hitting a site that is saying no; the reason is recorded with the crawl.
        match f.final_status {
            Some(401 | 403 | 429) => refusal_streak += 1,
            Some(200) => refusal_streak = 0,
            _ => {}
        }
        if refusal_streak >= REFUSAL_STREAK_LIMIT && stop_reason.is_none() {
            let reason = format!(
                "stopped after {refusal_streak} consecutive refusals (HTTP {}): the site appears to be blocking the crawler",
                f.final_status.unwrap_or_default()
            );
            pb.println(format!("⚠ {reason}"));
            writer.send(Msg::StopReason(reason.clone()));
            stop_reason = Some(reason);
        }
        if matches!(first_status, Some(429 | 503)) && f.task.retries < MAX_RETRIES {
            interval = (interval * 2).min(MAX_INTERVAL);
            pb.println(format!(
                "{} → {:?}, slowing to {}ms/request",
                f.task.url,
                first_status,
                interval.as_millis()
            ));
            let mut t = f.task.clone();
            t.retries += 1;
            started_count -= 1;
            link_q.push_back(t);
            continue;
        }

        // Ease back toward the configured rate once the server stops pushing back.
        interval = base_interval.max(interval.mul_f64(0.9));

        let (records, extracted_from) = into_records(f, &mut seen, started_count < cfg.max_pages);
        // A redirect target fetched in the same request still counts against the page budget.
        started_count += records.len().saturating_sub(1);
        if let Some((page_url, depth, x)) = extracted_from
            && depth < cfg.max_depth
        {
            let extra = [
                x.canonical.as_deref().map(|c| (c, "canonical")),
                x.pagination_next.as_deref().map(|c| (c, "pagination")),
                x.pagination_prev.as_deref().map(|c| (c, "pagination")),
            ];
            let candidates = x
                .links
                .iter()
                .filter(|l| l.internal)
                .filter_map(|l| l.target.as_deref().map(|t| (t, "link")))
                .chain(extra.into_iter().flatten())
                .chain(x.hreflang.iter().map(|(_, h)| (h.as_str(), "hreflang")));
            for (target, via) in candidates {
                let Ok(u) = Url::parse(target) else { continue };
                if !scope.is_internal(&u) || seen.contains(u.as_str()) {
                    continue;
                }
                if u.query().is_some() {
                    let key = format!("{}{}", u.host_str().unwrap_or_default(), u.path());
                    let n = query_variants.entry(key).or_default();
                    if *n >= cfg.max_query_variants {
                        continue;
                    }
                    *n += 1;
                }
                seen.insert(u.to_string());
                link_q.push_back(Task {
                    url: u,
                    depth: depth + 1,
                    via,
                    found_on: Some(page_url.clone()),
                    retries: 0,
                });
            }
        }
        for r in records {
            writer.send(Msg::Page(Box::new(r)));
            written += 1;
        }

        pb.set_message(format!(
            "{written} pages · {} in flight · {} queued · {} sitemap-only pending",
            inflight.len(),
            link_q.len(),
            sitemap_q.len()
        ));
    }

    pb.finish_and_clear();
    writer.finish()?;
    Ok(CrawlStats {
        pages: written,
        sitemap_urls,
        elapsed: started.elapsed(),
    })
}

/// Turns one fetch into one or two page records: the requested URL, plus the redirect
/// target if it is internal and hasn't been seen (its body was already downloaded).
/// Returns the page whose links should be followed.
fn into_records(
    f: Fetched,
    seen: &mut HashSet<String>,
    budget_left: bool,
) -> (Vec<PageRecord>, Option<(String, u32, Extracted)>) {
    let mut chain = f.hops.clone();
    if !chain.is_empty()
        && let Some(s) = f.final_status
    {
        chain.push(Hop {
            url: f.final_url.to_string(),
            status: s,
        });
    }
    let base = PageRecord {
        url: f.task.url.to_string(),
        crawl_depth: f.task.depth,
        discovered_via: f.task.via,
        found_on: f.task.found_on.clone(),
        fetched_at: f.fetched_at,
        blocked_by_robots: false,
        error: f.error.clone(),
        status: f.hops.first().map(|h| h.status).or(f.final_status),
        final_status: f.final_status,
        final_url: Some(f.final_url.to_string()),
        redirect_chain: chain,
        content_type: f.content_type.clone(),
        bytes: f.bytes,
        ttfb_ms: f.ttfb_ms,
        total_ms: Some(f.total_ms),
        headers: f.headers.clone(),
        extracted: None,
    };

    if f.hops.is_empty() {
        let follow = f.extracted.clone().map(|x| (base.url.clone(), f.task.depth, x));
        return (
            vec![PageRecord {
                extracted: f.extracted,
                ..base
            }],
            follow,
        );
    }

    // Redirected: the source record carries no content; the target gets its own record.
    let source = PageRecord {
        content_type: None,
        bytes: None,
        headers: Default::default(),
        ..base
    };
    let target_url = normalize(&f.final_url).map(|u| u.to_string());
    match (target_url, f.extracted) {
        (Some(t), Some(x)) if budget_left && seen.insert(t.clone()) => {
            let target = PageRecord {
                url: t.clone(),
                discovered_via: "redirect",
                found_on: Some(f.task.url.to_string()),
                status: f.final_status,
                final_url: Some(t.clone()),
                redirect_chain: vec![],
                content_type: f.content_type,
                bytes: f.bytes,
                headers: f.headers,
                extracted: Some(x.clone()),
                ..source.clone()
            };
            (vec![source, target], Some((t, f.task.depth, x)))
        }
        _ => (vec![source], None),
    }
}

fn blocked_record(task: &Task) -> PageRecord {
    PageRecord {
        url: task.url.to_string(),
        crawl_depth: task.depth,
        discovered_via: task.via,
        found_on: task.found_on.clone(),
        fetched_at: Utc::now(),
        blocked_by_robots: true,
        error: None,
        status: None,
        final_status: None,
        final_url: None,
        redirect_chain: vec![],
        content_type: None,
        bytes: None,
        ttfb_ms: None,
        total_ms: None,
        headers: Default::default(),
        extracted: None,
    }
}

async fn fetch(client: reqwest::Client, scope: Arc<Scope>, task: Task, keep_html: bool) -> Fetched {
    let t_start = Instant::now();
    let fetched_at = Utc::now();
    let mut hops: Vec<Hop> = Vec::new();
    let mut current = task.url.clone();

    let done = |final_url: Url, hops: Vec<Hop>, error: Option<String>| Fetched {
        task: task.clone(),
        fetched_at,
        hops,
        final_url,
        final_status: None,
        content_type: None,
        bytes: None,
        ttfb_ms: None,
        total_ms: t_start.elapsed().as_millis() as u32,
        headers: Default::default(),
        error,
        extracted: None,
    };

    loop {
        let t0 = Instant::now();
        let resp = match client.get(current.clone()).send().await {
            Ok(r) => r,
            Err(e) => return done(current, hops, Some(error_kind(&e))),
        };
        let ttfb = t0.elapsed().as_millis() as u32;
        let status = resp.status();

        if status.is_redirection()
            && let Some(next) = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|loc| current.join(loc).ok())
        {
            hops.push(Hop {
                url: current.to_string(),
                status: status.as_u16(),
            });
            if hops.iter().any(|h| h.url == next.as_str()) {
                return done(next, hops, Some("redirect_loop".into()));
            }
            if hops.len() > MAX_REDIRECTS {
                return done(next, hops, Some("too_many_redirects".into()));
            }
            current = next;
            continue;
        }

        let mut headers = serde_json::Map::new();
        for name in KEPT_HEADERS {
            let vals: Vec<&str> = resp
                .headers()
                .get_all(*name)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            if !vals.is_empty() {
                headers.insert(name.to_string(), vals.join(", ").into());
            }
        }
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let declared_len = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let is_html = content_type
            .as_deref()
            .is_some_and(|c| c.contains("text/html") || c.contains("application/xhtml"));

        let mut f = done(current.clone(), std::mem::take(&mut hops), None);
        f.final_status = Some(status.as_u16());
        f.content_type = content_type;
        f.ttfb_ms = Some(ttfb);
        f.headers = headers;
        f.bytes = declared_len;

        // Only download bodies we will parse: internal HTML under the size cap.
        if is_html && scope.is_internal(&current) && declared_len.is_none_or(|l| l <= MAX_HTML_BYTES) {
            match resp.text().await {
                Ok(body) => {
                    f.bytes = Some(body.len() as u64);
                    let (url, scope) = (current.clone(), scope.clone());
                    f.extracted = tokio::task::spawn_blocking(move || {
                        let mut x = extract::extract(&body, &url, &scope);
                        x.html = keep_html.then_some(body);
                        x
                    })
                    .await
                    .ok();
                }
                Err(e) => f.error = Some(error_kind(&e)),
            }
        }
        f.total_ms = t_start.elapsed().as_millis() as u32;
        return f;
    }
}

fn error_kind(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_body() || e.is_decode() {
        "body"
    } else {
        "request"
    };
    format!("{kind}: {e}")
}
