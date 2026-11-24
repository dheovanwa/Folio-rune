//! Fetch a URL and turn it into Folio's block model.
//!
//! `dom_smoothie` (Mozilla Readability) picks the article body; `dom_query` walks the retained
//! HTML so headings, quotes, lists, code and links survive as blocks instead of one wall of text.
//!
//! Extraction has four rungs, tried in order, so a page only reaches the next one when the
//! previous could not read it:
//!
//! 1. A site profile's pinned container ([`parser::fallback::from_container`]).
//! 2. A structured `__NEXT_DATA__` payload.
//! 3. Readability's own scoring.
//! 4. The zero-config density heuristic ([`parser::fallback::analyse`]).
//!
//! Whichever rung wins, a profile's cleanup rules are applied to the result before it is handed
//! back, and the plain text is rebuilt so search and word count match what is drawn.

use dom_query::Document;
use dom_smoothie::{Article, Config as ReadConfig, Readability, TextMode};
use serde::{Deserialize, Serialize};

use self::parser::fallback::{self, Found};
use self::parser::profile::Profile;
use self::parser::sanitize::{Sanitizer, plain_of};

/// Extraction and site profiles live in `src/parser/`.
///
/// Declared here rather than at the crate root because the integration tests link this file on
/// its own (`#[path = "../src/article.rs"] mod article;`); a `crate::parser` path would not
/// resolve there, while one module tree serves both the binary and the tests.
#[path = "parser/mod.rs"]
pub mod parser;

/// One block of article prose, already shaped for the reader.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// `level` is 1..=6, as `<h1>`..`<h6>`.
    Heading {
        level: u8,
        text: String,
    },
    Para {
        spans: Vec<Inline>,
    },
    Quote {
        spans: Vec<Inline>,
    },
    /// Ordered or unordered; `start` is the first number for `<ol start=..>`.
    List {
        ordered: bool,
        start: u32,
        items: Vec<Vec<Inline>>,
    },
    Code {
        text: String,
    },
    Rule,
}

/// A run of inline text. Links keep their absolute target so the reader can open them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inline {
    pub text: String,
    pub kind: InlineKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InlineKind {
    Text,
    Bold,
    Italic,
    Code,
    Link(String),
}

impl Inline {
    fn plain(text: impl Into<String>) -> Inline {
        Inline {
            text: text.into(),
            kind: InlineKind::Text,
        }
    }
}

impl Block {
    /// Visit every drawn string in this block, with its index within the block.
    ///
    /// A block can draw more than one string (a list draws one per item), and a search hit is a
    /// byte range into the string that is actually shaped, so this must hand over exactly what
    /// the reader passes to `rich`/`text` — no markers, no numbering, no separators.
    pub fn each_text(&self, mut f: impl FnMut(usize, &str)) {
        match self {
            Block::Heading { text, .. } => f(0, text),
            Block::Para { spans } | Block::Quote { spans } => {
                let joined = spans.iter().map(|s| s.text.as_str()).collect::<String>();
                f(0, &joined);
            }
            Block::List { items, .. } => {
                for (n, item) in items.iter().enumerate() {
                    let joined = item.iter().map(|s| s.text.as_str()).collect::<String>();
                    f(n, &joined);
                }
            }
            Block::Code { text } => f(0, text),
            Block::Rule => {}
        }
    }

    /// Whether this block is drawn as a heading, and at what level.
    pub fn heading(&self) -> Option<(u8, &str)> {
        match self {
            Block::Heading { level, text } => Some((*level, text)),
            _ => None,
        }
    }
}

/// A fetched, extracted article, plus the metadata Folio shows above the body.
#[derive(Clone, Debug)]
pub struct ArticleDoc {
    pub url: String,
    pub title: String,
    pub byline: Option<String>,
    pub site: Option<String>,
    pub excerpt: Option<String>,
    pub blocks: Vec<Block>,
    /// Everything the article says, for search and the empty-state word count.
    pub plain: String,
    /// The next chapter's absolute URL, when the page declares one. The prefetcher queues it so
    /// the next chapter is already cached when the reader gets there.
    pub next_chapter_url: Option<String>,
}

impl ArticleDoc {
    pub fn word_count(&self) -> usize {
        self.plain.split_whitespace().count()
    }

    /// Minutes at a 230 wpm reading speed, rounded up, never zero.
    pub fn reading_minutes(&self) -> usize {
        (self.word_count() as f32 / 230.0).ceil().max(1.0) as usize
    }

    /// The TOC: the article itself, then every heading with the block it names.
    ///
    /// The title leads the list so a chapter with no headings at all — a serialised chapter's
    /// prose is all one style — still has something to navigate to. `None` means "the top".
    pub fn toc(&self) -> Vec<(u8, String, Option<usize>)> {
        let mut out = vec![(1u8, self.title.clone(), None)];
        out.extend(self.blocks.iter().enumerate().filter_map(|(i, b)| match b {
            Block::Heading { level, text } => Some((*level, text.clone(), Some(i))),
            _ => None,
        }));
        out
    }

    /// Every match of `needle` in the article, in reading order. Case-insensitive.
    ///
    /// The needle is lowercased here rather than by the caller: a caller who forgets would get
    /// a silent zero hits, which looks like "no matches" instead of a bug.
    ///
    /// Offsets are UTF-8 bytes into the string the block draws, which is what
    /// [`TextHighlight`](rune::TextHighlight) wants, so a hit can be painted without the
    /// reader reshaping anything.
    pub fn find(&self, needle: &str) -> Vec<Hit> {
        let needle = needle.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            block.each_text(|item, text| {
                let hay = text.to_lowercase();
                let mut from = 0;
                while let Some(at) = hay[from..].find(needle.as_str()) {
                    let start = from + at;
                    hits.push(Hit {
                        block: i,
                        item,
                        start,
                        end: start + needle.len(),
                    });
                    from = start + needle.len().max(1);
                }
            });
        }
        hits
    }
}

/// One search hit: which block, which of its text runs (list items are numbered), and the byte
/// range within that run's drawn text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub block: usize,
    pub item: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub enum FetchError {
    Network(String),
    NotReadable,
    Empty,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(e) => write!(f, "{e}"),
            FetchError::NotReadable => write!(f, "Couldn't find an article on that page."),
            FetchError::Empty => write!(f, "That page had no readable text."),
        }
    }
}

/// Block a background thread does the network I/O on, then hands to the UI.
pub fn fetch(url: &str) -> Result<ArticleDoc, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent(concat!(
            "Folio/",
            env!("CARGO_PKG_VERSION"),
            " (Rune reader)"
        ))
        .build()
        .into();
    let html = agent
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml")
        .call()
        .map_err(|e| FetchError::Network(e.to_string()))?
        .body_mut()
        .read_to_string()
        .map_err(|e| FetchError::Network(e.to_string()))?;
    extract(&html, url)
}

/// Whether two URLs point at the same page, ignoring a trailing slash and the fragment.
fn same_page(a: &str, b: &str) -> bool {
    fn key(u: &str) -> &str {
        u.split('#').next().unwrap_or(u).trim_end_matches('/')
    }
    key(a).eq_ignore_ascii_case(key(b))
}

/// The pure part: HTML in, blocks out. Split from `fetch` so it is testable offline.
///
/// The site profile is looked up from the installed `profiles/` once per call; use
/// [`extract_with`] when the caller already has one.
pub fn extract(html: &str, url: &str) -> Result<ArticleDoc, FetchError> {
    extract_with(html, url, parser::profile_for(url))
}

/// `extract`, with the profile given rather than looked up. `None` is the zero-config path.
pub fn extract_with(
    html: &str,
    url: &str,
    profile: Option<&Profile>,
) -> Result<ArticleDoc, FetchError> {
    // Relative hrefs in the extracted body resolve against the page they came from.
    set_base_url(url);

    // Rung 1: a site profile can pin the container, which beats every guess below it.
    if let Some(p) = profile
        && let Some(found) = fallback::from_container(html, p)
        && let Some(doc) = doc_from(found, url)
    {
        return finish(doc, profile, html, url);
    }

    // Rung 2: some sites ship the article as structured JSON in `__NEXT_DATA__` rather than as
    // prose markup (Next.js + Sanity, common for serialised fiction). Readability only sees
    // navigation on those pages and gives up, so the payload is tried first.
    if let Some(doc) = next_data_doc(html, url) {
        return finish(doc, profile, html, url);
    }

    // Rung 3: Readability. A refusal is not fatal any more: the rung below is the last resort,
    // and a page Readability cannot score may still be readable by density.
    let cfg = ReadConfig {
        text_mode: TextMode::Formatted,
        ..ReadConfig::default()
    };
    let readable = Readability::new(html, Some(url), Some(cfg))
        .ok()
        .and_then(|mut r| r.parse().ok())
        .and_then(|a| {
            let blocks = blocks_from(&a.content);
            if blocks.is_empty() {
                return None;
            }
            Some(article_doc(url, a, blocks))
        });
    if let Some(doc) = readable {
        return finish(doc, profile, html, url);
    }

    // Rung 4: no profile, no payload, and Readability found nothing. Score the page's containers
    // by prose density and read the best one.
    let found = fallback::analyse(html).ok_or(FetchError::NotReadable)?;
    let doc = doc_from(found, url).ok_or(FetchError::Empty)?;
    finish(doc, profile, html, url)
}

/// `ArticleDoc` from a Readability result, with the title falling back to the URL.
fn article_doc(url: &str, article: Article, blocks: Vec<Block>) -> ArticleDoc {
    let title = if article.title.trim().is_empty() {
        url.to_string()
    } else {
        article.title.clone()
    };
    ArticleDoc {
        url: url.to_string(),
        title,
        byline: article.byline.clone(),
        site: article.site_name.clone(),
        excerpt: article.excerpt.clone(),
        blocks,
        plain: article.text_content.to_string(),
        next_chapter_url: None,
    }
}

/// `ArticleDoc` from a heuristic read, or `None` when it found no blocks.
fn doc_from(found: Found, url: &str) -> Option<ArticleDoc> {
    if found.blocks.is_empty() {
        return None;
    }
    let title = found
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| url.to_string());
    let plain = plain_of(&found.blocks);
    if plain.trim().is_empty() {
        return None;
    }
    Some(ArticleDoc {
        url: url.to_string(),
        title,
        byline: found.byline,
        site: found.site,
        excerpt: found.excerpt,
        blocks: found.blocks,
        plain,
        next_chapter_url: None,
    })
}

/// Apply the site's cleanup rules, then rebuild the plain text so search, the word count and the
/// clipboard all describe what is actually on screen.
///
/// The next chapter's link is recorded here too, from the page as fetched: cleanup drops junk
/// blocks, and a site's "next chapter" anchor often lives in exactly that kind of furniture.
fn finish(
    mut doc: ArticleDoc,
    profile: Option<&Profile>,
    html: &str,
    url: &str,
) -> Result<ArticleDoc, FetchError> {
    doc.next_chapter_url =
        parser::next_url(html, url, profile).filter(|next| !same_page(next, url));
    if let Some(p) = profile
        && !p.is_empty_rules()
    {
        Sanitizer::new(p).apply(&mut doc.blocks);
        if doc.blocks.is_empty() {
            return Err(FetchError::Empty);
        }
        doc.plain = plain_of(&doc.blocks);
    }
    Ok(doc)
}

// ── Next.js / Sanity Portable Text ─────────────────────────────────────────────────────────────

/// The `pageProps` shape these sites use for a chapter: `chapter.content` is Portable Text.
#[derive(Deserialize)]
struct NextData {
    props: NextProps,
}

#[derive(Deserialize)]
struct NextProps {
    #[serde(rename = "pageProps")]
    page_props: PageProps,
}

#[derive(Deserialize)]
struct PageProps {
    chapter: Option<NextChapter>,
    #[serde(rename = "volumeTitle")]
    volume_title: Option<String>,
}

#[derive(Deserialize)]
struct NextChapter {
    title: Option<String>,
    #[serde(default)]
    content: Vec<PBlock>,
}

/// One Portable Text block. `_type` is `block` for prose, `image`/`emptySpace`/`readerSection`
/// for things the reader has no use for.
#[derive(Deserialize)]
struct PBlock {
    #[serde(rename = "_type", default)]
    kind: String,
    #[serde(default)]
    children: Vec<PSpan>,
    style: Option<String>,
    #[serde(rename = "markDefs", default)]
    mark_defs: Vec<PMark>,
    #[serde(rename = "listItem")]
    list_item: Option<String>,
    /// A `readerSection`'s label ("Part 1"), which is the only structure a serialised chapter
    /// carries: its prose blocks are all styled `normal`, so without this the TOC is empty.
    label: Option<String>,
}

#[derive(Deserialize)]
struct PSpan {
    #[serde(default)]
    text: String,
    #[serde(default)]
    marks: Vec<String>,
}

#[derive(Deserialize)]
struct PMark {
    #[serde(rename = "_key", default)]
    key: String,
    #[serde(rename = "_type", default)]
    kind: String,
    href: Option<String>,
}

/// Pull the chapter out of a `__NEXT_DATA__` payload, if there is one worth reading.
fn next_data_doc(html: &str, url: &str) -> Option<ArticleDoc> {
    let doc = Document::from(html.to_string());
    let script = doc.select_single("script#__NEXT_DATA__");
    if !script.exists() {
        return None;
    }
    let data: NextData = serde_json::from_str(script.text().as_ref()).ok()?;
    let chapter = data.props.page_props.chapter?;
    let blocks = portable_blocks(&chapter.content);
    if blocks.is_empty() {
        return None;
    }
    let plain = blocks
        .iter()
        .map(|b| match b {
            Block::Heading { text, .. } => text.clone(),
            Block::Para { spans } | Block::Quote { spans } => {
                spans.iter().map(|s| s.text.as_str()).collect()
            }
            Block::List { items, .. } => items
                .iter()
                .map(|i| i.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
            Block::Code { text } => text.clone(),
            Block::Rule => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let title = chapter
        .title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| data.props.page_props.volume_title.clone())
        .unwrap_or_else(|| url.to_string());
    Some(ArticleDoc {
        url: url.to_string(),
        title: collapse(&title),
        byline: None,
        site: None,
        excerpt: None,
        blocks,
        plain,
        next_chapter_url: None,
    })
}

/// Fold Portable Text blocks into Folio's model, joining runs of list items into one list.
fn portable_blocks(content: &[PBlock]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for b in content {
        // A section break is the chapter's only structure: Sanity renders it as a labelled
        // divider, and its label is exactly what a reader wants in a contents list. Promote it
        // to a heading so it both draws and indexes.
        if b.kind == "readerSection" {
            if let Some(label) = b.label.as_deref().map(collapse).filter(|l| !l.is_empty()) {
                out.push(Block::Heading {
                    level: 2,
                    text: label,
                });
            }
            continue;
        }
        if b.kind != "block" {
            continue;
        }
        let spans = portable_spans(b);
        if !has_words(&spans) {
            continue;
        }
        let text = spans.iter().map(|s| s.text.as_str()).collect::<String>();

        if let Some(item) = &b.list_item {
            let ordered = item == "number";
            // Extend the list this item belongs to, or start one.
            if let Some(Block::List {
                ordered: o, items, ..
            }) = out.last_mut()
                && *o == ordered
            {
                items.push(spans);
                continue;
            }
            out.push(Block::List {
                ordered,
                start: 1,
                items: vec![spans],
            });
            continue;
        }

        match b.style.as_deref() {
            Some("h1") | Some("h2") | Some("h3") | Some("h4") | Some("h5") | Some("h6") => {
                let level = b.style.as_deref().unwrap()[1..].parse::<u8>().unwrap_or(2);
                out.push(Block::Heading {
                    level,
                    text: collapse(&text),
                });
            }
            Some("blockquote") => out.push(Block::Quote { spans }),
            _ => out.push(Block::Para { spans }),
        }
    }
    out
}

/// Turn one block's spans into [`Inline`] runs, resolving `markDefs` links to their hrefs.
fn portable_spans(b: &PBlock) -> Vec<Inline> {
    let mut runs = Vec::new();
    for span in &b.children {
        if span.text.is_empty() {
            continue;
        }
        let mut kind = InlineKind::Text;
        for mark in &span.marks {
            match mark.as_str() {
                "strong" => kind = InlineKind::Bold,
                "em" => kind = InlineKind::Italic,
                "code" => kind = InlineKind::Code,
                other => {
                    if let Some(def) = b.mark_defs.iter().find(|m| m.key == other)
                        && def.kind == "link"
                        && let Some(href) = &def.href
                    {
                        kind = InlineKind::Link(href.clone());
                    }
                }
            }
        }
        runs.push(Inline {
            text: span.text.clone(),
            kind,
        });
    }
    merge(runs)
}

/// Walk an HTML fragment into blocks. Public so the profile and heuristic readers share exactly
/// one flattening, and a container's markup becomes the same model Readability's body would.
pub fn blocks_from_html(html: &str) -> Vec<Block> {
    blocks_from(html)
}

/// Walk the article's retained HTML, flattening it into blocks. Inline formatting inside a
/// paragraph is preserved as [`Inline`] runs.
fn blocks_from(content: &str) -> Vec<Block> {
    let doc = Document::from(content.to_string());
    let root = doc.select("body");
    let root = if root.exists() {
        root
    } else {
        doc.select("html")
    };
    let mut out = Vec::new();
    let nodes: Vec<_> = root.nodes().to_vec();
    for node in nodes {
        walk(&node, &mut out);
    }
    out
}

fn walk(node: &dom_query::NodeRef<'_>, out: &mut Vec<Block>) {
    for child in node.element_children() {
        let name = child
            .node_name()
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse::<u8>().unwrap_or(2);
                let text = collapse(&child.text());
                if !text.is_empty() {
                    out.push(Block::Heading { level, text });
                }
            }
            "p" => {
                let spans = inline_of(&child);
                if has_words(&spans) {
                    out.push(Block::Para { spans });
                }
            }
            "blockquote" => {
                // A quote's own paragraphs become one block: Folio draws the quote as a unit.
                let text = collapse(&child.text());
                if !text.is_empty() {
                    out.push(Block::Quote {
                        spans: vec![Inline::plain(text)],
                    });
                }
            }
            "pre" => {
                let text = child.text().to_string();
                if !text.trim().is_empty() {
                    out.push(Block::Code {
                        text: text.trim_end().to_string(),
                    });
                }
            }
            "ul" | "ol" => {
                let ordered = name == "ol";
                let start = child
                    .attr("start")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                let items: Vec<Vec<Inline>> = child
                    .element_children()
                    .into_iter()
                    .filter(|li| {
                        li.node_name()
                            .map(|n| n.eq_ignore_ascii_case("li"))
                            .unwrap_or(false)
                    })
                    .map(|li| inline_of(&li))
                    .filter(|spans| has_words(spans))
                    .collect();
                if !items.is_empty() {
                    out.push(Block::List {
                        ordered,
                        start,
                        items,
                    });
                }
            }
            "hr" => out.push(Block::Rule),
            "div" | "section" | "article" | "main" | "header" | "footer" | "aside" | "figure" => {
                walk(&child, out)
            }
            // A bare inline element at block level (rare, but Readability emits them).
            "span" | "a" | "strong" | "em" | "b" | "i" | "code" => {
                let spans = inline_of(&child);
                if has_words(&spans) {
                    out.push(Block::Para { spans });
                }
            }
            _ => {}
        }
    }
}

fn has_words(spans: &[Inline]) -> bool {
    spans.iter().any(|s| !s.text.trim().is_empty())
}

/// Collect the inline runs of an element, descending through formatting tags.
fn inline_of(node: &dom_query::NodeRef<'_>) -> Vec<Inline> {
    let mut runs = Vec::new();
    collect_inline(node, InlineKind::Text, &mut runs);
    merge(runs)
}

fn collect_inline(node: &dom_query::NodeRef<'_>, kind: InlineKind, runs: &mut Vec<Inline>) {
    for child in node.children() {
        if child.is_text() {
            let text = child.text().to_string();
            if !text.is_empty() {
                runs.push(Inline {
                    text,
                    kind: kind.clone(),
                });
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }
        let name = child
            .node_name()
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        let next = match name.as_str() {
            "strong" | "b" => InlineKind::Bold,
            "em" | "i" => InlineKind::Italic,
            "code" | "kbd" | "samp" => InlineKind::Code,
            "a" => match child.attr("href") {
                Some(href) if !href.trim().is_empty() => InlineKind::Link(absolute(&href)),
                _ => kind.clone(),
            },
            // A <br> is a soft line break; Folio's prose keeps it as a space.
            "br" => {
                runs.push(Inline {
                    text: " ".into(),
                    kind: kind.clone(),
                });
                continue;
            }
            _ => kind.clone(),
        };
        collect_inline(&child, next, runs);
    }
}

/// Neighbouring runs with the same formatting become one, and whitespace collapses the way
/// the browser's normal `white-space` handling would.
fn merge(runs: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for run in runs {
        let text = collapse_keeping_edges(&run.text);
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.kind == run.kind => last.text.push_str(&text),
            _ => out.push(Inline {
                text,
                kind: run.kind,
            }),
        }
    }
    if let Some(first) = out.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = out.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    out.retain(|r| !r.text.is_empty());
    out
}

/// Whitespace to single spaces, edges trimmed.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whitespace to single spaces, keeping one space at either edge so adjacent runs join
/// without glueing words together.
fn collapse_keeping_edges(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    if space && !out.is_empty() {
        out.push(' ');
    }
    if s.starts_with(char::is_whitespace) && !out.starts_with(' ') {
        out.insert(0, ' ');
    }
    out
}

/// Resolve a link against the article's URL, so relative hrefs still work.
fn absolute(href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
        return href.to_string();
    }
    BASE_URL.with(|b| {
        let base = b.borrow();
        match base.as_deref() {
            Some(base) if href.starts_with('/') => {
                // Keep scheme + host from the base, drop its path.
                let trimmed = base.split("://").nth(1).unwrap_or(base);
                let host = trimmed.split('/').next().unwrap_or(trimmed);
                let scheme = base.split("://").next().unwrap_or("https");
                format!("{scheme}://{host}{href}")
            }
            Some(base) => format!("{}/{}", base.trim_end_matches('/'), href),
            None => href.to_string(),
        }
    })
}

thread_local! {
    /// The URL `absolute` resolves against. Set once per extraction on the same thread.
    static BASE_URL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Set the base URL that relative links resolve against.
pub fn set_base_url(url: &str) {
    BASE_URL.with(|b| *b.borrow_mut() = Some(url.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <html><head><title>The Quiet Page</title></head><body>
          <article>
            <h1>The Quiet Page</h1>
            <p>Reading is <strong>slow</strong> on purpose, and <a href="/notes">that is fine</a>. Reading is a long sentence that goes on for a while, so that the readability scorer has enough text to work with, because short documents are discarded by the extractor as navigation.</p>
            <blockquote>Attention is the rarest form of generosity.</blockquote>
            <ul><li>One</li><li>Two</li></ul>
            <pre><code>fn main() {}</code></pre>
            <h2>A Second Part</h2>
            <p>Done. This paragraph is also long enough to matter, because the extractor weighs paragraphs by their length, and a page of one-line paragraphs looks like a menu rather than an article.</p>
          </article>
        </body></html>"#;

    #[test]
    fn headings_paragraphs_lists_and_code_become_blocks() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        assert_eq!(doc.title, "The Quiet Page");
        // Readability lifts the H1 into `title` and drops it from the body, so the body opens
        // with prose; the reader draws the headline from `doc.title`.
        assert!(matches!(doc.blocks[0], Block::Para { .. }));
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Quote { .. })));
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::List { ordered: false, .. }))
        );
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Code { .. })));
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::Heading { level: 2, .. }))
        );
    }

    #[test]
    fn inline_formatting_survives_as_spans() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        let spans = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Para { spans } => Some(spans),
                _ => None,
            })
            .unwrap();
        assert!(
            spans
                .iter()
                .any(|s| s.kind == InlineKind::Bold && s.text == "slow")
        );
        assert!(spans.iter().any(|s| matches!(s.kind, InlineKind::Link(_))));
    }

    #[test]
    fn relative_links_resolve_against_the_article_url() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        let link = doc.blocks.iter().find_map(|b| match b {
            Block::Para { spans } => spans.iter().find_map(|s| match &s.kind {
                InlineKind::Link(href) => Some(href.clone()),
                _ => None,
            }),
            _ => None,
        });
        assert_eq!(link.as_deref(), Some("https://example.com/notes"));
    }

    #[test]
    fn the_toc_leads_with_the_title_then_the_headings() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        let toc = doc.toc();
        // The title leads, so a chapter with no `<h*>` still has an entry, and it points at the
        // top of the article rather than at a block.
        assert_eq!(toc[0], (1, "The Quiet Page".to_string(), None));
        // The H1 became the article's title, so only the H2 remains as a body heading.
        assert_eq!(toc[1].1, "A Second Part");
        assert!(toc[1].2.is_some(), "a heading entry must name its block");
    }

    /// A synthetic `__NEXT_DATA__` chapter page, with no fixture file behind it.
    ///
    /// `paragraphs` prose blocks are generated so the size checks in the tests below are about
    /// the parser rather than about how long the sample happens to be. Section breaks are placed
    /// at the quarter and the halfway point, and the opening paragraph carries a link through
    /// `markDefs`, which is the one part of Portable Text that needs resolving.
    fn next_chapter_page(paragraphs: usize, title: &str) -> String {
        use serde_json::json;

        let opening = "The special exam aboard the ship began before dawn, and nobody had slept.";
        let body = "The corridor was quiet in the way a held breath is quiet, and every door along \
                    it stayed shut while the announcement was read out over the speakers.";
        let mut content = Vec::with_capacity(paragraphs + 2);
        for i in 0..paragraphs {
            if i == paragraphs / 4 {
                content.push(json!({"_type": "readerSection", "label": "Part 1"}));
            }
            if i == paragraphs / 2 {
                content.push(json!({"_type": "readerSection", "label": "Part 2"}));
            }
            let text = if i == 0 {
                format!("{opening} She read the notice twice. {body}")
            } else {
                format!("{body} Paragraph {i}.")
            };
            let mut children = vec![json!({"_type": "span", "text": text, "marks": []})];
            if i == 0 {
                children.push(json!({
                    "_type": "span",
                    "text": "Join the discussion",
                    "marks": ["link1"],
                }));
            }
            content.push(json!({
                "_type": "block",
                "style": "normal",
                "children": children,
                "markDefs": [{"_key": "link1", "_type": "link", "href": "https://example.com/chat"}],
            }));
        }
        let data = json!({
            "props": {"pageProps": {"chapter": {"title": title, "content": content}}}
        })
        .to_string();
        format!(
            r#"<html><head><title>Site</title></head><body>
                 <a rel="next" href="/ch5">Next chapter</a>
                 <script id="__NEXT_DATA__" type="application/json">{data}</script>
               </body></html>"#
        )
    }

    #[test]
    fn a_reader_section_label_becomes_a_heading() {
        // A serialised chapter has no `<h*>` at all: its only structure is `readerSection`
        // breaks, whose labels are what the contents list must show.
        let html = next_chapter_page(12, "Chapter 4");
        let doc = extract(&html, "https://example.com/ch4").expect("should extract");
        let headings: Vec<&str> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(headings.contains(&"Part 1"), "headings were {headings:?}");
        assert!(headings.contains(&"Part 2"), "headings were {headings:?}");
        // And they reach the TOC, after the title.
        let toc: Vec<String> = doc.toc().into_iter().map(|(_, t, _)| t).collect();
        assert!(toc.iter().any(|t| t == "Part 1"), "toc was {toc:?}");
        assert!(toc.len() >= 3, "toc was {toc:?}");
    }

    #[test]
    fn find_returns_every_occurrence_with_byte_offsets() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        let hits = doc.find("reading");
        assert!(hits.len() >= 2, "got {} hits", hits.len());
        // Every offset must land on the needle in the string the reader actually draws, or the
        // highlight would paint the wrong words.
        for hit in &hits {
            let mut drawn = String::new();
            doc.blocks[hit.block].each_text(|item, text| {
                if item == hit.item {
                    drawn = text.to_string();
                }
            });
            assert!(
                drawn[hit.start..hit.end].eq_ignore_ascii_case("reading"),
                "range {:?} of {:?}",
                hit.start..hit.end,
                drawn
            );
        }
    }

    #[test]
    fn find_is_case_insensitive_and_empty_for_no_match() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        assert!(!doc.find("ATTENTION").is_empty());
        assert!(doc.find("zzzznothing").is_empty());
        assert!(doc.find("").is_empty());
    }

    #[test]
    fn each_text_matches_what_the_reader_draws() {
        // A list draws one string per item, and the search offsets index into those strings, so
        // the numbering and bullets must not be part of them.
        let block = Block::List {
            ordered: false,
            start: 1,
            items: vec![
                vec![Inline {
                    text: "One".into(),
                    kind: InlineKind::Text,
                }],
                vec![Inline {
                    text: "Two".into(),
                    kind: InlineKind::Text,
                }],
            ],
        };
        let mut got = Vec::new();
        block.each_text(|i, text| got.push((i, text.to_string())));
        assert_eq!(got, vec![(0, "One".to_string()), (1, "Two".to_string())]);
    }

    #[test]
    fn a_next_data_chapter_extracts_its_prose() {
        let html = next_chapter_page(60, "Chapter 4");
        let doc = extract(&html, "https://example.com/ch4").expect("should extract");
        assert!(doc.blocks.len() > 50, "got {} blocks", doc.blocks.len());
        assert!(doc.title.contains("Chapter 4"), "title was {:?}", doc.title);
        let first = match &doc.blocks[0] {
            Block::Para { spans } => spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            other => panic!("first block was {other:?}"),
        };
        assert!(
            first.contains("special exam aboard the ship"),
            "first para: {first:?}"
        );
        assert!(doc.word_count() > 500, "words {}", doc.word_count());
        // The link in the prose must survive as a Link run.
        let has_link = doc.blocks.iter().any(|b| match b {
            Block::Para { spans } | Block::Quote { spans } => {
                spans.iter().any(|s| matches!(s.kind, InlineKind::Link(_)))
            }
            _ => false,
        });
        assert!(has_link, "no link run found");
    }

    #[test]
    fn the_next_chapters_link_is_recorded() {
        let html = next_chapter_page(6, "Chapter 4");
        let doc = extract(&html, "https://example.com/book/ch4").expect("should extract");
        assert_eq!(
            doc.next_chapter_url.as_deref(),
            Some("https://example.com/ch5"),
            "a rel=next link should resolve against the article's host"
        );
    }

    #[test]
    fn a_page_without_a_next_link_leaves_the_field_empty() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        assert_eq!(doc.next_chapter_url, None);
    }

    #[test]
    fn a_next_link_to_the_same_page_is_not_recorded() {
        let html = PAGE.replace(
            "</body>",
            r#"<a rel="next" href="/posts/quiet#top">Next chapter</a></body>"#,
        );
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(&html, "https://example.com/posts/quiet").unwrap();
        assert_eq!(doc.next_chapter_url, None);
    }

    #[test]
    fn a_page_with_no_profile_falls_back_to_the_density_heuristic() {
        // No `__NEXT_DATA__`, and markup Readability is likely to refuse: a lone div of prose
        // with no article or main element, which is what the heuristic exists for.
        let body = "The harbour was empty at that hour, and the water held the lights of the town \
                    without moving them, as if the whole bay were deciding whether to wake.";
        let html = format!(
            r#"<html><body><div class="entry">
                 <p>{body}</p><p>{body}</p><p>{body}</p><p>{body}</p>
               </div></body></html>"#
        );
        let doc =
            extract_with(&html, "https://nomatch.test/a", None).expect("heuristic should read it");
        assert!(
            doc.plain.contains("harbour was empty"),
            "plain was {:?}",
            doc.plain
        );
        assert!(doc.word_count() > 30);
    }

    #[test]
    fn a_profile_can_pin_a_container_and_clean_its_text() {
        let html = r#"<html><body>
            <article class="post">
              <h2>Cleaned</h2>
              <p>The first paragraph of the article is long enough to read, and it ends with a
                 watermark that the profile should cut out. (Advertisement)</p>
              <p>Advertisement</p>
              <p>The second paragraph continues the piece, at a similar length, so the block
                 walker has prose on both sides of the dropped line.</p>
            </article></body></html>"#;
        let profile = Profile::parse(
            "domain = \"example.com\"\n[extract]\nselector = \"article.post\"\n\
             [sanitize]\ndrop_blocks = [\"Advertisement\"]\nstrip_text = [\"(Advertisement)\"]",
            "test.toml",
        )
        .unwrap();
        let doc = extract_with(html, "https://example.com/x", Some(&profile)).expect("extract");
        // The block that was nothing but the watermark is gone, and the one that merely ended
        // with it lost only that phrase.
        assert!(
            !doc.plain.contains("Advertisement"),
            "plain was {:?}",
            doc.plain
        );
        assert!(
            doc.plain.contains("first paragraph"),
            "plain was {:?}",
            doc.plain
        );
        assert!(
            doc.plain.contains("second paragraph"),
            "plain was {:?}",
            doc.plain
        );
        assert!(
            !doc.blocks.iter().any(|b| matches!(b, Block::Para { spans }
                if spans.iter().map(|s| s.text.as_str()).collect::<String>().trim()
                    == "Advertisement")),
            "the watermark-only paragraph should be dropped"
        );
    }

    #[test]
    fn reading_time_is_at_least_a_minute() {
        set_base_url("https://example.com/posts/quiet");
        let doc = extract(PAGE, "https://example.com/posts/quiet").unwrap();
        assert!(doc.reading_minutes() >= 1);
    }
}
