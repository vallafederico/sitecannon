//! Content analysis. Splits each page into its sections (hero, features, testimonials, …)
//! and discovers what the site talks about from page text (TF-IDF + spherical k-means),
//! so the report can show topics across the site and where on each page they sit.
//! Deterministic and offline; LLM labelling can be layered on later.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::Result;
use duckdb::{Connection, params};
use scraper::{ElementRef, Html, Node, Selector};

use crate::templates::{is_chrome, is_hidden, stable_class};

static MAIN: LazyLock<Selector> = LazyLock::new(|| Selector::parse(r#"main, [role="main"]"#).unwrap());
static BODY: LazyLock<Selector> = LazyLock::new(|| Selector::parse("body").unwrap());
static HEADING: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h1, h2, h3").unwrap());
static ANY_HEADING: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1, h2, h3, h4, h5, h6, summary, dt").unwrap());
static LINKS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static IMAGES: LazyLock<Selector> = LazyLock::new(|| Selector::parse("img, picture").unwrap());
static FORMS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("form, input[type=email], textarea").unwrap());
static MEDIA: LazyLock<Selector> = LazyLock::new(|| Selector::parse("iframe, video").unwrap());
static QUOTES: LazyLock<Selector> = LazyLock::new(|| Selector::parse("blockquote, q").unwrap());
static DETAILS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("details").unwrap());
static BUTTONS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("button, a[href]").unwrap());

const SKIP: &[&str] = &["script", "style", "noscript", "svg", "template", "head"];
const MAX_SECTIONS: usize = 40;
const MAX_TOPICS: usize = 24;
const SECTION_TOPIC_MIN_WORDS: usize = 25;

#[derive(Debug, Clone)]
pub struct Section {
    pub ordinal: usize,
    pub kind: &'static str,
    pub group: &'static str,
    pub heading: Option<String>,
    pub words: usize,
    pub links: usize,
    pub images: usize,
    pub cards: usize,
    pub class_hint: String,
    pub text: String,
}

/// Section type → the broader role it plays, used to colour page anatomy.
fn group_of(kind: &str) -> &'static str {
    match kind {
        "hero" | "cta" | "form" => "intro & conversion",
        "minor" => "minor",
        "listing" => "lists & navigation",
        "testimonials" | "logos" | "stats" | "team" | "pricing" | "faq" => "proof & detail",
        _ => "content",
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn skipped(el: ElementRef) -> bool {
    if SKIP.contains(&el.value().name()) || is_hidden(el) {
        return true;
    }
    // A <header> inside <main>/<article> is a page or article header, not site chrome.
    if el.value().name() == "header"
        && el
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|a| matches!(a.value().name(), "main" | "article"))
    {
        return false;
    }
    is_chrome(el, stable_class(el).as_deref())
}

/// Visible text of an element, leaving out scripts, hidden elements and site chrome.
fn visible_text(el: ElementRef, out: &mut String) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                out.push_str(t);
                out.push(' ');
            }
            Node::Element(_) => {
                if let Some(c) = ElementRef::wrap(child)
                    && !skipped(c)
                {
                    visible_text(c, out);
                }
            }
            _ => {}
        }
    }
}

fn word_count(s: &str) -> usize {
    s.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

fn content_children(el: ElementRef) -> Vec<(ElementRef, String)> {
    el.children()
        .filter_map(ElementRef::wrap)
        .filter(|c| !skipped(*c))
        .filter_map(|c| {
            let mut t = String::new();
            visible_text(c, &mut t);
            let has_media = c.select(&IMAGES).next().is_some() || c.select(&MEDIA).next().is_some();
            (word_count(&t) > 0 || has_media).then_some((c, t))
        })
        .collect()
}

/// Splits a page's main content into its top-level sections.
pub fn sections(html: &str) -> Vec<Section> {
    let doc = Html::parse_document(html);
    let Some(mut container) = doc.select(&MAIN).next().or_else(|| doc.select(&BODY).next()) else {
        return Vec::new();
    };
    // Descend through wrapper elements until the content fans out into sections.
    for _ in 0..10 {
        let kids = content_children(container);
        if kids.len() == 1 {
            container = kids[0].0;
            continue;
        }
        let total: usize = kids.iter().map(|(_, t)| word_count(t)).sum();
        let dominant = kids.iter().find(|(k, t)| {
            total > 0 && word_count(t) as f64 >= 0.85 * total as f64 && content_children(*k).len() >= 3
        });
        match dominant {
            Some((k, _)) if kids.len() <= 3 => container = *k,
            _ => break,
        }
    }
    content_children(container)
        .into_iter()
        .take(MAX_SECTIONS)
        .enumerate()
        .map(|(i, (el, text))| describe(el, i, collapse(&text)))
        .collect()
}

const RICH_TEXT_HINTS: &[&str] = &[
    "richtext",
    "rich-text",
    "prose",
    "entry-content",
    "post-content",
    "article-body",
];

/// Is this element a card-like container (not a paragraph of running text)?
fn card_like(c: ElementRef) -> bool {
    let name = c.value().name();
    if matches!(
        name,
        "p" | "span"
            | "strong"
            | "em"
            | "b"
            | "i"
            | "br"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
    ) {
        return false;
    }
    stable_class(c).is_some()
        || name == "article"
        || (name == "li"
            && c.children()
                .filter_map(ElementRef::wrap)
                .any(|k| matches!(k.value().name(), "div" | "a" | "h2" | "h3" | "h4" | "img")))
}

/// Largest group of same-shaped card-like siblings with text (cards, tiles, list items) within a section.
fn repeated_cards(el: ElementRef, depth: usize) -> usize {
    if depth > 5 {
        return 0;
    }
    if stable_class(el).is_some_and(|c| RICH_TEXT_HINTS.iter().any(|h| c.contains(h))) {
        return 0;
    }
    let mut groups: HashMap<String, usize> = HashMap::new();
    let mut best = 0;
    for c in el
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|c| !skipped(*c))
    {
        let mut t = String::new();
        visible_text(c, &mut t);
        if word_count(&t) >= 2 && card_like(c) {
            let sig = format!("{}.{}", c.value().name(), stable_class(c).unwrap_or_default());
            *groups.entry(sig).or_default() += 1;
        }
        best = best.max(repeated_cards(c, depth + 1));
    }
    best.max(groups.into_values().max().unwrap_or(0))
}

fn describe(el: ElementRef, ordinal: usize, text: String) -> Section {
    let words = word_count(&text);
    let heading_el = el
        .select(&HEADING)
        .find(|h| !h.ancestors().filter_map(ElementRef::wrap).any(skipped));
    let heading = heading_el
        .map(|h| collapse(&h.text().collect::<String>()))
        .filter(|h| !h.is_empty());
    let has_h1 = el.select(&HEADING).any(|h| h.value().name() == "h1");
    let links = el.select(&LINKS).count();
    let images = el.select(&IMAGES).count();
    let forms = el.select(&FORMS).count();
    let media = el.select(&MEDIA).count();
    let quotes = el.select(&QUOTES).count() + text.matches('“').count().min(text.matches('”').count());
    let details = el.select(&DETAILS).count();
    let questions = el
        .select(&ANY_HEADING)
        .filter(|h| h.text().collect::<String>().trim_end().ends_with('?'))
        .count();
    let buttons = el
        .select(&BUTTONS)
        .filter(|b| {
            b.value().name() == "button"
                || b.value().classes().any(|c| {
                    let c = c.to_ascii_lowercase();
                    c.contains("btn") || c.contains("button") || c.contains("cta")
                })
        })
        .count();
    let numbers = text
        .split_whitespace()
        .filter(|w| {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '%' && c != '+');
            !w.is_empty() && w.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .count();
    let currency = {
        let chars: Vec<char> = text.chars().collect();
        chars
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                matches!(c, '€' | '$' | '£')
                    && (chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())
                        || (*i > 0 && chars[i - 1].is_ascii_digit())
                        || (*i > 1 && chars[i - 2].is_ascii_digit()))
            })
            .count()
            + text.matches("/mo").count()
            + text.to_lowercase().matches("per month").count()
    };
    let cards = repeated_cards(el, 0);

    let mut hint: Vec<String> = Vec::new();
    for e in std::iter::once(el).chain(el.descendants().filter_map(ElementRef::wrap).take(12)) {
        if let Some(c) = stable_class(e) {
            hint.push(c);
        }
        if let Some(id) = e.value().id() {
            hint.push(id.to_ascii_lowercase());
        }
    }
    let class_hint = hint.join(" ");
    let has = |keys: &[&str]| keys.iter().any(|k| class_hint.contains(k));

    let invitation = heading.as_deref().is_some_and(|h| h.contains('?'))
        || [
            "get in touch",
            "contact us",
            "start a conversation",
            "book a demo",
            "get started",
            "sign up",
            "talk to",
        ]
        .iter()
        .any(|p| text.to_lowercase().contains(p));
    let kind = if words < 5 && images == 0 && forms == 0 && quotes == 0 && heading.is_none() {
        "minor"
    } else if (ordinal == 0 && has_h1) || has(&["hero", "banner"]) {
        "hero"
    } else if has(&["testimonial", "review", "quote"]) || (quotes > 0 && words < 400) {
        "testimonials"
    } else if has(&["faq", "accordion"]) || details >= 2 || questions >= 3 {
        "faq"
    } else if has(&["pricing", "price-", "plans"])
        || (currency >= 2
            && words < 400
            && ["plan", "month", "year", "/mo", "annual"]
                .iter()
                .any(|p| text.to_lowercase().contains(p)))
    {
        "pricing"
    } else if words < 60 && links >= 1 && images < 4 && (invitation || buttons >= 1) {
        "cta"
    } else if images >= 3 && (has(&["logo", "client", "partner", "brands", "trusted"]) || words < images * 6)
    {
        "logos"
    } else if forms > 0 && words < 250 {
        "form"
    } else if has(&["cta", "call-to-action", "signup", "get-started"]) {
        "cta"
    } else if media > 0 && words < 120 {
        "video"
    } else if has(&["stat", "metric", "counter", "numbers"])
        || (numbers >= 3 && words < 80 && numbers * 4 >= words)
    {
        "stats"
    } else if has(&["team", "people", "founder"]) && cards >= 3 {
        "team"
    } else if cards >= 3 && links >= cards && words <= cards * 80 {
        "listing"
    } else if cards >= 3 && words <= cards * 80 {
        "features"
    } else if words >= 250 {
        "article"
    } else if has(&["feature", "benefit", "service"]) {
        "features"
    } else if images >= 1 && words < 40 {
        "media"
    } else {
        "content"
    };

    Section {
        ordinal,
        kind,
        group: group_of(kind),
        heading: heading.map(|h| h.chars().take(120).collect()),
        words,
        links,
        images,
        cards,
        class_hint: class_hint.chars().take(200).collect(),
        text,
    }
}

fn sections_parallel(htmls: &[String]) -> Vec<Vec<Section>> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let chunk = htmls.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = htmls
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|h| if h.is_empty() { Vec::new() } else { sections(h) })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

// ---------------------------------------------------------------- topics

const STOPWORDS: &str = "a about above after again against all also am an and any are as at be because been before being \
below between both but by can could did do does doing down during each few for from further had has have having he her \
here hers him his how i if in into is it its itself just me more most my no nor not now of off on once only or other our \
ours out over own same she should so some such than that the their theirs them then there these they this those through \
to too under until up very was we were what when where which while who whom why will with would you your yours \
yourself get got make made use using used one two three new also may many much every via per within without across \
read learn click here view see find discover today now day days year years time way like want need help us page site \
home menu contact cookie cookies privacy policy terms rights reserved copyright email subscribe newsletter follow share \
http https www com html php il lo la le gli un una uno di da del della dei delle degli al alla ai alle nel nella nei \
che chi cui non per con su tra fra come anche più piu sono essere ha hanno questo questa questi queste quello quella \
el los las del por para con una unos unas que como mas der die das und den dem des ein eine mit von zu auf ist les \
des une pour avec dans sur est sont able actually add allow always another around available best better big \
build built come complete different easy ensure ensuring even ever everything example first free full give go good great \
high important include including keep know last let long look lot low main means might must next offer often part \
place please possible provide put real really right run say set simple since small start still sure take thing things \
think top true try well whether work works information device etc yes don doesn didn isn aren wasn weren won wouldn \
couldn shouldn haven hasn ll ve re min mins read reading january february march april may june july august \
september october november december jan feb mar apr jun jul aug sep sept oct nov dec monday tuesday wednesday \
thursday friday saturday sunday gennaio febbraio marzo aprile maggio giugno luglio agosto settembre ottobre novembre \
dicembre related";

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOPWORDS.split_whitespace().collect());

/// Words of a markdown document, with link targets removed and stopwords dropped.
fn tokens(md: &str) -> Vec<String> {
    let mut clean = String::with_capacity(md.len());
    let mut chars = md.chars().peekable();
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        // Skip "(...)" right after "]": markdown link and image targets.
        if c == '(' && prev == ']' {
            let mut depth = 1;
            for n in chars.by_ref() {
                if n == '(' {
                    depth += 1;
                } else if n == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            prev = ')';
            continue;
        }
        clean.push(c);
        prev = c;
    }
    clean
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| {
            w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(w.as_str())
        })
        .collect()
}

/// Unigrams plus adjacent-word bigrams.
fn terms(md: &str) -> Vec<String> {
    let t = tokens(md);
    let mut out = t.clone();
    out.extend(t.windows(2).map(|w| format!("{} {}", w[0], w[1])));
    out
}

type SparseVec = Vec<(u32, f32)>;

struct Vocab {
    index: HashMap<String, u32>,
    terms: Vec<String>,
    idf: Vec<f32>,
}

impl Vocab {
    fn build(docs: &[Vec<String>], exclude: &HashSet<String>) -> Self {
        let n = docs.iter().filter(|d| !d.is_empty()).count().max(1);
        let mut df: HashMap<&str, usize> = HashMap::new();
        for d in docs {
            for t in d.iter().map(String::as_str).collect::<HashSet<_>>() {
                *df.entry(t).or_default() += 1;
            }
        }
        let min_df = if n >= 10 { 2 } else { 1 };
        // Terms on more than half the pages are boilerplate, not topics.
        let max_df = if n >= 10 { n / 2 } else { n };
        let mut kept: Vec<(&str, usize)> = df
            .into_iter()
            .filter(|(t, c)| *c >= min_df && *c <= max_df && !exclude.contains(*t))
            .collect();
        kept.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        kept.truncate(30_000);
        let terms: Vec<String> = kept.iter().map(|(t, _)| t.to_string()).collect();
        let idf = kept
            .iter()
            .map(|(_, c)| ((n as f32 + 1.0) / (*c as f32 + 1.0)).ln() + 1.0)
            .collect();
        let index = terms
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        Self { index, terms, idf }
    }

    fn vectorize(&self, doc: &[String]) -> SparseVec {
        let mut tf: HashMap<u32, f32> = HashMap::new();
        for t in doc {
            if let Some(&i) = self.index.get(t) {
                *tf.entry(i).or_default() += 1.0;
            }
        }
        let mut v: SparseVec = tf
            .into_iter()
            .map(|(i, c)| (i, (1.0 + c.ln()) * self.idf[i as usize]))
            .collect();
        let norm = v.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|(_, w)| *w /= norm);
        }
        v.sort_unstable_by_key(|(i, _)| *i);
        v
    }
}

fn dot(v: &SparseVec, c: &[f32]) -> f32 {
    v.iter().map(|(i, w)| w * c[*i as usize]).sum()
}

/// Spherical k-means with deterministic k-means++ seeding. Returns (assignment, centroids).
fn kmeans(vecs: &[SparseVec], dims: usize, k: usize) -> (Vec<usize>, Vec<Vec<f32>>) {
    let n = vecs.len();
    let mut rng: u64 = 0x5eed_c0de;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let dense = |v: &SparseVec| {
        let mut d = vec![0f32; dims];
        v.iter().for_each(|(i, w)| d[*i as usize] = *w);
        d
    };
    let mut centroids: Vec<Vec<f32>> = vec![dense(&vecs[(rand() * n as f64) as usize % n])];
    while centroids.len() < k {
        let dist: Vec<f64> = vecs
            .iter()
            .map(|v| {
                let best = centroids.iter().map(|c| dot(v, c)).fold(f32::MIN, f32::max);
                (1.0 - best.max(0.0) as f64).powi(2)
            })
            .collect();
        let total: f64 = dist.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut r = rand() * total;
        let mut pick = n - 1;
        for (i, d) in dist.iter().enumerate() {
            r -= d;
            if r <= 0.0 {
                pick = i;
                break;
            }
        }
        centroids.push(dense(&vecs[pick]));
    }

    let mut assign = vec![0usize; n];
    for _ in 0..30 {
        let mut changed = false;
        for (i, v) in vecs.iter().enumerate() {
            let best = (0..centroids.len())
                .max_by(|&a, &b| dot(v, &centroids[a]).total_cmp(&dot(v, &centroids[b])))
                .unwrap_or(0);
            if assign[i] != best {
                assign[i] = best;
                changed = true;
            }
        }
        let mut sums = vec![vec![0f32; dims]; centroids.len()];
        let mut counts = vec![0usize; centroids.len()];
        for (v, &a) in vecs.iter().zip(&assign) {
            counts[a] += 1;
            v.iter().for_each(|(i, w)| sums[a][*i as usize] += w);
        }
        for (c, (sum, count)) in centroids.iter_mut().zip(sums.into_iter().zip(counts)) {
            if count == 0 {
                continue;
            }
            let norm = sum.iter().map(|w| w * w).sum::<f32>().sqrt();
            if norm > 0.0 {
                *c = sum.into_iter().map(|w| w / norm).collect();
            }
        }
        if !changed {
            break;
        }
    }
    (assign, centroids)
}

/// Picks up to three label terms from a centroid, preferring phrases and skipping words
/// already covered by a chosen phrase.
/// Picks up to `n` label terms for topic `k`: the terms that weigh most in this topic
/// relative to the others, preferring phrases and skipping words a chosen phrase covers.
fn label(k: usize, centroids: &[Vec<f32>], vocab: &Vocab, n: usize) -> Vec<String> {
    let c = &centroids[k];
    let others = centroids.len().saturating_sub(1).max(1) as f32;
    let mut candidates: Vec<usize> = (0..c.len()).filter(|&i| c[i] > 0.0).collect();
    candidates.sort_by(|&a, &b| c[b].total_cmp(&c[a]));
    candidates.truncate(200);
    let score = |i: usize| {
        let elsewhere: f32 = centroids
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .map(|(_, o)| o[i])
            .sum::<f32>()
            / others;
        c[i] - elsewhere
    };
    candidates.sort_by(|&a, &b| score(b).total_cmp(&score(a)));
    let mut out: Vec<String> = Vec::new();
    for i in candidates {
        if out.len() >= n {
            break;
        }
        let t = &vocab.terms[i];
        let overlaps = out
            .iter()
            .any(|o| o.split(' ').any(|w| t.split(' ').any(|x| x == w)));
        if !overlaps {
            out.push(t.clone());
        }
    }
    out
}

pub struct TopicModel {
    pub labels: Vec<Vec<String>>,
    pub page_topic: Vec<Option<usize>>,
    pub page_score: Vec<f32>,
    pub page_keywords: Vec<Vec<String>>,
    vocab: Vocab,
    centroids: Vec<Vec<f32>>,
}

impl TopicModel {
    fn assign(&self, md: &str) -> Option<(usize, f32)> {
        let v = self.vocab.vectorize(&terms(md));
        self.centroids
            .iter()
            .enumerate()
            .map(|(i, c)| (i, dot(&v, c)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, s)| *s >= 0.08)
    }
}

/// Terms on most pages of one template are that template's boilerplate ("visit website",
/// "read the case study"), not a topic.
fn template_boilerplate(toks: &[Vec<String>], templates: &[Option<i32>]) -> HashSet<String> {
    let mut members: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, t) in templates.iter().enumerate() {
        if let Some(t) = t {
            members.entry(*t).or_default().push(i);
        }
    }
    let mut out = HashSet::new();
    for pages in members.values().filter(|p| p.len() >= 5) {
        let mut df: HashMap<&str, usize> = HashMap::new();
        for &i in pages {
            for t in toks[i].iter().map(String::as_str).collect::<HashSet<_>>() {
                *df.entry(t).or_default() += 1;
            }
        }
        out.extend(
            df.into_iter()
                .filter(|(_, c)| *c as f64 > 0.6 * pages.len() as f64)
                .map(|(t, _)| t.to_string()),
        );
    }
    out
}

pub fn topics(docs: &[String], templates: &[Option<i32>]) -> TopicModel {
    let toks: Vec<Vec<String>> = docs.iter().map(|d| terms(d)).collect();
    let vocab = Vocab::build(&toks, &template_boilerplate(&toks, templates));
    let vecs: Vec<SparseVec> = toks.iter().map(|t| vocab.vectorize(t)).collect();
    let usable: Vec<usize> = (0..vecs.len()).filter(|&i| !vecs[i].is_empty()).collect();
    let n = usable.len();
    let empty = || TopicModel {
        labels: vec![],
        page_topic: vec![None; docs.len()],
        page_score: vec![0.0; docs.len()],
        page_keywords: vec![vec![]; docs.len()],
        vocab: Vocab {
            index: HashMap::new(),
            terms: vec![],
            idf: vec![],
        },
        centroids: vec![],
    };
    if n < 4 || vocab.terms.is_empty() {
        return empty();
    }
    let k = ((n as f64 / 2.0).sqrt().round() as usize)
        .clamp(2, MAX_TOPICS)
        .min(n);
    // Fit topics on content pages; listing pages (mostly links and teasers) would otherwise
    // pull clusters towards headline fragments. They're assigned afterwards.
    let listing_like = |i: usize| docs[i].matches("](").count() * 6 > toks[i].len().max(1) / 2;
    let mut fit: Vec<usize> = usable.iter().copied().filter(|&i| !listing_like(i)).collect();
    if fit.len() < 4.max(usable.len() / 5) {
        fit = usable.clone();
    }
    let k = k.min(fit.len());
    let subset: Vec<SparseVec> = fit.iter().map(|&i| vecs[i].clone()).collect();
    let (assign, centroids) = kmeans(&subset, vocab.terms.len(), k);

    // Renumber topics largest first and drop empty ones.
    let mut sizes = vec![0usize; centroids.len()];
    assign.iter().for_each(|&a| sizes[a] += 1);
    let mut order: Vec<usize> = (0..centroids.len()).filter(|&c| sizes[c] > 0).collect();
    order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let centroids: Vec<Vec<f32>> = order.iter().map(|&c| centroids[c].clone()).collect();
    let labels = (0..centroids.len())
        .map(|k| label(k, &centroids, &vocab, 3))
        .collect();

    let mut page_topic = vec![None; docs.len()];
    let mut page_score = vec![0.0; docs.len()];
    for &i in &usable {
        if let Some((t, score)) = centroids
            .iter()
            .enumerate()
            .map(|(t, c)| (t, dot(&vecs[i], c)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        {
            page_topic[i] = Some(t);
            page_score[i] = score;
        }
    }
    let page_keywords = vecs
        .iter()
        .map(|v| {
            let mut v = v.clone();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v.into_iter()
                .take(6)
                .map(|(i, _)| vocab.terms[i as usize].clone())
                .collect()
        })
        .collect();
    TopicModel {
        labels,
        page_topic,
        page_score,
        page_keywords,
        vocab,
        centroids,
    }
}

// ---------------------------------------------------------------- persistence

const CONTENT_SQL: &str = r#"
DROP TABLE IF EXISTS topics;
DROP TABLE IF EXISTS page_topics;
DROP TABLE IF EXISTS page_sections;
CREATE TABLE topics (topic_id INTEGER, label VARCHAR, terms JSON, pages INTEGER);
CREATE TABLE page_topics (url VARCHAR, topic_id INTEGER, score DOUBLE, keywords JSON);
CREATE TABLE page_sections (
    url VARCHAR, ordinal INTEGER, section_type VARCHAR, section_group VARCHAR, heading VARCHAR,
    words INTEGER, links INTEGER, images INTEGER, cards INTEGER, topic_id INTEGER, class_hint VARCHAR
);
"#;

/// Computes sections and topics for every content page and stores them. Returns the topic count.
pub fn run(conn: &Connection, urls: &[&str], htmls: &[String], templates: &[Option<i32>]) -> Result<usize> {
    let mut md_by_url: HashMap<String, String> = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT url, coalesce(main_markdown, full_markdown, '') FROM page_content")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (u, md) = row?;
            md_by_url.insert(u, md);
        }
    }
    let page_sections = sections_parallel(htmls);
    // A page's topic comes from its own content sections; related-story lists, author boxes,
    // CTAs and logo strips repeat across pages and would blur the topics.
    let docs: Vec<String> = urls
        .iter()
        .zip(&page_sections)
        .map(|(u, secs)| {
            let own: Vec<&str> = secs
                .iter()
                .filter(|s| {
                    matches!(
                        s.kind,
                        "hero" | "article" | "content" | "features" | "faq" | "pricing" | "video" | "media"
                    )
                })
                .map(|s| s.text.as_str())
                .collect();
            let md = md_by_url.remove(*u).unwrap_or_default();
            if own.is_empty() { md } else { own.join("\n") }
        })
        .collect();
    let model = topics(&docs, templates);

    conn.execute_batch(CONTENT_SQL)?;
    {
        let mut app = conn.appender("topics")?;
        for (i, l) in model.labels.iter().enumerate() {
            let pages = model.page_topic.iter().filter(|t| **t == Some(i)).count();
            app.append_row(params![
                i as i32 + 1,
                l.join(" · "),
                serde_json::to_string(l)?,
                pages as i32
            ])?;
        }
    }
    {
        let mut app = conn.appender("page_topics")?;
        for (i, u) in urls.iter().enumerate() {
            app.append_row(params![
                u,
                model.page_topic[i].map(|t| t as i32 + 1),
                model.page_score[i] as f64,
                serde_json::to_string(&model.page_keywords[i])?,
            ])?;
        }
    }
    {
        let mut app = conn.appender("page_sections")?;
        for (u, secs) in urls.iter().zip(&page_sections) {
            for s in secs {
                let topic = (s.words >= SECTION_TOPIC_MIN_WORDS)
                    .then(|| model.assign(&s.text))
                    .flatten()
                    .map(|(t, _)| t as i32 + 1);
                app.append_row(params![
                    u,
                    s.ordinal as i32,
                    s.kind,
                    s.group,
                    s.heading,
                    s.words as i32,
                    s.links as i32,
                    s.images as i32,
                    s.cards as i32,
                    topic,
                    s.class_hint,
                ])?;
            }
        }
    }
    Ok(model.labels.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_landing_page_into_typed_sections() {
        let html = r#"<html><body><nav class="navbar"><a href="/">Home</a></nav><main><div class="page-wrapper">
            <section class="section_hero"><h1>Gym software for BJJ</h1><p>Run your academy.</p><a class="button" href="/demo">Book a demo</a></section>
            <section class="section_features"><h2>Everything you need</h2>
              <div class="feature"><h3>Billing</h3><p>Automatic direct debit.</p></div>
              <div class="feature"><h3>Attendance</h3><p>Check-in from any phone.</p></div>
              <div class="feature"><h3>Belts</h3><p>Track progression easily.</p></div></section>
            <section class="section_testimonial"><blockquote>“MAAT changed how we run the gym.”</blockquote></section>
            <section class="section_faq"><h2>FAQ</h2><h3>Is there a contract?</h3><p>No.</p><h3>Can I import members?</h3><p>Yes.</p><h3>Do you support MMA?</h3><p>Yes.</p></section>
            <section class="section_cta"><h2>Ready?</h2><a class="button" href="/signup">Start free</a></section>
            </div></main><footer class="footer"><a href="/legal">Legal</a></footer></body></html>"#;
        let s = sections(html);
        let kinds: Vec<&str> = s.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, vec!["hero", "features", "testimonials", "faq", "cta"]);
        assert_eq!(s[0].heading.as_deref(), Some("Gym software for BJJ"));
        assert_eq!(s[1].cards, 3);
    }

    #[test]
    fn finds_topics_in_two_obvious_groups() {
        let mut docs = Vec::new();
        for i in 0..6 {
            docs.push(format!(
                "jiu jitsu gym belts sparring academy grappling class {i} black belt promotion"
            ));
            docs.push(format!(
                "injection moulding footwear soles compound polyurethane factory {i} sole production"
            ));
        }
        let m = topics(&docs, &vec![None; docs.len()]);
        assert!(m.labels.len() >= 2);
        let a = m.page_topic[0];
        assert!(m.page_topic.iter().step_by(2).all(|t| *t == a));
        assert!(m.page_topic.iter().skip(1).step_by(2).all(|t| *t != a));
    }

    #[test]
    fn tokens_drop_link_targets_and_stopwords() {
        let t = tokens("Read the [pricing guide](https://acme.com/pricing-guide) for gyms");
        assert_eq!(t, vec!["pricing", "guide", "gyms"]);
    }
}
