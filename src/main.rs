mod analyze;
mod content;
mod crawl;
mod discover;
mod extract;
mod model;
mod query;
mod reextract;
mod report;
mod store;
mod templates;
mod urlnorm;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use url::Url;

const DEFAULT_UA: &str = concat!("SiteCannon/", env!("CARGO_PKG_VERSION"), " (site audit crawler)");

#[derive(Parser)]
#[command(name = "sitecannon", version, about = "Study a website before you rebuild it")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Crawl a site into a project directory (DuckDB database + content)
    Crawl(CrawlArgs),
    /// Compute link graph metrics, click depth, URL tree, templates and duplicates
    Analyze {
        project: PathBuf,
        /// Min layout similarity (0–1) for a page to join a template
        #[arg(long, default_value_t = 0.7)]
        template_similarity: f64,
        /// Max differing bits (of 64) for two pages to count as near-duplicate content
        #[arg(long, default_value_t = 3)]
        duplicate_bits: u32,
    },
    /// Re-run HTML extraction over stored HTML (after upgrading sitecannon), then re-analyse
    Reextract { project: PathBuf },
    /// Write a single-file HTML report (runs `analyze` first if needed)
    Report {
        project: PathBuf,
        /// Output file [default: <project>/report.html]
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Write the report data as JSON instead of HTML
        #[arg(long)]
        json: bool,
    },
    /// Run read-only SQL against a crawled project
    Query {
        /// Project directory (or path to site.duckdb)
        project: PathBuf,
        sql: String,
        /// Emit JSON rows instead of a table
        #[arg(long)]
        json: bool,
        /// Truncate table cells to this many characters
        #[arg(long, default_value_t = 60)]
        width: usize,
    },
    /// Print a summary of a crawled project
    Info { project: PathBuf },
}

#[derive(clap::Args)]
struct CrawlArgs {
    /// Start URL (usually the homepage)
    url: String,
    /// Project directory [default: ./projects/<host>]
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Stop after this many URLs
    #[arg(long, default_value_t = 1000)]
    max_pages: usize,
    /// Maximum link hops from the start URL (or from a sitemap entry)
    #[arg(long, default_value_t = 50)]
    max_depth: u32,
    /// Parallel requests
    #[arg(short, long, default_value_t = 8)]
    concurrency: usize,
    /// Requests per second (politeness cap; robots.txt Crawl-delay wins if slower)
    #[arg(long, default_value_t = 5.0)]
    rps: f64,
    #[arg(long, default_value = DEFAULT_UA)]
    user_agent: String,
    /// Ignore robots.txt. Only with the site owner's permission.
    #[arg(long)]
    ignore_robots: bool,
    /// Skip XML sitemap discovery
    #[arg(long)]
    no_sitemaps: bool,
    /// Max distinct query-string variants crawled per path (guards against faceted-nav explosions)
    #[arg(long, default_value_t = 25)]
    max_query_variants: usize,
    /// Per-request timeout in seconds
    #[arg(long, default_value_t = 20)]
    timeout: u64,
    /// Overwrite an existing crawl in the project directory
    #[arg(long)]
    force: bool,
    /// Don't store raw HTML (smaller database, but templates can't be re-analysed without re-crawling)
    #[arg(long)]
    no_html: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Crawl(a) => run_crawl(a).await,
        Cmd::Analyze {
            project,
            template_similarity,
            duplicate_bits,
        } => {
            let s = analyze::run(
                &project,
                &analyze::Options {
                    template_similarity,
                    duplicate_bits,
                },
            )?;
            println!("Analysed {} pages from {}", s.pages, s.homepage);
            println!(
                "  click depth: max {}, {} unreachable from the homepage",
                s.max_depth, s.unreachable
            );
            println!(
                "  {} orphans (no internal links in), {} dead ends (no links out)",
                s.orphans, s.dead_ends
            );
            println!(
                "  {} templates covering {} pages, {} one-off pages",
                s.templates, s.templated_pages, s.one_off_pages
            );
            println!(
                "  {} near-duplicate content groups, {} topics",
                s.dup_groups, s.topics
            );
            println!("Next: sitecannon info {}", project.display());
            Ok(())
        }
        Cmd::Reextract { project } => {
            let n = reextract::run(&project)?;
            let s = analyze::run(
                &project,
                &analyze::Options {
                    template_similarity: 0.7,
                    duplicate_bits: 3,
                },
            )?;
            println!(
                "Re-extracted {n} pages; {} templates covering {} pages, {} one-off pages",
                s.templates, s.templated_pages, s.one_off_pages
            );
            Ok(())
        }
        Cmd::Report { project, out, json } => {
            let analysed = query::run(
                &project,
                "SELECT 1 FROM information_schema.tables WHERE table_name = 'page_metrics'",
            )
            .is_ok_and(|t| !t.rows.is_empty());
            if !analysed {
                println!("Analysing first…");
                analyze::run(
                    &project,
                    &analyze::Options {
                        template_similarity: 0.7,
                        duplicate_bits: 3,
                    },
                )?;
            }
            let path = report::run(&project, out, json)?;
            println!("Report written to {}", path.display());
            Ok(())
        }
        Cmd::Query {
            project,
            sql,
            json,
            width,
        } => {
            let t = query::run(&project, &sql)?;
            if json {
                query::print_json(&t)
            } else {
                query::print_table(&t, width)
            }
            Ok(())
        }
        Cmd::Info { project } => {
            let analysed = query::run(
                &project,
                "SELECT 1 FROM information_schema.tables WHERE table_name = 'page_metrics'",
            )
            .is_ok_and(|t| !t.rows.is_empty());
            let extra: &[(&str, &str)] = if analysed { query::ANALYSIS_QUERIES } else { &[] };
            for (title, sql) in query::INFO_QUERIES.iter().chain(extra) {
                println!("\n■ {title}");
                match query::run(&project, sql) {
                    Ok(t) => query::print_table(&t, 70),
                    Err(e) => println!("  (failed: {e})"),
                }
            }
            if !analysed {
                println!(
                    "\nRun `sitecannon analyze {}` for structure, templates and link metrics.",
                    project.display()
                );
            }
            Ok(())
        }
    }
}

async fn run_crawl(a: CrawlArgs) -> Result<()> {
    let raw = if a.url.contains("://") {
        a.url.clone()
    } else {
        format!("https://{}", a.url)
    };
    let seed = Url::parse(&raw).with_context(|| format!("invalid URL: {}", a.url))?;
    let host = seed.host_str().context("URL has no host")?.to_string();
    let out = a.out.unwrap_or_else(|| PathBuf::from("projects").join(&host));

    let db = out.join(store::DB_FILE);
    if db.exists() {
        if !a.force {
            bail!("{} already exists; pass --force to replace it", db.display());
        }
        std::fs::remove_file(&db)?;
        let _ = std::fs::remove_file(db.with_extension("duckdb.wal"));
    }

    let robots_agent = a
        .user_agent
        .split(['/', ' '])
        .next()
        .unwrap_or("SiteCannon")
        .to_string();
    let cfg = crawl::CrawlConfig {
        seed,
        max_pages: a.max_pages,
        max_depth: a.max_depth,
        concurrency: a.concurrency.max(1),
        rps: a.rps,
        user_agent: a.user_agent,
        robots_agent,
        respect_robots: !a.ignore_robots,
        use_sitemaps: !a.no_sitemaps,
        max_query_variants: a.max_query_variants,
        timeout: Duration::from_secs(a.timeout),
        keep_html: !a.no_html,
    };
    let stats = crawl::run(cfg, &out).await?;
    println!(
        "Crawled {} URLs ({} in sitemaps) in {:.1}s → {}",
        stats.pages,
        stats.sitemap_urls,
        stats.elapsed.as_secs_f64(),
        out.display()
    );
    println!("Next: sitecannon info {}", out.display());
    Ok(())
}
