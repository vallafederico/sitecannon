use std::collections::{HashSet, VecDeque};
use std::io::Read;

use quick_xml::Reader;
use quick_xml::events::Event;
use texting_robots::Robot;
use url::Url;

use crate::model::SitemapEntry;
use crate::urlnorm::normalize;

const MAX_SITEMAP_FILES: usize = 200;
const MAX_SITEMAP_URLS: usize = 200_000;
const MAX_SITEMAP_BYTES: usize = 100 * 1024 * 1024;

pub struct Robots {
    pub robot: Option<Robot>,
    pub sitemaps: Vec<String>,
    pub status: Option<u16>,
    pub raw: Option<String>,
}

pub async fn fetch_robots(client: &reqwest::Client, seed: &Url, agent: &str) -> Robots {
    let Ok(url) = seed.join("/robots.txt") else {
        return Robots {
            robot: None,
            sitemaps: vec![],
            status: None,
            raw: None,
        };
    };
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => {
            return Robots {
                robot: None,
                sitemaps: vec![],
                status: None,
                raw: None,
            };
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        // 4xx = no restrictions; 5xx would technically mean "assume disallow", but we treat it
        // as unrestricted and surface the status in the crawl metadata.
        return Robots {
            robot: None,
            sitemaps: vec![],
            status: Some(status),
            raw: None,
        };
    }
    let body = resp.text().await.unwrap_or_default();
    let robot = Robot::new(agent, body.as_bytes()).ok();
    let sitemaps = robot.as_ref().map(|r| r.sitemaps.clone()).unwrap_or_default();
    Robots {
        robot,
        sitemaps,
        status: Some(status),
        raw: Some(body),
    }
}

/// Walks sitemap indexes recursively. Falls back to `/sitemap.xml` and `/sitemap_index.xml`
/// when robots.txt doesn't declare any.
pub async fn fetch_sitemaps(client: &reqwest::Client, seed: &Url, declared: &[String]) -> Vec<SitemapEntry> {
    let mut queue: VecDeque<String> = declared.iter().cloned().collect();
    if queue.is_empty() {
        for p in ["/sitemap.xml", "/sitemap_index.xml"] {
            if let Ok(u) = seed.join(p) {
                queue.push_back(u.to_string());
            }
        }
    }
    let mut seen_files = HashSet::new();
    let mut entries = Vec::new();

    while let Some(sm) = queue.pop_front() {
        if !seen_files.insert(sm.clone()) || seen_files.len() > MAX_SITEMAP_FILES {
            continue;
        }
        let Some(xml) = fetch_xml(client, &sm).await else {
            continue;
        };
        let parsed = parse_sitemap(&xml);
        queue.extend(parsed.child_sitemaps);
        for (loc, lastmod) in parsed.urls {
            if entries.len() >= MAX_SITEMAP_URLS {
                break;
            }
            let url_norm = Url::parse(&loc)
                .ok()
                .and_then(|u| normalize(&u))
                .map(|u| u.to_string());
            entries.push(SitemapEntry {
                url: loc,
                url_norm,
                lastmod,
                sitemap: sm.clone(),
            });
        }
    }
    entries
}

async fn fetch_xml(client: &reqwest::Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_SITEMAP_BYTES {
        return None;
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .take(MAX_SITEMAP_BYTES as u64)
            .read_to_string(&mut out)
            .ok()?;
        return Some(out);
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // Servers sometimes answer /sitemap.xml with an HTML 200 page.
    text.trim_start()
        .starts_with('<')
        .then_some(text)
        .filter(|t| !t.contains("<html"))
}

#[derive(Default)]
struct ParsedSitemap {
    child_sitemaps: Vec<String>,
    urls: Vec<(String, Option<String>)>,
}

fn parse_sitemap(xml: &str) -> ParsedSitemap {
    let mut reader = Reader::from_str(xml);
    let mut out = ParsedSitemap::default();
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    let (mut loc, mut lastmod): (Option<String>, Option<String>) = (None, None);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                stack.push(e.local_name().as_ref().to_ascii_lowercase());
                text.clear();
            }
            Ok(Event::Text(t)) => text.push_str(&t),
            Ok(Event::CData(t)) => text.push_str(&t),
            Ok(Event::GeneralRef(r)) => match &*r {
                "amp" => text.push('&'),
                "lt" => text.push('<'),
                "gt" => text.push('>'),
                "quot" => text.push('"'),
                "apos" => text.push('\''),
                _ => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        text.push(c);
                    }
                }
            },
            Ok(Event::End(_)) => {
                let name = stack.pop().unwrap_or_default();
                let parent = stack.last().map(String::as_str).unwrap_or_default();
                match (parent, name.as_str()) {
                    ("url" | "sitemap", "loc") => loc = Some(text.trim().to_string()),
                    ("url" | "sitemap", "lastmod") => lastmod = Some(text.trim().to_string()),
                    (_, "url") => {
                        if let Some(l) = loc.take() {
                            out.urls.push((l, lastmod.take()));
                        }
                        lastmod = None;
                    }
                    (_, "sitemap") => {
                        if let Some(l) = loc.take() {
                            out.child_sitemaps.push(l);
                        }
                        lastmod = None;
                    }
                    _ => {}
                }
                text.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urlset_and_index() {
        let urlset = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://a.com/x?a=1&amp;b=2</loc><lastmod>2026-01-02</lastmod></url>
            <url><loc><![CDATA[https://a.com/y]]></loc></url></urlset>"#;
        let p = parse_sitemap(urlset);
        assert_eq!(
            p.urls,
            vec![
                ("https://a.com/x?a=1&b=2".into(), Some("2026-01-02".into())),
                ("https://a.com/y".into(), None),
            ]
        );

        let index = r#"<sitemapindex><sitemap><loc>https://a.com/s1.xml</loc></sitemap></sitemapindex>"#;
        assert_eq!(parse_sitemap(index).child_sitemaps, vec!["https://a.com/s1.xml"]);
    }
}
