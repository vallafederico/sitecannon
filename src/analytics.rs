//! Real usage data next to the crawl: GA4 (sessions, engagement, key events, revenue, channels)
//! and Search Console (clicks, impressions, CTR, position, queries). Fetched with the user's
//! Google credentials or imported from CSV exports, then joined to crawled pages by URL path so
//! the analysis can say what actually works: by template, topic, depth and section type.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Days, Utc};
use duckdb::{Connection, params};
use serde_json::{Value, json};

use crate::store::DB_FILE;

const GA_SCOPE: &str = "https://www.googleapis.com/auth/analytics.readonly";
const GSC_SCOPE: &str = "https://www.googleapis.com/auth/webmasters.readonly";
const GA_PAGE_ROWS: usize = 100_000;
const GA_MAX_ROWS: usize = 500_000;
const GSC_PAGE_ROWS: usize = 25_000;
const GSC_MAX_QUERY_ROWS: usize = 100_000;

const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS analytics_meta (
    source VARCHAR, detail VARCHAR, date_start VARCHAR, date_end VARCHAR, rows BIGINT, method VARCHAR, loaded_at TIMESTAMPTZ
);
CREATE TABLE IF NOT EXISTS ga_landing (
    host VARCHAR, path VARCHAR, sessions DOUBLE, engaged_sessions DOUBLE, avg_session_duration DOUBLE,
    key_events DOUBLE, revenue DOUBLE, new_users DOUBLE
);
CREATE TABLE IF NOT EXISTS ga_channels (host VARCHAR, path VARCHAR, channel VARCHAR, sessions DOUBLE);
CREATE TABLE IF NOT EXISTS ga_pages (
    host VARCHAR, path VARCHAR, views DOUBLE, active_users DOUBLE, engagement_seconds DOUBLE, key_events DOUBLE
);
CREATE TABLE IF NOT EXISTS gsc_pages (page_url VARCHAR, clicks DOUBLE, impressions DOUBLE, ctr DOUBLE, position DOUBLE);
CREATE TABLE IF NOT EXISTS gsc_queries (
    page_url VARCHAR, query VARCHAR, clicks DOUBLE, impressions DOUBLE, ctr DOUBLE, position DOUBLE
);
"#;

/// Joins usage data to crawled pages on a normalised path (lowercase, no query, no trailing
/// slash). Rows for other hosts in the same GA property are ignored; rows without a host match.
const VIEWS: &str = r#"
CREATE OR REPLACE MACRO sc_path(p) AS
    coalesce(nullif(rtrim(lower(regexp_replace(coalesce(p, '/'), '[?#].*$', '')), '/'), ''), '/');
CREATE OR REPLACE MACRO sc_url_path(u) AS sc_path(regexp_replace(u, '^https?://[^/]+', ''));
CREATE OR REPLACE MACRO sc_host(h) AS lower(regexp_replace(coalesce(h, ''), '^www\.', ''));

CREATE OR REPLACE VIEW site_host AS
SELECT sc_host(regexp_extract(seed, '^https?://([^/:]+)', 1)) AS host FROM crawl;

CREATE OR REPLACE VIEW ga_landing_k AS
SELECT sc_path(path) AS k, sum(sessions) AS sessions, sum(engaged_sessions) AS engaged_sessions,
    sum(avg_session_duration * sessions) / nullif(sum(sessions), 0) AS avg_session_duration,
    sum(key_events) AS key_events, sum(revenue) AS revenue, sum(new_users) AS new_users
FROM ga_landing WHERE coalesce(host, '') = '' OR sc_host(host) = (SELECT host FROM site_host)
GROUP BY 1;

CREATE OR REPLACE VIEW ga_channels_k AS
SELECT sc_path(path) AS k,
    sum(sessions) FILTER (WHERE channel = 'Organic Search') AS organic_sessions,
    sum(sessions) AS channel_sessions
FROM ga_channels WHERE coalesce(host, '') = '' OR sc_host(host) = (SELECT host FROM site_host)
GROUP BY 1;

CREATE OR REPLACE VIEW ga_pages_k AS
SELECT sc_path(path) AS k, sum(views) AS views, sum(active_users) AS active_users,
    sum(engagement_seconds) AS engagement_seconds, sum(key_events) AS page_key_events
FROM ga_pages WHERE coalesce(host, '') = '' OR sc_host(host) = (SELECT host FROM site_host)
GROUP BY 1;

CREATE OR REPLACE VIEW gsc_pages_k AS
SELECT sc_url_path(page_url) AS k, sum(clicks) AS clicks, sum(impressions) AS impressions,
    sum(clicks) / nullif(sum(impressions), 0) AS ctr,
    sum(position * impressions) / nullif(sum(impressions), 0) AS position
FROM gsc_pages GROUP BY 1;

-- One row per crawled content page with whatever usage data matched it.
CREATE OR REPLACE VIEW page_performance AS
SELECT f.url, f.title, f.template_id, t.pattern AS template_pattern, (t.pages = 1) AS one_off,
    f.click_depth, round(f.pagerank, 3) AS pagerank, f.inlinks, f.main_word_count, pt.topic_id,
    coalesce(l.sessions, 0) AS sessions, coalesce(l.engaged_sessions, 0) AS engaged_sessions,
    l.engaged_sessions / nullif(l.sessions, 0) AS engagement_rate,
    l.avg_session_duration, coalesce(l.key_events, 0) AS key_events,
    l.key_events / nullif(l.sessions, 0) AS key_events_per_session,
    coalesce(l.revenue, 0) AS revenue, c.organic_sessions,
    coalesce(p.views, 0) AS views, p.engagement_seconds / nullif(p.views, 0) AS seconds_per_view,
    coalesce(g.clicks, 0) AS search_clicks, coalesce(g.impressions, 0) AS search_impressions,
    g.ctr AS search_ctr, g.position AS search_position
FROM pages_full f
LEFT JOIN templates t USING (template_id)
LEFT JOIN page_topics pt USING (url)
LEFT JOIN ga_landing_k l ON l.k = sc_url_path(f.url)
LEFT JOIN ga_channels_k c ON c.k = sc_url_path(f.url)
LEFT JOIN ga_pages_k p ON p.k = sc_url_path(f.url)
LEFT JOIN gsc_pages_k g ON g.k = sc_url_path(f.url);

-- Landing pages with traffic that don't match a crawled content page: old URLs that redirect,
-- broken URLs still getting visits, or pages the crawl never reached.
CREATE OR REPLACE VIEW analytics_unmatched AS
WITH crawled AS (
    SELECT sc_url_path(url) AS k, any_value(url) AS url, any_value(status) AS status, any_value(final_url) AS final_url,
        bool_or(final_status = 200 AND is_html AND redirect_hops = 0) AS content
    FROM pages GROUP BY 1)
SELECT l.k AS path, l.sessions, l.key_events, c.url AS crawled_url, c.status, c.final_url,
    CASE WHEN c.k IS NULL THEN 'not crawled' WHEN c.status BETWEEN 300 AND 399 THEN 'redirects'
         WHEN c.status >= 400 THEN 'error' ELSE 'other' END AS crawl_status
FROM ga_landing_k l LEFT JOIN crawled c ON c.k = l.k
WHERE NOT coalesce(c.content, false) AND l.sessions > 0
ORDER BY l.sessions DESC;
"#;

fn open(project: &Path) -> Result<Connection> {
    let path = if project.is_dir() {
        project.join(DB_FILE)
    } else {
        project.to_path_buf()
    };
    anyhow::ensure!(path.exists(), "no crawl database at {}", path.display());
    let conn = Connection::open(&path)?;
    conn.execute_batch(TABLES)?;
    Ok(conn)
}

/// Creates the views; needs `analyze` to have run (they read pages_full and templates).
fn refresh_views(conn: &Connection) -> Result<()> {
    let analysed: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'page_metrics'",
        [],
        |r| r.get(0),
    )?;
    if analysed {
        conn.execute_batch(VIEWS)?;
    }
    Ok(())
}

pub fn has_data(project: &Path) -> bool {
    crate::query::run(project, "SELECT count(*) FROM analytics_meta")
        .ok()
        .and_then(|t| t.rows.first().map(|r| crate::query::to_json(&r[0])))
        .and_then(|v| v.as_i64())
        .is_some_and(|n| n > 0)
}

fn record_meta(
    conn: &Connection,
    source: &str,
    detail: &str,
    range: (&str, &str),
    rows: usize,
    method: &str,
) -> Result<()> {
    // An API fetch ("ga4"/"gsc") replaces all earlier imports of that family; a CSV import
    // (e.g. "gsc_queries") replaces itself and any earlier API fetch of its family.
    let family = if source.starts_with("ga4") { "ga4" } else { "gsc" };
    if source == family {
        conn.execute(
            "DELETE FROM analytics_meta WHERE source LIKE concat(?, '%')",
            params![family],
        )?;
    } else {
        conn.execute(
            "DELETE FROM analytics_meta WHERE source = ? OR source = ?",
            params![source, family],
        )?;
    }
    conn.execute(
        "INSERT INTO analytics_meta VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![source, detail, range.0, range.1, rows as i64, method, Utc::now()],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ Google auth

pub struct GoogleAuth {
    pub token: String,
    pub quota_project: Option<String>,
    pub source: String,
}

fn adc_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        return Some(p.into());
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)?
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    let p = base.join("gcloud").join("application_default_credentials.json");
    p.exists().then_some(p)
}

/// Finds a Google access token for the given scopes. Order: SITECANNON_GOOGLE_TOKEN,
/// Application Default Credentials (user login or service-account key), then the gcloud CLI.
pub fn google_auth(scopes: &[&str]) -> Result<GoogleAuth> {
    if let Ok(t) = std::env::var("SITECANNON_GOOGLE_TOKEN")
        && !t.trim().is_empty()
    {
        return Ok(GoogleAuth {
            token: t.trim().into(),
            quota_project: std::env::var("SITECANNON_GOOGLE_QUOTA_PROJECT").ok(),
            source: "SITECANNON_GOOGLE_TOKEN".into(),
        });
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    if let Some(path) = adc_path() {
        let creds: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("reading {}", path.display()))?;
        let quota = creds["quota_project_id"].as_str().map(String::from);
        match creds["type"].as_str() {
            Some("authorized_user") => {
                let resp: Value = client
                    .post("https://oauth2.googleapis.com/token")
                    .form(&[
                        ("client_id", creds["client_id"].as_str().unwrap_or_default()),
                        (
                            "client_secret",
                            creds["client_secret"].as_str().unwrap_or_default(),
                        ),
                        (
                            "refresh_token",
                            creds["refresh_token"].as_str().unwrap_or_default(),
                        ),
                        ("grant_type", "refresh_token"),
                    ])
                    .send()?
                    .json()?;
                let token = resp["access_token"]
                    .as_str()
                    .ok_or_else(|| anyhow!("token refresh failed: {resp}"))?;
                return Ok(GoogleAuth {
                    token: token.into(),
                    quota_project: quota,
                    source: format!("user credentials ({})", path.display()),
                });
            }
            Some("service_account") => {
                let token = service_account_token(&client, &creds, scopes)?;
                let who = creds["client_email"].as_str().unwrap_or("service account");
                return Ok(GoogleAuth {
                    token,
                    quota_project: quota,
                    source: format!("service account {who}"),
                });
            }
            other => bail!("unsupported credential type {other:?} in {}", path.display()),
        }
    }
    if let Ok(out) = std::process::Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        && out.status.success()
    {
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !token.is_empty() {
            return Ok(GoogleAuth {
                token,
                quota_project: None,
                source: "gcloud CLI".into(),
            });
        }
    }
    bail!(
        "no Google credentials found. Either run\n  gcloud auth application-default login --scopes={},{},https://www.googleapis.com/auth/cloud-platform\n\
         or set GOOGLE_APPLICATION_CREDENTIALS to a service-account key that has been given access to the GA4 property \
         and Search Console site, or set SITECANNON_GOOGLE_TOKEN. Alternatively import CSV exports with import_analytics.",
        GA_SCOPE,
        GSC_SCOPE
    )
}

fn service_account_token(
    client: &reqwest::blocking::Client,
    creds: &Value,
    scopes: &[&str],
) -> Result<String> {
    let token_uri = creds["token_uri"]
        .as_str()
        .unwrap_or("https://oauth2.googleapis.com/token");
    let now = Utc::now().timestamp();
    let claims = json!({
        "iss": creds["client_email"], "scope": scopes.join(" "), "aud": token_uri, "iat": now, "exp": now + 3600,
    });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = creds["private_key_id"].as_str().map(String::from);
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(
        creds["private_key"].as_str().unwrap_or_default().as_bytes(),
    )?;
    let assertion = jsonwebtoken::encode(&header, &claims, &key)?;
    let resp: Value = client
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()?
        .json()?;
    resp["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow!("service account token exchange failed: {resp}"))
}

/// Which credential source would be used, without printing the token.
pub fn auth_status() -> Value {
    match google_auth(&[GA_SCOPE, GSC_SCOPE]) {
        Ok(a) => json!({"ok": true, "source": a.source, "quota_project": a.quota_project}),
        Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
    }
}

fn api_post(client: &reqwest::blocking::Client, auth: &GoogleAuth, url: &str, body: &Value) -> Result<Value> {
    let mut req = client.post(url).bearer_auth(&auth.token).json(body);
    if let Some(q) = &auth.quota_project {
        req = req.header("x-goog-user-project", q);
    }
    let resp = req.send()?;
    let status = resp.status();
    let v: Value = resp.json().unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().unwrap_or("request failed");
        bail!("{status} from {url}: {msg}");
    }
    Ok(v)
}

// ------------------------------------------------------------------ GA4

fn ga_api() -> String {
    std::env::var("SITECANNON_GA_API")
        .unwrap_or_else(|_| "https://analyticsdata.googleapis.com/v1beta".into())
}

fn gsc_api() -> String {
    std::env::var("SITECANNON_GSC_API").unwrap_or_else(|_| "https://www.googleapis.com/webmasters/v3".into())
}

/// Runs a GA4 report with pagination. Returns rows as (dimension values, metric values).
fn ga_report(
    client: &reqwest::blocking::Client,
    auth: &GoogleAuth,
    property: &str,
    days: u32,
    dims: &[&str],
    metrics: &[&str],
) -> Result<Vec<(Vec<String>, Vec<f64>)>> {
    let url = format!("{}/{property}:runReport", ga_api());
    let mut out = Vec::new();
    loop {
        let body = json!({
            "dateRanges": [{"startDate": format!("{days}daysAgo"), "endDate": "yesterday"}],
            "dimensions": dims.iter().map(|d| json!({"name": d})).collect::<Vec<_>>(),
            "metrics": metrics.iter().map(|m| json!({"name": m})).collect::<Vec<_>>(),
            "limit": GA_PAGE_ROWS, "offset": out.len(),
        });
        let v = api_post(client, auth, &url, &body)?;
        let rows = parse_ga_rows(&v);
        let n = rows.len();
        out.extend(rows);
        let total = v["rowCount"].as_u64().unwrap_or(0) as usize;
        if n == 0 || out.len() >= total || out.len() >= GA_MAX_ROWS {
            break;
        }
    }
    Ok(out)
}

fn parse_ga_rows(v: &Value) -> Vec<(Vec<String>, Vec<f64>)> {
    v["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let d = r["dimensionValues"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|x| x["value"].as_str().unwrap_or_default().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let m = r["metricValues"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|x| x["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0))
                                .collect()
                        })
                        .unwrap_or_default();
                    (d, m)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_ga4(conn: &Connection, auth: &GoogleAuth, property: &str, days: u32) -> Result<Value> {
    let property = if property.starts_with("properties/") {
        property.to_string()
    } else {
        format!("properties/{property}")
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let landing = ga_report(
        &client,
        auth,
        &property,
        days,
        &["hostName", "landingPagePlusQueryString"],
        &[
            "sessions",
            "engagedSessions",
            "averageSessionDuration",
            "keyEvents",
            "totalRevenue",
            "newUsers",
        ],
    )?;
    let channels = ga_report(
        &client,
        auth,
        &property,
        days,
        &[
            "hostName",
            "landingPagePlusQueryString",
            "sessionDefaultChannelGroup",
        ],
        &["sessions"],
    )?;
    let pages = ga_report(
        &client,
        auth,
        &property,
        days,
        &["hostName", "pagePath"],
        &[
            "screenPageViews",
            "activeUsers",
            "userEngagementDuration",
            "keyEvents",
        ],
    )?;

    conn.execute_batch("DELETE FROM ga_landing; DELETE FROM ga_channels; DELETE FROM ga_pages;")?;
    {
        let mut app = conn.appender("ga_landing")?;
        for (d, m) in &landing {
            app.append_row(params![d[0], d[1], m[0], m[1], m[2], m[3], m[4], m[5]])?;
        }
        let mut app = conn.appender("ga_channels")?;
        for (d, m) in &channels {
            app.append_row(params![d[0], d[1], d[2], m[0]])?;
        }
        let mut app = conn.appender("ga_pages")?;
        for (d, m) in &pages {
            app.append_row(params![d[0], d[1], m[0], m[1], m[2], m[3]])?;
        }
    }
    let end = Utc::now().date_naive() - Days::new(1);
    let start = end - Days::new(days as u64 - 1);
    record_meta(
        conn,
        "ga4",
        &property,
        (&start.to_string(), &end.to_string()),
        landing.len() + pages.len(),
        "api",
    )?;
    Ok(
        json!({"property": property, "landing_page_rows": landing.len(), "channel_rows": channels.len(), "page_rows": pages.len()}),
    )
}

// ------------------------------------------------------------------ Search Console

fn gsc_query(
    client: &reqwest::blocking::Client,
    auth: &GoogleAuth,
    site: &str,
    range: (&str, &str),
    dims: &[&str],
    max_rows: usize,
) -> Result<Vec<(Vec<String>, [f64; 4])>> {
    // Domain properties (sc-domain:) go in the path as-is; URL-prefix properties are encoded.
    let site_path = if site.starts_with("sc-domain:") {
        site.to_string()
    } else {
        url::form_urlencoded::byte_serialize(site.as_bytes()).collect()
    };
    let url = format!("{}/sites/{site_path}/searchAnalytics/query", gsc_api());
    let mut out = Vec::new();
    loop {
        let body = json!({"startDate": range.0, "endDate": range.1, "dimensions": dims, "rowLimit": GSC_PAGE_ROWS, "startRow": out.len(), "type": "web"});
        let v = api_post(client, auth, &url, &body)?;
        let rows = parse_gsc_rows(&v);
        let n = rows.len();
        out.extend(rows);
        if n < GSC_PAGE_ROWS || out.len() >= max_rows {
            break;
        }
    }
    Ok(out)
}

fn parse_gsc_rows(v: &Value) -> Vec<(Vec<String>, [f64; 4])> {
    v["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let keys = r["keys"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|k| k.as_str().unwrap_or_default().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let f = |k: &str| r[k].as_f64().unwrap_or(0.0);
                    (keys, [f("clicks"), f("impressions"), f("ctr"), f("position")])
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_gsc(conn: &Connection, auth: &GoogleAuth, site: &str, days: u32) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    // Search Console data lags by ~2–3 days.
    let end = Utc::now().date_naive() - Days::new(3);
    let start = end - Days::new(days as u64 - 1);
    let (s, e) = (start.to_string(), end.to_string());
    let pages = gsc_query(&client, auth, site, (&s, &e), &["page"], 200_000)?;
    let queries = gsc_query(
        &client,
        auth,
        site,
        (&s, &e),
        &["page", "query"],
        GSC_MAX_QUERY_ROWS,
    )?;
    conn.execute_batch("DELETE FROM gsc_pages; DELETE FROM gsc_queries;")?;
    {
        let mut app = conn.appender("gsc_pages")?;
        for (k, m) in &pages {
            app.append_row(params![k[0], m[0], m[1], m[2], m[3]])?;
        }
        let mut app = conn.appender("gsc_queries")?;
        for (k, m) in &queries {
            app.append_row(params![k[0], k[1], m[0], m[1], m[2], m[3]])?;
        }
    }
    record_meta(conn, "gsc", site, (&s, &e), pages.len() + queries.len(), "api")?;
    Ok(json!({"site": site, "page_rows": pages.len(), "query_rows": queries.len(), "date_range": [s, e]}))
}

/// Fetches GA4 and/or Search Console data into the project and refreshes the joined views.
pub fn fetch(project: &Path, ga4_property: Option<&str>, gsc_site: Option<&str>, days: u32) -> Result<Value> {
    if ga4_property.is_none() && gsc_site.is_none() {
        bail!(
            "pass a GA4 property id (ga4_property) and/or a Search Console site (gsc_site, e.g. sc-domain:example.com)"
        );
    }
    let days = days.clamp(7, 480);
    let conn = open(project)?;
    let mut scopes = Vec::new();
    if ga4_property.is_some() {
        scopes.push(GA_SCOPE);
    }
    if gsc_site.is_some() {
        scopes.push(GSC_SCOPE);
    }
    let auth = google_auth(&scopes)?;
    let mut out = json!({"auth": auth.source, "days": days});
    if let Some(p) = ga4_property {
        out["ga4"] = fetch_ga4(&conn, &auth, p, days).context("GA4 fetch failed")?;
    }
    if let Some(s) = gsc_site {
        out["gsc"] = fetch_gsc(&conn, &auth, s, days).context("Search Console fetch failed")?;
    }
    refresh_views(&conn)?;
    out["matched"] = coverage(&conn)?;
    Ok(out)
}

fn coverage(conn: &Connection) -> Result<Value> {
    let analysed: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'page_performance'",
        [],
        |r| r.get(0),
    )?;
    if !analysed {
        return Ok(json!("run analyze to join usage data to pages"));
    }
    let (sessions, matched, unmatched): (f64, f64, f64) = conn.query_row(
        "SELECT coalesce((SELECT sum(sessions) FROM ga_landing_k), 0), coalesce((SELECT sum(sessions) FROM page_performance), 0),
                coalesce((SELECT sum(sessions) FROM analytics_unmatched), 0)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(json!({
        "ga_sessions": sessions, "sessions_on_crawled_pages": matched, "sessions_on_unmatched_urls": unmatched,
        "share_matched": if sessions > 0.0 { (matched / sessions * 1000.0).round() / 1000.0 } else { 0.0 },
    }))
}

// ------------------------------------------------------------------ CSV import

fn norm_header(h: &str) -> String {
    h.trim()
        .trim_start_matches('\u{feff}')
        .to_lowercase()
        .replace(['_', '-'], " ")
}

fn find(headers: &[String], aliases: &[&str]) -> Option<usize> {
    aliases.iter().find_map(|a| headers.iter().position(|h| h == a))
}

/// Parses "1,234", "4.5%", "00:01:23" and plain numbers. Percentages become fractions.
fn num(s: &str) -> f64 {
    let t = s.trim().replace([',', ' '], "");
    if let Some(p) = t.strip_suffix('%') {
        return p.parse::<f64>().map(|v| v / 100.0).unwrap_or(0.0);
    }
    if t.contains(':') {
        return t
            .split(':')
            .fold(0.0, |acc, part| acc * 60.0 + part.parse::<f64>().unwrap_or(0.0));
    }
    t.parse().unwrap_or(0.0)
}

fn split_url(v: &str) -> (String, String) {
    match url::Url::parse(v) {
        Ok(u) if u.has_host() => (
            u.host_str().unwrap_or_default().to_string(),
            format!(
                "{}{}",
                u.path(),
                u.query().map(|q| format!("?{q}")).unwrap_or_default()
            ),
        ),
        _ => (String::new(), v.to_string()),
    }
}

/// Imports a GA4 or Search Console CSV export. The kind is detected from the column names
/// (or forced with `kind`: ga4_landing, ga4_pages, gsc_pages, gsc_queries).
pub fn import(project: &Path, file: &Path, kind: Option<&str>, append: bool) -> Result<Value> {
    let raw = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    // GA4 exports name their range in the comment header: "# Start date: 20260101".
    let header_date = |label: &str| -> String {
        raw.lines()
            .take_while(|l| l.starts_with('#') || l.trim().is_empty())
            .find_map(|l| l.to_lowercase().split_once(label).map(|(_, v)| v.trim_start_matches(':').trim().to_string()))
            .map(|d| if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) { format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]) } else { d })
            .unwrap_or_default()
    };
    let (date_start, date_end) = (header_date("start date"), header_date("end date"));
    // GA4 exports start with "#" comment lines and a blank line before the header.
    let body: String = raw
        .lines()
        .skip_while(|l| l.starts_with('#') || l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(body.as_bytes());
    let headers: Vec<String> = rdr.headers()?.iter().map(norm_header).collect();
    let records: Vec<csv::StringRecord> = rdr.records().collect::<Result<_, _>>()?;
    let get = |r: &csv::StringRecord, i: Option<usize>| {
        i.and_then(|i| r.get(i)).unwrap_or_default().trim().to_string()
    };

    let path_col = find(
        &headers,
        &[
            "landing page + query string",
            "landing page",
            "landingpageplusquerystring",
            "landingpage",
            "page path and screen class",
            "page path + query string",
            "page path",
            "pagepath",
            "top pages",
            "page",
            "pages",
            "url",
            "address",
            "landing page url",
        ],
    );
    let query_col = find(&headers, &["query", "top queries", "queries", "search query"]);
    let host_col = find(&headers, &["hostname", "host name", "host"]);
    let has = |a: &[&str]| find(&headers, a).is_some();
    let detected = kind.map(String::from).unwrap_or_else(|| {
        if has(&["clicks"]) && has(&["impressions"]) {
            if query_col.is_some() && path_col.is_some() {
                "gsc_queries".into()
            } else {
                "gsc_pages".into()
            }
        } else if has(&["sessions"]) {
            "ga4_landing".into()
        } else {
            "ga4_pages".into()
        }
    });
    let path_col =
        path_col.ok_or_else(|| anyhow!("no page/landing page/URL column found in {:?}", headers))?;

    let conn = open(project)?;
    let mut n = 0usize;
    match detected.as_str() {
        "ga4_landing" => {
            if !append {
                conn.execute_batch("DELETE FROM ga_landing; DELETE FROM ga_channels;")?;
            }
            let (s, e, er, dur, ke, rev, nu, ch) = (
                find(&headers, &["sessions"]),
                find(&headers, &["engaged sessions", "engagedsessions"]),
                find(&headers, &["engagement rate", "engagementrate"]),
                find(
                    &headers,
                    &[
                        "average session duration",
                        "averagesessionduration",
                        "avg. session duration",
                    ],
                ),
                find(&headers, &["key events", "keyevents", "conversions"]),
                find(&headers, &["total revenue", "revenue", "totalrevenue"]),
                find(&headers, &["new users", "newusers"]),
                find(
                    &headers,
                    &[
                        "session default channel group",
                        "sessiondefaultchannelgroup",
                        "default channel group",
                        "session primary channel group",
                    ],
                ),
            );
            let mut land = conn.appender("ga_landing")?;
            let mut chan = conn.appender("ga_channels")?;
            for r in &records {
                let p = get(r, Some(path_col));
                if p.is_empty()
                    || p == "(not set)"
                    || p.eq_ignore_ascii_case("grand total")
                    || p.eq_ignore_ascii_case("total")
                {
                    continue;
                }
                let (h2, p2) = split_url(&p);
                let host = if host_col.is_some() { get(r, host_col) } else { h2 };
                let sessions = num(&get(r, s));
                let engaged = if e.is_some() {
                    num(&get(r, e))
                } else {
                    num(&get(r, er)) * sessions
                };
                land.append_row(params![
                    host,
                    p2,
                    sessions,
                    engaged,
                    num(&get(r, dur)),
                    num(&get(r, ke)),
                    num(&get(r, rev)),
                    num(&get(r, nu))
                ])?;
                if ch.is_some() {
                    chan.append_row(params![host, p2, get(r, ch), sessions])?;
                }
                n += 1;
            }
        }
        "ga4_pages" => {
            if !append {
                conn.execute_batch("DELETE FROM ga_pages;")?;
            }
            let (v, u, eng, ke) = (
                find(
                    &headers,
                    &[
                        "views",
                        "screen page views",
                        "screenpageviews",
                        "pageviews",
                        "page views",
                    ],
                ),
                find(&headers, &["active users", "users", "activeusers", "total users"]),
                find(
                    &headers,
                    &[
                        "user engagement",
                        "user engagement duration",
                        "userengagementduration",
                    ],
                ),
                find(&headers, &["key events", "keyevents", "conversions"]),
            );
            let mut app = conn.appender("ga_pages")?;
            for r in &records {
                let p = get(r, Some(path_col));
                if p.is_empty() || p == "(not set)" || p.eq_ignore_ascii_case("grand total") {
                    continue;
                }
                let (h2, p2) = split_url(&p);
                let host = if host_col.is_some() { get(r, host_col) } else { h2 };
                app.append_row(params![
                    host,
                    p2,
                    num(&get(r, v)),
                    num(&get(r, u)),
                    num(&get(r, eng)),
                    num(&get(r, ke))
                ])?;
                n += 1;
            }
        }
        "gsc_pages" | "gsc_queries" => {
            let queries = detected == "gsc_queries";
            if !append {
                conn.execute_batch(if queries {
                    "DELETE FROM gsc_queries;"
                } else {
                    "DELETE FROM gsc_pages;"
                })?;
            }
            let (c, i, ctr, pos) = (
                find(&headers, &["clicks"]),
                find(&headers, &["impressions"]),
                find(&headers, &["ctr", "click through rate"]),
                find(&headers, &["position", "average position", "avg. position"]),
            );
            let mut app = conn.appender(if queries { "gsc_queries" } else { "gsc_pages" })?;
            for r in &records {
                let p = get(r, Some(path_col));
                if p.is_empty() {
                    continue;
                }
                let (clicks, impressions) = (num(&get(r, c)), num(&get(r, i)));
                let ctr_v = if ctr.is_some() {
                    num(&get(r, ctr))
                } else if impressions > 0.0 {
                    clicks / impressions
                } else {
                    0.0
                };
                if queries {
                    app.append_row(params![
                        p,
                        get(r, query_col),
                        clicks,
                        impressions,
                        ctr_v,
                        num(&get(r, pos))
                    ])?;
                } else {
                    app.append_row(params![p, clicks, impressions, ctr_v, num(&get(r, pos))])?;
                }
                n += 1;
            }
        }
        other => bail!("unknown kind {other}; use ga4_landing, ga4_pages, gsc_pages or gsc_queries"),
    }
    let source = detected.as_str();
    let total: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM {}",
            match detected.as_str() {
                "ga4_landing" => "ga_landing",
                "ga4_pages" => "ga_pages",
                "gsc_pages" => "gsc_pages",
                _ => "gsc_queries",
            }
        ),
        [],
        |r| r.get(0),
    )?;
    let detail = format!(
        "{} ({})",
        file.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default(),
        detected
    );
    if !append || !has_meta(&conn, source)? {
        record_meta(&conn, source, &detail, (&date_start, &date_end), total as usize, "csv")?;
    }
    refresh_views(&conn)?;
    Ok(json!({"kind": detected, "rows_imported": n, "columns": headers, "matched": coverage(&conn)?}))
}

/// Sources are "ga4"/"gsc" for API fetches and the import kind (ga4_landing, gsc_pages…) for
/// CSVs, so a family matches by prefix.
fn has_meta(conn: &Connection, source: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) > 0 FROM analytics_meta WHERE source LIKE concat(?, '%')",
        params![source],
        |r| r.get(0),
    )?)
}

/// What usage data a project has, and whether Google credentials are available.
pub fn status(project: &Path) -> Result<Value> {
    let conn = open(project)?;
    refresh_views(&conn)?;
    let mut stmt = conn.prepare("SELECT source, detail, date_start, date_end, rows, method, loaded_at::VARCHAR FROM analytics_meta ORDER BY source")?;
    let sources: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({"source": r.get::<_, String>(0)?, "detail": r.get::<_, Option<String>>(1)?, "date_start": r.get::<_, Option<String>>(2)?,
                      "date_end": r.get::<_, Option<String>>(3)?, "rows": r.get::<_, i64>(4)?, "method": r.get::<_, String>(5)?, "loaded_at": r.get::<_, String>(6)?}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(
        json!({"sources": sources, "matched": if sources.is_empty() { Value::Null } else { coverage(&conn)? }, "google_auth": auth_status()}),
    )
}

/// Call after `analyze` so the joined views exist if usage data was loaded earlier.
pub fn refresh(project: &Path) -> Result<()> {
    if has_data(project) {
        refresh_views(&open(project)?)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ analysis

fn rows(conn: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rs = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rs.next()? {
        let st = r.as_ref();
        let names: Vec<String> = (0..st.column_count())
            .map(|i| st.column_name(i).map(|s| s.to_string()).unwrap_or_default())
            .collect();
        let mut o = serde_json::Map::new();
        for (i, name) in names.into_iter().enumerate() {
            let v: duckdb::types::Value = r.get(i)?;
            o.insert(name, crate::query::to_json(&v));
        }
        out.push(Value::Object(o));
    }
    Ok(out)
}

/// Approximate organic CTR by position, for spotting snippets that underperform.
const EXPECTED_CTR: &str = "CASE WHEN search_position < 1.5 THEN 0.28 WHEN search_position < 2.5 THEN 0.15 WHEN search_position < 3.5 THEN 0.10
    WHEN search_position < 4.5 THEN 0.07 WHEN search_position < 5.5 THEN 0.05 WHEN search_position < 10.5 THEN 0.03 ELSE 0.01 END";

const BY_SESSIONS: &str = "sessions DESC NULLS LAST, search_clicks DESC";

/// What works and what doesn't: performance by template, topic, depth and section type, plus
/// concrete opportunity lists.
pub fn performance(project: &Path) -> Result<Value> {
    let conn = open(project)?;
    refresh_views(&conn)?;
    let exists: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'page_performance'",
        [],
        |r| r.get(0),
    )?;
    if !exists || !has_meta(&conn, "ga4")? && !has_meta(&conn, "gsc")? {
        bail!(
            "no usage data in this project yet: use fetch_analytics (GA4 / Search Console) or import_analytics (CSV exports)"
        );
    }
    let has_ga = conn.query_row("SELECT count(*) > 0 FROM ga_landing", [], |r| r.get::<_, bool>(0))?;
    let has_gsc = conn.query_row("SELECT count(*) > 0 FROM gsc_pages", [], |r| r.get::<_, bool>(0))?;
    let has_q = conn.query_row("SELECT count(*) > 0 FROM gsc_queries", [], |r| {
        r.get::<_, bool>(0)
    })?;

    let sources = rows(
        &conn,
        "SELECT source, detail, date_start, date_end, method FROM analytics_meta ORDER BY source",
    )?;
    let totals = rows(&conn, "SELECT round(sum(sessions)) AS sessions, round(sum(engaged_sessions) / nullif(sum(sessions), 0), 3) AS engagement_rate,
            round(sum(key_events)) AS key_events, round(sum(key_events) / nullif(sum(sessions), 0), 4) AS key_events_per_session,
            round(sum(revenue), 2) AS revenue, round(sum(organic_sessions) / nullif(sum(sessions), 0), 3) AS organic_share,
            round(sum(search_clicks)) AS search_clicks, round(sum(search_impressions)) AS search_impressions,
            count(*) FILTER (WHERE sessions > 0) AS pages_with_sessions, count(*) AS pages
        FROM page_performance")?;

    let group = |key: &str, label: &str, from: &str, order: &str| -> Result<Vec<Value>> {
        rows(&conn, &format!(
            "SELECT {key} AS key, {label} AS label, count(*) AS pages, count(*) FILTER (WHERE sessions > 0) AS pages_with_sessions,
                round(sum(sessions)) AS sessions, round(sum(sessions) / count(*), 1) AS sessions_per_page,
                round(sum(engaged_sessions) / nullif(sum(sessions), 0), 3) AS engagement_rate,
                round(sum(key_events) / nullif(sum(sessions), 0), 4) AS key_events_per_session,
                round(sum(key_events)) AS key_events, round(sum(search_clicks)) AS search_clicks,
                round(sum(search_clicks) / nullif(sum(search_impressions), 0), 4) AS search_ctr
             FROM {from} GROUP BY 1, 2 ORDER BY {order}"
        ))
    };
    let by_template = group(
        "CASE WHEN one_off THEN NULL ELSE template_id END",
        "CASE WHEN one_off THEN 'one-off pages' ELSE '#' || template_id || ' ' || split_part(template_pattern, ' · ', 1) END",
        "page_performance",
        BY_SESSIONS,
    )?;
    let by_topic = group(
        "p.topic_id",
        "coalesce(t.label, 'no topic')",
        "page_performance p LEFT JOIN topics t USING (topic_id)",
        BY_SESSIONS,
    )
    .unwrap_or_default();
    let by_depth = group(
        "click_depth",
        "coalesce(click_depth::VARCHAR, 'unreachable')",
        "page_performance",
        "key NULLS LAST",
    )?;

    // Pages with a section type vs without, among pages with enough sessions to compare.
    // Correlation only: section types co-vary with page type.
    let by_section = rows(&conn, "WITH pp AS (SELECT * FROM page_performance WHERE sessions >= 20),
             st AS (SELECT DISTINCT section_type FROM page_sections WHERE section_type <> 'minor'),
             x AS (SELECT st.section_type, pp.*, EXISTS (SELECT 1 FROM page_sections s WHERE s.url = pp.url AND s.section_type = st.section_type) AS has FROM st, pp)
         SELECT section_type,
             count(*) FILTER (WHERE has) AS pages_with, count(*) FILTER (WHERE NOT has) AS pages_without,
             round(sum(engaged_sessions) FILTER (WHERE has) / nullif(sum(sessions) FILTER (WHERE has), 0), 3) AS engagement_with,
             round(sum(engaged_sessions) FILTER (WHERE NOT has) / nullif(sum(sessions) FILTER (WHERE NOT has), 0), 3) AS engagement_without,
             round(sum(key_events) FILTER (WHERE has) / nullif(sum(sessions) FILTER (WHERE has), 0), 4) AS key_events_per_session_with,
             round(sum(key_events) FILTER (WHERE NOT has) / nullif(sum(sessions) FILTER (WHERE NOT has), 0), 4) AS key_events_per_session_without
         FROM x GROUP BY 1 HAVING count(*) FILTER (WHERE has) >= 5 AND count(*) FILTER (WHERE NOT has) >= 5
         ORDER BY pages_with DESC").unwrap_or_default();

    let mut opp = serde_json::Map::new();
    let mut add = |name: &str, description: &str, sql: String| -> Result<()> {
        let list = rows(&conn, &sql)?;
        opp.insert(
            name.into(),
            json!({"description": description, "count": list.len(), "rows": list}),
        );
        Ok(())
    };
    if has_ga {
        add("hidden_winners", "Pages that earn top-quintile sessions but sit deep or get little internal linking. Promote them in navigation and from related content.",
            "SELECT url, title, round(sessions) AS sessions, round(engagement_rate, 3) AS engagement_rate, click_depth, inlinks, pagerank FROM page_performance
             WHERE sessions >= greatest(10, (SELECT quantile_cont(sessions, 0.8) FROM page_performance WHERE sessions > 0))
               AND (click_depth >= 3 OR pagerank < 0.5) ORDER BY sessions DESC LIMIT 25".into())?;
        add("over_promoted", "Pages that receive the most internal link equity but little traffic. Check whether they deserve their prominence.",
            "SELECT url, title, pagerank, inlinks, round(sessions) AS sessions FROM page_performance
             WHERE pagerank >= (SELECT quantile_cont(pagerank, 0.9) FROM page_performance)
               AND sessions <= (SELECT quantile_cont(sessions, 0.3) FROM page_performance) ORDER BY pagerank DESC LIMIT 25".into())?;
        add("top_converters", "Landing pages that drive the most key events. Protect them in any redesign or migration.",
            "SELECT url, title, round(key_events) AS key_events, round(sessions) AS sessions, round(key_events_per_session, 4) AS key_events_per_session, template_pattern
             FROM page_performance WHERE key_events > 0 ORDER BY key_events DESC LIMIT 20".into())?;
        add("poor_engagement", "Landing pages with real traffic but bottom-quartile engagement. The content may not match what visitors arrive for.",
            "SELECT url, title, round(sessions) AS sessions, round(engagement_rate, 3) AS engagement_rate, template_pattern FROM page_performance
             WHERE sessions >= greatest(20, (SELECT quantile_cont(sessions, 0.5) FROM page_performance WHERE sessions > 0))
               AND engagement_rate <= (SELECT quantile_cont(engagement_rate, 0.25) FROM page_performance WHERE sessions >= 20)
             ORDER BY sessions DESC LIMIT 25".into())?;
        add("unmatched_traffic", "URLs that receive sessions but aren't live content pages in the crawl: old URLs (they must keep redirecting after a migration), broken URLs still getting visits, or pages the crawl didn't reach.",
            "SELECT path, round(sessions) AS sessions, round(key_events) AS key_events, crawl_status, final_url FROM analytics_unmatched LIMIT 40".into())?;
        add("no_traffic", "Indexable pages with no sessions (and no search impressions if Search Console is loaded) in the period. Candidates to merge, improve or retire, after checking they aren't new.",
            format!("SELECT f.url, f.title, f.template_pattern, f.main_word_count FROM page_performance f JOIN pages_v v USING (url)
                     WHERE v.indexability = 'indexable' AND f.sessions = 0 {} ORDER BY f.pagerank LIMIT 60",
                    if has_gsc { "AND f.search_impressions = 0" } else { "" }))?;
    }
    if has_gsc {
        add("low_ctr", "Pages that rank on page one with plenty of impressions but a click-through rate well below what their position usually gets. Rewrite the title and meta description.",
            format!("SELECT url, title, round(search_impressions) AS impressions, round(search_ctr, 4) AS ctr, round(search_position, 1) AS position,
                        {EXPECTED_CTR} AS typical_ctr FROM page_performance
                     WHERE search_position <= 10 AND search_impressions >= greatest(100, (SELECT quantile_cont(search_impressions, 0.75) FROM page_performance WHERE search_impressions > 0))
                       AND search_ctr < 0.5 * ({EXPECTED_CTR}) ORDER BY search_impressions DESC LIMIT 25"))?;
    }
    if has_q {
        add("striking_distance", "Queries where a page ranks 4–20 with meaningful impressions. Small improvements can move these onto page one or into the top three.",
            "SELECT query, page_url, round(impressions) AS impressions, round(clicks) AS clicks, round(position, 1) AS position FROM gsc_queries
             WHERE position BETWEEN 4 AND 20 AND impressions >= greatest(20, (SELECT quantile_cont(impressions, 0.9) FROM gsc_queries))
             ORDER BY impressions DESC LIMIT 30".into())?;
    }

    Ok(json!({
        "sources": sources, "totals": totals.first(), "matched": coverage(&conn)?,
        "by_template": by_template, "by_topic": by_topic, "by_depth": by_depth, "by_section": by_section,
        "opportunities": opp,
        "caveats": [
            "Analytics undercounts where consent banners block tracking; compare ratios between pages rather than absolute totals.",
            "Section-type comparisons are correlations: pages with a section type also differ in other ways.",
            "URLs are matched on path (case-insensitive, query strings and trailing slashes ignored).",
            "Typical-CTR benchmarks are approximate industry averages."
        ],
    }))
}

/// Per-page usage columns for the report, aligned with the given URLs.
pub fn page_columns(project: &Path, urls: &[String]) -> Result<Option<Value>> {
    if !has_data(project) {
        return Ok(None);
    }
    let conn = open(project)?;
    refresh_views(&conn)?;
    let mut stmt = conn.prepare("SELECT url, sessions, engagement_rate, key_events, search_clicks, search_impressions, search_position FROM page_performance")?;
    let mut by_url: HashMap<String, [Option<f64>; 6]> = HashMap::new();
    let mut rs = stmt.query([])?;
    while let Some(r) = rs.next()? {
        by_url.insert(
            r.get(0)?,
            [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?],
        );
    }
    let col = |i: usize| -> Value {
        json!(
            urls.iter()
                .map(|u| by_url
                    .get(u)
                    .and_then(|v| v[i])
                    .map(|x| (x * 1000.0).round() / 1000.0))
                .collect::<Vec<_>>()
        )
    };
    Ok(Some(json!({
        "sessions": col(0), "engagement_rate": col(1), "key_events": col(2),
        "search_clicks": col(3), "search_impressions": col(4), "search_position": col(5),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_export_number_formats() {
        assert_eq!(num("1,234"), 1234.0);
        assert!((num("4.5%") - 0.045).abs() < 1e-9);
        assert_eq!(num("00:01:23"), 83.0);
        assert_eq!(num(""), 0.0);
    }

    #[test]
    fn parses_api_responses() {
        let ga = json!({"rows": [{"dimensionValues": [{"value": "www.a.com"}, {"value": "/pricing"}], "metricValues": [{"value": "120"}, {"value": "80.5"}]}], "rowCount": 1});
        let r = parse_ga_rows(&ga);
        assert_eq!(r[0].0, vec!["www.a.com", "/pricing"]);
        assert_eq!(r[0].1, vec![120.0, 80.5]);
        let gsc = json!({"rows": [{"keys": ["https://a.com/x", "bjj software"], "clicks": 3.0, "impressions": 100.0, "ctr": 0.03, "position": 7.2}]});
        let g = parse_gsc_rows(&gsc);
        assert_eq!(g[0].0[1], "bjj software");
        assert_eq!(g[0].1, [3.0, 100.0, 0.03, 7.2]);
    }

    #[test]
    fn splits_full_urls_into_host_and_path() {
        assert_eq!(
            split_url("https://www.a.com/x?y=1"),
            ("www.a.com".into(), "/x?y=1".into())
        );
        assert_eq!(split_url("/x"), (String::new(), "/x".into()));
    }
}
