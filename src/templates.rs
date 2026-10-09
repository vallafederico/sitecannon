//! Page-template detection from stored HTML.
//!
//! A page's template is described by the layout containers inside its main content region
//! (`section.section_blog-header > div.container > ...`). Site chrome (header, nav, footer)
//! is excluded because it is shared by every template and would drown out the differences.

use std::collections::HashSet;
use std::sync::LazyLock;

use scraper::{ElementRef, Html, Selector};
use xxhash_rust::xxh3::xxh3_64;

static MAIN: LazyLock<Selector> = LazyLock::new(|| Selector::parse(r#"main, [role="main"]"#).unwrap());
static BODY: LazyLock<Selector> = LazyLock::new(|| Selector::parse("body").unwrap());

/// How many container levels below the content root describe a template.
const MAX_DEPTH: usize = 5;

const SKIP: &[&str] = &["script", "style", "noscript", "template", "svg", "iframe", "form"];
const CHROME: &[&str] = &["header", "nav", "footer", "aside"];
/// Text-level tags: without a class they are content, not layout.
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
/// CMS rich-text bodies: their insides are content, never layout.
const RICH_TEXT_HINTS: &[&str] = &[
    "richtext",
    "rich-text",
    "entry-content",
    "post-content",
    "prose",
    "article-body",
];

/// Hashed layout paths of the page's main content region, sorted for fast set comparison.
pub fn layout_features(html: &str) -> Vec<u64> {
    let doc = Html::parse_document(html);
    let Some(root) = doc.select(&MAIN).next().or_else(|| doc.select(&BODY).next()) else {
        return Vec::new();
    };
    let mut paths = HashSet::new();
    walk(root, &mut Vec::new(), &mut paths);
    let mut out: Vec<u64> = paths.iter().map(|p| xxh3_64(p.as_bytes())).collect();
    out.sort_unstable();
    out
}

pub fn stable_class(el: ElementRef) -> Option<String> {
    // State classes (`w--current`, `is-active`) differ between pages built from one template.
    el.value().classes().map(str::to_ascii_lowercase).find(|c| {
        !(c.starts_with("w--") || c.starts_with("is-") || c.contains("active") || c.contains("current"))
    })
}

/// Elements present in the HTML but not shown: Webflow conditional visibility and form
/// state messages, `hidden`, `aria-hidden`, inline `display:none`.
pub fn is_hidden(el: ElementRef) -> bool {
    let e = el.value();
    // React streaming SSR parks content in <div hidden id="S:n"> until JavaScript swaps it in.
    if e.id().is_some_and(|id| id.starts_with("S:")) {
        return false;
    }
    e.attr("hidden").is_some()
        || e.attr("aria-hidden") == Some("true")
        || e.attr("style")
            .is_some_and(|s| s.replace(' ', "").contains("display:none"))
        || e.classes()
            .any(|c| matches!(c, "w-condition-invisible" | "w-form-done" | "w-form-fail"))
}

pub fn is_chrome(el: ElementRef, class: Option<&str>) -> bool {
    if CHROME.contains(&el.value().name()) {
        return true;
    }
    if matches!(
        el.value().attr("role"),
        Some("navigation" | "banner" | "contentinfo" | "complementary")
    ) {
        return true;
    }
    class.is_some_and(|c| {
        c.contains("navbar") || c.contains("footer") || c.starts_with("nav") || c == "header"
    })
}

fn walk(node: ElementRef, path: &mut Vec<String>, out: &mut HashSet<String>) {
    for child in node.children() {
        let Some(el) = ElementRef::wrap(child) else {
            continue;
        };
        let name = el.value().name();
        if SKIP.contains(&name) || is_hidden(el) {
            continue;
        }
        let class = stable_class(el);
        // Chrome is skipped even inside <main>: some builds wrap the whole app, navbar
        // included, in one <main>.
        if is_chrome(el, class.as_deref()) {
            continue;
        }
        if INLINE_CONTENT.contains(&name) && class.is_none() {
            continue;
        }
        path.push(match &class {
            Some(c) => format!("{name}.{c}"),
            None => name.to_string(),
        });
        out.insert(path.join(">"));
        let rich_text = class
            .as_deref()
            .is_some_and(|c| RICH_TEXT_HINTS.iter().any(|h| c.contains(h)));
        if path.len() < MAX_DEPTH && !rich_text {
            walk(el, path, out);
        }
        path.pop();
    }
}

/// Jaccard similarity of two sorted, deduplicated feature lists. Two empty pages are identical.
pub fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// Leader clustering: visit pages in priority order; each joins the most similar existing
/// template if similarity ≥ `threshold`, otherwise founds a new one. Unlike single-linkage,
/// it can't chain dissimilar pages together through intermediates.
/// Returns clusters (largest first) of indices into `features`.
pub fn cluster(features: &[Vec<u64>], order: &[usize], threshold: f64) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for &i in order {
        let best = clusters
            .iter()
            .enumerate()
            .map(|(c, members)| (c, jaccard(&features[i], &features[members[0]])))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((c, sim)) if sim >= threshold => clusters[c].push(i),
            _ => clusters.push(vec![i]),
        }
    }
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
    clusters
}

/// Computes features for many pages across all cores.
pub fn layout_features_parallel(htmls: &[String]) -> Vec<Vec<u64>> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let chunk = htmls.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = htmls
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(|h| layout_features(h)).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV: &str = r#"<div class="navbar"><nav class="menu"><a class="link">A</a><div class="dropdown"><a class="link">B</a></div></nav></div>"#;

    fn page(main: &str) -> String {
        format!("<html><body>{NAV}<main>{main}</main><footer class=f><a>x</a></footer></body></html>")
    }

    #[test]
    fn same_template_different_content_matches() {
        let a = page(
            r#"<section class="section_post"><div class="container"><div class="w-richtext"><h2>Hi</h2><p>One</p><ul><li>x</li></ul></div></div></section>"#,
        );
        let b = page(
            r#"<section class="section_post"><div class="container"><div class="w-richtext"><p>Two</p><table><tr><td>y</td></tr></table></div></div></section>"#,
        );
        assert_eq!(jaccard(&layout_features(&a), &layout_features(&b)), 1.0);
    }

    #[test]
    fn different_templates_differ_despite_shared_chrome() {
        let a = page(
            r#"<section class="section_post"><div class="container"><div class="w-richtext"><p>x</p></div></div></section>"#,
        );
        let b = page(
            r#"<section class="section_hero"><div class="hero_grid"><div class="card"></div></div></section><section class="section_pricing"></section>"#,
        );
        assert!(jaccard(&layout_features(&a), &layout_features(&b)) < 0.2);
    }

    #[test]
    fn chrome_inside_main_is_ignored() {
        let wrap = |inner: &str| {
            format!(
                r#"<html><body><main class="app"><nav class="navbar"><div class="nav-block"><a class="nav-link">A</a></div><div class="nav-box"><div class="f-mise"></div></div></nav><div class="content">{inner}</div></main></body></html>"#
            )
        };
        let a = wrap(
            r#"<div class="hero"><div class="index-title"></div></div><div class="world-wrap"><div class="world-list"></div></div>"#,
        );
        let b = wrap(r#"<div class="main"><div class="policy"><div class="policy-block"></div></div></div>"#);
        assert!(jaccard(&layout_features(&a), &layout_features(&b)) < 0.3);
    }

    #[test]
    fn ignores_state_classes() {
        let a = page(r#"<div class="tabs"><a class="tab w--current">1</a><a class="tab">2</a></div>"#);
        let b = page(r#"<div class="tabs"><a class="tab">1</a><a class="tab w--current">2</a></div>"#);
        assert_eq!(layout_features(&a), layout_features(&b));
    }

    #[test]
    fn leader_clustering_groups_similar_pages() {
        let f = vec![
            vec![1, 2, 3, 4],
            vec![1, 2, 3, 5],
            vec![7, 8, 9],
            vec![1, 2, 3, 4],
        ];
        let c = cluster(&f, &[0, 1, 2, 3], 0.6);
        assert_eq!(c, vec![vec![0, 1, 3], vec![2]]);
    }
}
