// Shape of the JSON the Rust CLI embeds (see src/report.rs).

export type Severity = "critical" | "serious" | "warning" | "info";
export type Cell = string | number | boolean | null | Cell[] | { [k: string]: Cell };

export interface Pages {
  url: string[];
  title: (string | null)[];
  status: number[];
  indexability: string[];
  template: (number | null)[];
  topic: (number | null)[];
  depth: (number | null)[];
  /** Index of the click-tree parent page, -1 for the homepage or unreachable pages. */
  parent: number[];
  /** Internal PageRank scaled so the site average is 1.0. */
  pagerank: number[];
  inlinks: number[];
  outlinks: number[];
  words: (number | null)[];
  main_words: (number | null)[];
  in_sitemap: boolean[];
}

export interface Template {
  id: number;
  pages: number;
  pattern: string;
  /** Page index of the most-linked page using this template. */
  example: number;
}

export interface NavItem {
  region: "header" | "footer";
  group: string | null;
  anchor: string;
  url: string;
  /** Page index, -1 when the target isn't a crawled 200 page. */
  page: number;
  coverage: number;
}

export interface Topic {
  id: number;
  label: string;
  terms: string[];
  pages: number;
}

/** One entry per page section, columnar. `page` indexes into `pages`. */
export interface Sections {
  page: number[];
  kind: string[];
  group: string[];
  heading: (string | null)[];
  words: number[];
  topic: (number | null)[];
}

export interface PerfGroup {
  key: number | string | null;
  label: string;
  pages: number;
  pages_with_sessions: number;
  sessions: number | null;
  sessions_per_page: number | null;
  engagement_rate: number | null;
  key_events_per_session: number | null;
  key_events: number | null;
  search_clicks: number | null;
  search_ctr: number | null;
}

export interface Performance {
  sources: { source: string; detail: string | null; date_start: string | null; date_end: string | null; method: string }[];
  totals: Record<string, number | null> | null;
  matched: Cell;
  by_template: PerfGroup[];
  by_topic: PerfGroup[];
  by_depth: PerfGroup[];
  by_section: {
    section_type: string; pages_with: number; pages_without: number;
    engagement_with: number | null; engagement_without: number | null;
    key_events_per_session_with: number | null; key_events_per_session_without: number | null;
  }[];
  opportunities: Record<string, { description: string; count: number; rows: Record<string, Cell>[] }>;
  caveats: string[];
}

/** Per-page usage, aligned with `pages`. */
export interface Usage {
  sessions: (number | null)[];
  engagement_rate: (number | null)[];
  key_events: (number | null)[];
  search_clicks: (number | null)[];
  search_impressions: (number | null)[];
  search_position: (number | null)[];
}

export interface Issue {
  id: string;
  title: string;
  severity: Severity;
  description: string;
  total: number;
  columns: string[];
  rows: Cell[][];
}

export interface ReportData {
  generated_at: string;
  site: {
    seed: string;
    started_at: string;
    robots_status: number | null;
    crawled: number;
    max_pages: number;
    capped: boolean;
    sitemap_urls: number;
    platform: string[] | null;
    hosting: string | null;
    stop_reason: string | null;
    /** URLs answered with 401/403/429. */
    refused: number;
  };
  pages: Pages;
  /** Flat triples [from, to, kind] as page indices; kind 2 = editorial, 1 = widget/sidebar, 0 = menu/footer. */
  edges: number[];
  url_tree: { parent: number[]; segment: string[]; path: string[]; page: number[]; subtree: number[] };
  nav: NavItem[];
  templates: Template[];
  topics: Topic[];
  sections: Sections;
  issues: Issue[];
  /** Strategy brief in markdown, written by an analyst or agent (`report --notes`). */
  notes?: string | null;
  performance?: Performance | null;
  usage?: Usage | null;
}
