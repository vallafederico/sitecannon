use std::collections::HashSet;
use std::sync::LazyLock;

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;
use xxhash_rust::xxh3::xxh3_64;

use crate::model::{Extracted, Image, LinkOut, LinkPosition};
use crate::urlnorm::{Scope, normalize_str};

macro_rules! sel {
    ($name:ident, $css:expr) => {
        static $name: LazyLock<Selector> = LazyLock::new(|| Selector::parse($css).unwrap());
    };
}

sel!(TITLE, "title");
sel!(ASSETS, "script[src], link[href], img[src]");
sel!(META, "meta");
sel!(LINK_TAG, "link[rel][href]");
sel!(HEADINGS, "h1, h2, h3, h4, h5, h6");
sel!(ANCHORS, "a[href]");
sel!(IMGS, "img");
sel!(JSONLD, r#"script[type="application/ld+json"]"#);
sel!(FORMS, "form");
sel!(IFRAMES, "iframe[src]");
sel!(HTML_TAG, "html");
sel!(BODY, "body");
sel!(MAIN, r#"main, [role="main"]"#);

/// `<template>` is deliberately not here: streaming SSR (SolidStart, React) ships real
/// page content inside template chunks that JavaScript moves into place.
const NON_CONTENT: &[&str] = &["script", "style", "noscript", "svg"];
const SKELETON_MAX_DEPTH: usize = 12;
const MIN_MAIN_WORDS: usize = 50;

pub fn extract(html: &str, page_url: &Url, scope: &Scope) -> Extracted {
    let doc = Html::parse_document(html);
    let mut x = Extracted::default();

    // <title> inside inline SVG icons is an accessibility label, not the page title.
    let titles: Vec<String> = doc
        .select(&TITLE)
        .filter(|e| {
            !e.ancestors()
                .any(|a| a.value().as_element().is_some_and(|el| el.name() == "svg"))
        })
        .map(|e| collapse(&text_of(e)))
        .collect();
    x.title_count = titles.len();
    x.title = titles.into_iter().next();

    x.lang = doc
        .select(&HTML_TAG)
        .next()
        .and_then(|e| e.value().attr("lang"))
        .map(|s| s.trim().to_string());

    for m in doc.select(&META) {
        let el = m.value();
        let content = el.attr("content").map(|s| s.trim().to_string());
        if let Some(name) = el.attr("name").map(str::to_ascii_lowercase) {
            match name.as_str() {
                "description" => x.meta_description = x.meta_description.take().or(content),
                "robots" => x.meta_robots = merge_directives(x.meta_robots.take(), content),
                "googlebot" => x.meta_robots = merge_directives(x.meta_robots.take(), content),
                "generator" => x.generator = x.generator.take().or(content),
                "viewport" => x.has_viewport = true,
                n if n.starts_with("twitter:") => {
                    if let Some(c) = content {
                        x.og.entry(n.to_string()).or_insert(c.into());
                    }
                }
                _ => {}
            }
        } else if let Some(prop) = el.attr("property").map(str::to_ascii_lowercase)
            && (prop.starts_with("og:") || prop.starts_with("article:"))
            && let Some(c) = content
        {
            x.og.entry(prop).or_insert(c.into());
        }
    }

    for l in doc.select(&LINK_TAG) {
        let el = l.value();
        let rel = el.attr("rel").unwrap_or_default().to_ascii_lowercase();
        let href = el.attr("href").unwrap_or_default();
        let resolved = normalize_str(page_url, href).map(|u| u.to_string());
        let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
        if rels.contains(&"canonical") {
            x.canonical_count += 1;
            if x.canonical.is_none() {
                x.canonical = resolved;
            }
        } else if rels.contains(&"alternate") && el.attr("hreflang").is_some() {
            if let Some(r) = resolved {
                x.hreflang
                    .push((el.attr("hreflang").unwrap().trim().to_string(), r));
            }
        } else if rels.contains(&"next") {
            x.pagination_next = x.pagination_next.take().or(resolved);
        } else if rels.contains(&"prev") || rels.contains(&"previous") {
            x.pagination_prev = x.pagination_prev.take().or(resolved);
        }
    }

    for h in doc.select(&HEADINGS) {
        let level = h.value().name()[1..].parse::<u8>().unwrap_or(0);
        let text = collapse(&text_of(h));
        if level == 1 {
            x.h1.push(text.clone());
        }
        x.headings.push((level, text));
    }

    for s in doc.select(&JSONLD) {
        let raw = s.text().collect::<String>();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
            collect_types(&v, &mut x.jsonld_types);
            x.jsonld.push(v);
        }
    }
    x.jsonld_types.sort();
    x.jsonld_types.dedup();

    for a in doc.select(&ANCHORS) {
        x.links.push(link_out(a, page_url, scope));
    }

    for img in doc.select(&IMGS) {
        let el = img.value();
        let src = el.attr("src").or_else(|| el.attr("data-src")).unwrap_or_default();
        if src.is_empty() {
            continue;
        }
        let src = page_url
            .join(src)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| src.to_string());
        x.images.push(Image {
            src,
            alt: el.attr("alt").map(|a| a.trim().to_string()),
        });
    }

    x.forms = doc.select(&FORMS).count();
    x.iframes = doc
        .select(&IFRAMES)
        .filter_map(|e| e.value().attr("src"))
        .map(|s| {
            page_url
                .join(s)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| s.to_string())
        })
        .collect();

    x.platform = detect_platform(&doc, html, x.generator.as_deref());

    if let Some(body) = doc.select(&BODY).next() {
        x.word_count = visible_words(body).len();

        let mut paths = HashSet::new();
        skeleton_paths(body, &mut Vec::new(), &mut paths);
        x.skeleton_hash = simhash(paths.iter().map(String::as_str));
    }

    x.full_markdown = doc
        .select(&BODY)
        .next()
        .and_then(|b| to_markdown(&without_chrome(b)));
    let (main, source) = match doc.select(&MAIN).next() {
        Some(m) => (to_markdown(&without_chrome(m)), "main_element"),
        None => (readability_markdown(html, page_url), "readability"),
    };
    let main_words = main.as_deref().map(count_words).unwrap_or(0);
    let full_words = x.full_markdown.as_deref().map(count_words).unwrap_or(0);
    // Readability gives up on short or component-heavy pages; fall back to the whole page.
    (x.main_markdown, x.main_source) = if main_words < MIN_MAIN_WORDS && full_words > main_words {
        (x.full_markdown.clone(), "full_page")
    } else {
        (main, source)
    };
    x.main_word_count = x.main_markdown.as_deref().map(count_words).unwrap_or(0);
    // Fingerprint the main content only: shared menus and footers would make every
    // short page look like a duplicate of every other.
    x.content_hash = content_simhash(x.main_markdown.as_deref().unwrap_or_default());

    x
}

fn link_out(a: ElementRef, page_url: &Url, scope: &Scope) -> LinkOut {
    let el = a.value();
    let href = el.attr("href").unwrap_or_default().trim().to_string();
    let mut scheme = href
        .split_once(':')
        .filter(|(s, _)| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_else(|| page_url.scheme().to_string());
    let mut target = normalize_str(page_url, &href);
    // Cloudflare rewrites mailto: links to /cdn-cgi/l/email-protection; they're emails, not pages.
    if target
        .as_ref()
        .is_some_and(|u| u.path().starts_with("/cdn-cgi/l/email-protection"))
    {
        target = None;
        scheme = "mailto".into();
    }
    let internal = target.as_ref().is_some_and(|u| scope.is_internal(u));

    let mut anchor = collapse(&text_of(a));
    let mut is_image_link = false;
    if anchor.is_empty() {
        if let Some(alt) = a.select(&IMGS).find_map(|i| i.value().attr("alt")) {
            anchor = collapse(alt);
            is_image_link = true;
        } else if let Some(label) = el.attr("aria-label").or_else(|| el.attr("title")) {
            anchor = collapse(label);
        }
    }

    let rel = el
        .attr("rel")
        .map(|r| r.trim().to_ascii_lowercase())
        .filter(|r| !r.is_empty());
    let nofollow = rel
        .as_deref()
        .is_some_and(|r| r.split_ascii_whitespace().any(|t| t == "nofollow"));

    let position = link_position(a);
    LinkOut {
        href,
        target: target.map(|u| u.to_string()),
        scheme,
        internal,
        anchor,
        is_image_link,
        rel,
        nofollow,
        position,
        menu_group: menu_group(a, position),
    }
}

/// Labels a link with the dropdown or footer column it sits under ("Resources", "Company"...).
/// Walks up from the link and takes the nearest *preceding* sibling that looks like a menu
/// toggle or column heading. Covers Webflow dropdowns (toggle + list), WordPress
/// (`li > a + ul.sub-menu`), `<details><summary>`, ARIA disclosure buttons and footer
/// columns with a heading.
fn menu_group(a: ElementRef, position: LinkPosition) -> Option<String> {
    if !matches!(
        position,
        LinkPosition::Nav | LinkPosition::Header | LinkPosition::Footer
    ) {
        return None;
    }
    let mut branch = a.id();
    for (level, anc) in a.ancestors().enumerate() {
        let Some(parent) = ElementRef::wrap(anc) else {
            break;
        };
        if level >= 8 || matches!(parent.value().name(), "body" | "html") {
            break;
        }
        for child in parent.children() {
            if child.id() == branch {
                break;
            }
            let Some(c) = ElementRef::wrap(child) else {
                continue;
            };
            if is_group_label(c, parent) {
                let label = collapse(&text_of(c));
                let lower = label.to_ascii_lowercase();
                if !label.is_empty()
                    && label.chars().count() <= 60
                    && !matches!(
                        lower.as_str(),
                        "menu" | "open menu" | "close" | "close menu" | "back" | "toggle navigation"
                    )
                {
                    return Some(label);
                }
            }
        }
        if matches!(parent.value().name(), "header" | "footer") {
            break;
        }
        branch = anc.id();
    }
    None
}

fn is_group_label(c: ElementRef, parent: ElementRef) -> bool {
    let el = c.value();
    if matches!(el.name(), "button" | "summary" | "h2" | "h3" | "h4" | "h5" | "h6") {
        return true;
    }
    if el.attr("aria-haspopup").is_some() || el.attr("aria-expanded").is_some() {
        return true;
    }
    if el.name() == "a" && parent.value().name() == "li" {
        return true;
    }
    el.classes().any(|cl| {
        let cl = cl.to_ascii_lowercase();
        ["toggle", "trigger", "heading", "title", "label"]
            .iter()
            .any(|k| cl.contains(k))
    })
}

/// Classifies a link by the layout region it sits in. Semantic tags and ARIA roles first,
/// then class/id heuristics for div-soup builders (Webflow `w-nav`, `navbar`, `site-footer`...).
fn link_position(a: ElementRef) -> LinkPosition {
    let (mut footer, mut nav, mut header, mut aside, mut main) = (false, false, false, false, false);
    for anc in a.ancestors() {
        let Some(e) = anc.value().as_element() else {
            continue;
        };
        let role = e.attr("role").unwrap_or_default();
        let ident = format!(
            "{} {}",
            e.attr("class").unwrap_or_default(),
            e.attr("id").unwrap_or_default()
        )
        .to_ascii_lowercase();
        let label = e.attr("aria-label").unwrap_or_default().to_ascii_lowercase();

        if ident.contains("breadcrumb")
            || label.contains("breadcrumb")
            || e.attr("itemtype").is_some_and(|t| t.contains("BreadcrumbList"))
        {
            return LinkPosition::Breadcrumb;
        }
        let tokens = ident.split_ascii_whitespace();
        match e.name() {
            "footer" => footer = true,
            "nav" => nav = true,
            "header" => header = true,
            "aside" => aside = true,
            "main" | "article" => main = true,
            _ => {}
        }
        match role {
            "contentinfo" => footer = true,
            "navigation" | "menubar" => nav = true,
            "banner" => header = true,
            "complementary" => aside = true,
            "main" => main = true,
            _ => {}
        }
        for t in tokens {
            if t.contains("footer") {
                footer = true;
            } else if t == "nav"
                || t.starts_with("nav")
                || t.starts_with("w-nav")
                || t.contains("navbar")
                || t.contains("navigation")
                || t.contains("menu")
            {
                nav = true;
            }
        }
    }
    if footer {
        LinkPosition::Footer
    } else if nav {
        LinkPosition::Nav
    } else if header {
        LinkPosition::Header
    } else if aside {
        LinkPosition::Aside
    } else if main {
        LinkPosition::Main
    } else {
        LinkPosition::Body
    }
}

/// Identifies CMS, site builder and front-end framework. Asset URLs (scripts, stylesheets,
/// images) are checked rather than the whole document, so a blog post that merely links to a
/// WordPress site isn't mistaken for one.
fn detect_platform(doc: &Html, html: &str, generator: Option<&str>) -> Vec<String> {
    let g = generator.unwrap_or_default().to_ascii_lowercase();
    let assets: String = doc
        .select(&ASSETS)
        .filter_map(|e| e.value().attr("src").or_else(|| e.value().attr("href")))
        .collect::<Vec<_>>()
        .join(" ");
    let has = |s: &str| assets.contains(s);
    let checks: &[(&str, bool)] = &[
        // Builders and CMSs
        ("webflow", html.contains("data-wf-site") || g.contains("webflow")),
        ("framer", g.contains("framer") || has("framerusercontent.com")),
        (
            "wordpress",
            g.contains("wordpress") || has("/wp-content/themes/") || has("/wp-includes/"),
        ),
        ("shopify", has("cdn.shopify.com")),
        (
            "squarespace",
            g.contains("squarespace") || has("static1.squarespace.com"),
        ),
        ("wix", g.contains("wix.com") || has("static.wixstatic.com")),
        ("hubspot-cms", g.contains("hubspot") || has("hubspotusercontent")),
        (
            "drupal",
            g.contains("drupal") || has("/sites/default/files/") || has("/core/misc/drupal"),
        ),
        ("ghost", g.contains("ghost")),
        ("sanity", has("cdn.sanity.io")),
        ("contentful", has("ctfassets.net")),
        ("storyblok", has("a.storyblok.com")),
        ("prismic", has("images.prismic.io")),
        // Front-end frameworks
        ("nextjs", html.contains("__NEXT_DATA__") || has("/_next/static/")),
        ("nuxt", html.contains("__NUXT__") || has("/_nuxt/")),
        ("solidstart", html.contains("_$HY") || has("/_build/assets/")),
        ("sveltekit", has("/_app/immutable/")),
        ("remix", html.contains("__remixContext")),
        ("gatsby", html.contains("___gatsby")),
        ("astro", html.contains("astro-island") || g.contains("astro")),
        ("hugo", g.contains("hugo")),
        // Content streamed in chunks and moved into place by JavaScript.
        (
            "streaming-ssr",
            html.contains("<template id=\"pl-") || html.contains("$RC("),
        ),
    ];
    checks
        .iter()
        .filter(|(_, hit)| *hit)
        .map(|(n, _)| n.to_string())
        .collect()
}

/// Re-serialises an element without site chrome (header/nav/footer, by tag, ARIA role or
/// builder class names like Webflow's `div.navbar`), keeping only attributes markdown needs.
fn without_chrome(root: ElementRef) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    fn write(node: ego_tree::NodeRef<Node>, out: &mut String) {
        match node.value() {
            Node::Text(t) => out.push_str(&esc(t)),
            // Template contents live in a document fragment; inline them without the wrapper.
            Node::Fragment | Node::Document => node.children().for_each(|c| write(c, out)),
            Node::Element(e) if e.name() == "template" => node.children().for_each(|c| write(c, out)),
            Node::Element(e) => {
                let Some(el) = ElementRef::wrap(node) else { return };
                if NON_CONTENT.contains(&e.name())
                    || crate::templates::is_hidden(el)
                    || crate::templates::is_chrome(el, crate::templates::stable_class(el).as_deref())
                {
                    return;
                }
                out.push('<');
                out.push_str(e.name());
                for attr in ["href", "src", "alt", "title"] {
                    if let Some(v) = e.attr(attr) {
                        out.push_str(&format!(" {attr}=\"{}\"", esc(v)));
                    }
                }
                out.push('>');
                for child in node.children() {
                    write(child, out);
                }
                out.push_str(&format!("</{}>", e.name()));
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for child in root.children() {
        write(child, &mut out);
    }
    out
}

/// Converts HTML to markdown, keeping hero/feature/CTA sections and stripping only site chrome.
fn to_markdown(html: &str) -> Option<String> {
    let conv = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "head", "script", "style", "noscript", "template", "svg", "iframe", "nav", "footer", "form",
        ])
        .build();
    conv.convert(html)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn readability_markdown(html: &str, page_url: &Url) -> Option<String> {
    let mut r = dom_smoothie::Readability::new(html, Some(page_url.as_str()), None).ok()?;
    let article = r.parse().ok()?;
    to_markdown(&article.content)
}

fn merge_directives(prev: Option<String>, next: Option<String>) -> Option<String> {
    match (prev, next) {
        (Some(p), Some(n)) => Some(format!("{p}, {n}")),
        (p, n) => p.or(n),
    }
}

fn collect_types(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_types(i, out)),
        serde_json::Value::Object(o) => {
            match o.get("@type") {
                Some(serde_json::Value::String(t)) => out.push(t.clone()),
                Some(serde_json::Value::Array(ts)) => {
                    out.extend(ts.iter().filter_map(|t| t.as_str().map(String::from)))
                }
                _ => {}
            }
            if let Some(g) = o.get("@graph") {
                collect_types(g, out);
            }
        }
        _ => {}
    }
}

fn text_of(e: ElementRef) -> String {
    e.text().collect::<Vec<_>>().join(" ")
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn count_words(s: &str) -> usize {
    s.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

fn visible_words(body: ElementRef) -> Vec<String> {
    let mut words = Vec::new();
    for node in body.descendants() {
        let Node::Text(t) = node.value() else { continue };
        let hidden = node.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|e| NON_CONTENT.contains(&e.name()))
        });
        if !hidden {
            words.extend(
                t.split_whitespace()
                    .filter(|w| w.chars().any(char::is_alphanumeric))
                    .map(str::to_lowercase),
            );
        }
    }
    words
}

/// SimHash of word 3-grams. Close hashes (few differing bits) mean near-identical text.
pub fn content_simhash(text: &str) -> u64 {
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    simhash(shingles(&words, 3).iter().map(String::as_str))
}

fn shingles(words: &[String], n: usize) -> Vec<String> {
    if words.len() < n {
        return vec![words.join(" ")];
    }
    words.windows(n).map(|w| w.join(" ")).collect()
}

/// Text-level tags. Unclassed ones are content, not layout, and are left out of the
/// template skeleton so two blog posts with different formatting still match.
const INLINE_CONTENT: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "blockquote",
    "figure",
    "figcaption",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
    "strong",
    "em",
    "b",
    "i",
    "u",
    "a",
    "span",
    "img",
    "picture",
    "source",
    "br",
    "hr",
    "code",
    "pre",
    "small",
    "sup",
    "sub",
    "mark",
    "time",
    "video",
    "audio",
];
/// Rich-text containers (CMS body fields): their insides are content, never layout.
const RICH_TEXT_HINTS: &[&str] = &[
    "richtext",
    "rich-text",
    "entry-content",
    "post-content",
    "prose",
    "article-body",
];

/// Collects tag/class paths of layout containers, e.g. `section.section_hero>div.container`.
fn skeleton_paths(node: ElementRef, path: &mut Vec<String>, out: &mut HashSet<String>) {
    for child in node.children() {
        let Some(el) = ElementRef::wrap(child) else {
            continue;
        };
        let name = el.value().name();
        if NON_CONTENT.contains(&name) {
            continue;
        }
        // State classes (`w--current`, `is-active`) differ per page on the same template.
        let class = el.value().classes().find(|c| {
            let c = c.to_ascii_lowercase();
            !(c.starts_with("w--") || c.starts_with("is-") || c.contains("active") || c.contains("current"))
        });
        if INLINE_CONTENT.contains(&name) && class.is_none() {
            continue;
        }
        path.push(match class {
            Some(c) => format!("{name}.{c}"),
            None => name.to_string(),
        });
        out.insert(path.join(">"));
        let rich_text = el.value().classes().any(|c| {
            let c = c.to_ascii_lowercase();
            RICH_TEXT_HINTS.iter().any(|h| c.contains(h))
        });
        if path.len() < SKELETON_MAX_DEPTH && !rich_text {
            skeleton_paths(el, path, out);
        }
        path.pop();
    }
}

fn simhash<'a>(features: impl Iterator<Item = &'a str>) -> u64 {
    let mut v = [0i32; 64];
    let mut any = false;
    for f in features {
        any = true;
        let h = xxh3_64(f.as_bytes());
        for (i, slot) in v.iter_mut().enumerate() {
            *slot += if (h >> i) & 1 == 1 { 1 } else { -1 };
        }
    }
    if !any {
        return 0;
    }
    v.iter()
        .enumerate()
        .fold(0u64, |acc, (i, &c)| if c > 0 { acc | (1 << i) } else { acc })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html lang="en"><head>
        <title>Pricing | Acme</title>
        <meta name="description" content="Plans for teams">
        <meta name="robots" content="noindex">
        <link rel="canonical" href="/pricing">
        <link rel="alternate" hreflang="de" href="https://acme.com/de/pricing">
        <script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"Organization"},{"@type":["WebPage","FAQPage"]}]}</script>
        </head><body>
        <header><nav><a href="/">Home</a><a href="/features?utm_source=nav">Features</a></nav></header>
        <div class="breadcrumbs"><a href="/">Acme</a></div>
        <main><svg><title>Expand section</title></svg><h1>Pricing</h1><p>Simple pricing for every team size.</p>
          <a href="https://other.com/x">Partner</a><a href="mailto:hi@acme.com">Email</a>
          <a href="/cdn-cgi/l/email-protection#abc">Protected</a>
          <a href="/signup"><img src="/cta.png" alt="Start free"></a></main>
        <div class="site-footer"><a href="/legal" rel="nofollow">Legal</a></div>
        </body></html>"#;

    #[test]
    fn extracts_seo_fields_and_link_positions() {
        let url = Url::parse("https://www.acme.com/pricing").unwrap();
        let x = extract(PAGE, &url, &Scope::new(&url));
        assert_eq!(x.title.as_deref(), Some("Pricing | Acme"));
        assert_eq!(x.title_count, 1, "SVG <title> must not count");
        assert_eq!(x.meta_robots.as_deref(), Some("noindex"));
        assert_eq!(x.canonical.as_deref(), Some("https://www.acme.com/pricing"));
        assert_eq!(x.hreflang.len(), 1);
        assert_eq!(x.jsonld_types, vec!["FAQPage", "Organization", "WebPage"]);
        assert_eq!(x.h1, vec!["Pricing"]);

        let by_anchor = |a: &str| x.links.iter().find(|l| l.anchor == a).unwrap();
        assert_eq!(by_anchor("Features").position, LinkPosition::Nav);
        assert_eq!(
            by_anchor("Features").target.as_deref(),
            Some("https://www.acme.com/features")
        );
        assert_eq!(by_anchor("Acme").position, LinkPosition::Breadcrumb);
        assert_eq!(by_anchor("Partner").position, LinkPosition::Main);
        assert!(!by_anchor("Partner").internal);
        assert_eq!(by_anchor("Email").scheme, "mailto");
        assert!(by_anchor("Email").target.is_none());
        assert_eq!(by_anchor("Protected").scheme, "mailto");
        assert!(by_anchor("Protected").target.is_none());
        assert!(by_anchor("Start free").is_image_link);
        let legal = by_anchor("Legal");
        assert_eq!(legal.position, LinkPosition::Footer);
        assert!(legal.nofollow);
        assert_eq!(legal.menu_group, None);
    }

    #[test]
    fn full_markdown_drops_div_based_chrome() {
        let html = r#"<html><body><div class="navbar w-nav"><a href="/a">Agency</a><a href="/p">Products</a></div>
            <div class="page"><h1>Subscribe</h1><div class="w-form-done">Thanks!</div><p>Get the newsletter &amp; updates.</p><img src="/x.png" alt="Hero"></div>
            <div class="footer_component"><a href="/legal">Legal</a></div></body></html>"#;
        let url = Url::parse("https://acme.com/subscribe").unwrap();
        let md = extract(html, &url, &Scope::new(&url)).full_markdown.unwrap();
        assert!(
            md.contains("# Subscribe")
                && md.contains("newsletter & updates")
                && md.contains("![Hero](/x.png)")
        );
        assert!(
            !md.contains("Agency") && !md.contains("Legal") && !md.contains("Thanks"),
            "{md}"
        );
    }

    #[test]
    fn streamed_template_content_counts_as_content() {
        let html = r#"<html><body><main><template id="pl-1"></template></main><footer>f</footer>
            <template id="pl-1"><article><h2>Streamed story</h2><p>Body text arrives late.</p></article></template>
            <div hidden id="S:0"><p>React streamed paragraph.</p></div></body></html>"#;
        let url = Url::parse("https://acme.com/topic").unwrap();
        let x = extract(html, &url, &Scope::new(&url));
        let md = x.full_markdown.unwrap();
        assert!(
            md.contains("Streamed story") && md.contains("React streamed paragraph"),
            "{md}"
        );
        assert!(x.word_count >= 8);
        assert!(x.platform.contains(&"streaming-ssr".to_string()));
    }

    #[test]
    fn groups_menu_links_under_their_dropdown_or_column() {
        let html = r#"<html><body>
            <div class="navbar w-nav"><nav class="nav-menu">
              <a href="/pricing">Pricing</a>
              <div class="w-dropdown"><div class="w-dropdown-toggle"><div>Resources</div></div>
                <nav class="w-dropdown-list"><a href="/blog">Blog</a><a href="/docs">Docs</a></nav></div>
            </nav></div>
            <ul class="menu"><li class="menu-item-has-children"><a href="/services">Services</a>
              <ul class="sub-menu"><li><a href="/services/seo">SEO</a></li></ul></li></ul>
            <footer><div class="col"><h4>Company</h4><ul><li><a href="/about">About</a></li></ul></div></footer>
            </body></html>"#;
        let url = Url::parse("https://acme.com/").unwrap();
        let x = extract(html, &url, &Scope::new(&url));
        let group = |a: &str| x.links.iter().find(|l| l.anchor == a).unwrap().menu_group.clone();
        assert_eq!(group("Pricing"), None);
        assert_eq!(group("Blog").as_deref(), Some("Resources"));
        assert_eq!(group("Docs").as_deref(), Some("Resources"));
        assert_eq!(group("Services"), None);
        assert_eq!(group("SEO").as_deref(), Some("Services"));
        assert_eq!(group("About").as_deref(), Some("Company"));
    }

    #[test]
    fn same_template_has_close_skeletons() {
        let a =
            "<html><body><header><nav><a>x</a></nav></header><main><h1>A</h1><p>one</p></main></body></html>";
        let b =
            "<html><body><header><nav><a>y</a></nav></header><main><h1>B</h1><p>two</p></main></body></html>";
        let c = "<html><body><div><table><tr><td>z</td></tr></table></div><form><input></form></body></html>";
        let url = Url::parse("https://acme.com/").unwrap();
        let s = Scope::new(&url);
        let (ha, hb, hc) = (
            extract(a, &url, &s).skeleton_hash,
            extract(b, &url, &s).skeleton_hash,
            extract(c, &url, &s).skeleton_hash,
        );
        assert_eq!((ha ^ hb).count_ones(), 0);
        assert!((ha ^ hc).count_ones() > 10);
    }
}
