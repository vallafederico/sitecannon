use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Hop {
    pub url: String,
    pub status: u16,
}

/// Where in the page layout a link sits. Drives nav-tree building and link-equity weighting.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LinkPosition {
    Breadcrumb,
    Footer,
    Nav,
    Header,
    Aside,
    Main,
    Body,
}

impl LinkPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Breadcrumb => "breadcrumb",
            Self::Footer => "footer",
            Self::Nav => "nav",
            Self::Header => "header",
            Self::Aside => "aside",
            Self::Main => "main",
            Self::Body => "body",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinkOut {
    pub href: String,
    /// Normalised absolute target, `None` for mailto:, tel:, javascript: and unparseable hrefs.
    pub target: Option<String>,
    pub scheme: String,
    pub internal: bool,
    pub anchor: String,
    pub is_image_link: bool,
    pub rel: Option<String>,
    pub nofollow: bool,
    pub position: LinkPosition,
    /// Dropdown / footer-column label for nav, header and footer links.
    pub menu_group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub src: String,
    /// `None` = attribute missing, `Some("")` = decorative/empty alt.
    pub alt: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Extracted {
    pub title: Option<String>,
    pub title_count: usize,
    pub meta_description: Option<String>,
    pub meta_robots: Option<String>,
    pub canonical: Option<String>,
    pub canonical_count: usize,
    pub lang: Option<String>,
    pub h1: Vec<String>,
    pub headings: Vec<(u8, String)>,
    pub hreflang: Vec<(String, String)>,
    pub pagination_next: Option<String>,
    pub pagination_prev: Option<String>,
    pub og: serde_json::Map<String, serde_json::Value>,
    pub jsonld: Vec<serde_json::Value>,
    pub jsonld_types: Vec<String>,
    pub generator: Option<String>,
    pub platform: Vec<String>,
    pub has_viewport: bool,
    pub links: Vec<LinkOut>,
    pub images: Vec<Image>,
    pub forms: usize,
    pub iframes: Vec<String>,
    pub word_count: usize,
    pub main_word_count: usize,
    pub main_markdown: Option<String>,
    /// How `main_markdown` was obtained: `main_element`, `readability` or `full_page`.
    pub main_source: &'static str,
    pub full_markdown: Option<String>,
    /// SimHash of the tag-path structure: pages built from the same template land close together.
    pub skeleton_hash: u64,
    /// SimHash of visible text: near-duplicate content lands close together.
    pub content_hash: u64,
    /// Raw HTML as served, kept so analysis can be re-run without re-crawling.
    pub html: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PageRecord {
    pub url: String,
    pub crawl_depth: u32,
    pub discovered_via: &'static str,
    pub found_on: Option<String>,
    pub fetched_at: DateTime<Utc>,
    pub blocked_by_robots: bool,
    pub error: Option<String>,
    /// Status of the first response for this URL (e.g. 301).
    pub status: Option<u16>,
    /// Status after following redirects (equal to `status` when there were none).
    pub final_status: Option<u16>,
    pub final_url: Option<String>,
    /// Every hop including the final response; empty when there was no redirect.
    pub redirect_chain: Vec<Hop>,
    pub content_type: Option<String>,
    pub bytes: Option<u64>,
    pub ttfb_ms: Option<u32>,
    pub total_ms: Option<u32>,
    pub headers: serde_json::Map<String, serde_json::Value>,
    pub extracted: Option<Extracted>,
}

#[derive(Debug, Clone)]
pub struct SitemapEntry {
    pub url: String,
    pub url_norm: Option<String>,
    pub lastmod: Option<String>,
    pub sitemap: String,
}
