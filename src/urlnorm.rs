use url::Url;

/// Query parameters that only carry tracking state and never change page content.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "fbclid",
    "msclkid",
    "yclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "hsctatracking",
    "_ga",
    "_gl",
    "ref_src",
];

fn is_tracking(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    TRACKING_PARAMS.contains(&k.as_str())
}

/// Canonical form used as the identity of a URL throughout the crawl store.
/// Drops fragments and tracking params; keeps path case and trailing slashes,
/// because those are distinct URLs as far as search engines are concerned.
pub fn normalize(u: &Url) -> Option<Url> {
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let mut u = u.clone();
    u.set_fragment(None);
    match u.query() {
        Some("") => u.set_query(None),
        Some(_) => {
            let pairs: Vec<(String, String)> = u
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let kept: Vec<_> = pairs.iter().filter(|(k, _)| !is_tracking(k)).collect();
            if kept.is_empty() {
                u.set_query(None);
            } else if kept.len() != pairs.len() {
                u.query_pairs_mut().clear().extend_pairs(kept);
            }
        }
        None => {}
    }
    Some(u)
}

pub fn normalize_str(base: &Url, href: &str) -> Option<Url> {
    base.join(href.trim()).ok().and_then(|u| normalize(&u))
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// Which hosts count as "this site". `example.com` and `www.example.com` are treated as one.
#[derive(Debug, Clone)]
pub struct Scope {
    base_host: String,
}

impl Scope {
    pub fn new(seed: &Url) -> Self {
        Self {
            base_host: strip_www(seed.host_str().unwrap_or_default()).to_string(),
        }
    }

    pub fn is_internal(&self, u: &Url) -> bool {
        u.host_str().is_some_and(|h| strip_www(h) == self.base_host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        normalize(&Url::parse(s).unwrap()).unwrap().to_string()
    }

    #[test]
    fn strips_fragment_and_tracking() {
        assert_eq!(n("https://Example.com/a#top"), "https://example.com/a");
        assert_eq!(
            n("https://example.com/a?utm_source=x&id=2"),
            "https://example.com/a?id=2"
        );
        assert_eq!(n("https://example.com/a?utm_source=x"), "https://example.com/a");
        assert_eq!(n("https://example.com/a?"), "https://example.com/a");
    }

    #[test]
    fn keeps_untouched_queries_verbatim() {
        assert_eq!(
            n("https://example.com/s?q=a%20b&p=1"),
            "https://example.com/s?q=a%20b&p=1"
        );
    }

    #[test]
    fn www_is_same_site() {
        let s = Scope::new(&Url::parse("https://www.example.com/").unwrap());
        assert!(s.is_internal(&Url::parse("https://example.com/x").unwrap()));
        assert!(!s.is_internal(&Url::parse("https://blog.example.com/x").unwrap()));
    }
}
