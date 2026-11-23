//! Reading a page without a profile, and when readability refuses it.
//!
//! Two entry points, both answering "which part of this page is the article?":
//!
//! * [`from_container`] reads the container a profile pins, with its `exclude` subtrees removed.
//! * [`analyse`] scores every plausible container by how much prose it holds and how little of it
//!   is navigation, then reads the winner.
//!
//! This is the zero-config path: no `profiles/` file, no readability guess, just text density.
//! It is the last resort, so it would rather return `None` than hand the reader a page of menus.

use dom_query::Matcher;
use dom_query::{Document, NodeRef};

use super::profile::Profile;
use crate::article::Block;

/// What the heuristic found: the body, and the metadata around it.
#[derive(Clone, Debug)]
pub struct Found {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub site: Option<String>,
    pub excerpt: Option<String>,
    pub blocks: Vec<Block>,
}

/// Elements that never hold prose. Removed before anything is scored, so their words cannot
/// inflate a menu into an article.
const NOISE: &str = "script, style, noscript, template, iframe, object, embed, svg, canvas, form, \
                     nav, header, footer, aside, button, select, input, [aria-hidden=true], \
                     [role=navigation], [role=banner], [role=contentinfo], [role=complementary]";

/// Containers a browser page actually puts articles in.
const CANDIDATES: &[&str] = &[
    "article",
    "main",
    "[role=main]",
    "div",
    "section",
    "td",
    "body",
];

/// The minimum amount of prose a container must hold before it is believed.
const MIN_CHARS: usize = 200;

/// Above this share of links, a container is a directory rather than an article.
const MAX_LINK_DENSITY: f32 = 0.5;

/// Read the container a profile names.
///
/// Returns `None` when the selector matches nothing or the container has no usable text, so the
/// caller can fall through to the heuristic instead of showing an empty page.
pub fn from_container(html: &str, profile: &Profile) -> Option<Found> {
    let matcher = profile.extract_selector()?;
    let doc = Document::from(html.to_string());
    let found = doc.select_matcher(matcher).first();
    if found.is_empty() {
        return None;
    }
    for exclude in profile.exclude() {
        let junk = found.select_matcher(exclude);
        if !junk.is_empty() {
            junk.remove();
        }
    }
    let blocks = crate::article::blocks_from_html(&found.html().to_string());
    if blocks.is_empty() {
        return None;
    }
    let mut read = metadata(&doc);
    read.blocks = blocks;
    Some(read)
}

/// Score the page's containers and read the best one.
///
/// Longest is not best: `<body>` holds the article plus every scrap the page kept, so the
/// winner is the *tightest* container that still holds nearly all of the prose. That is what
/// separates `<main>` (or an `<article>` inside it) from its parent.
pub fn analyse(html: &str) -> Option<Found> {
    let doc = Document::from(html.to_string());
    // `try_select` answers `None` for an empty selection.
    if let Some(noise) = doc.try_select(NOISE) {
        noise.remove();
    }

    let mut best: Option<(f32, NodeRef<'_>)> = None;
    let mut seen: Vec<(NodeRef<'_>, Prose)> = Vec::new();
    for sel in CANDIDATES {
        let Ok(matcher) = Matcher::new(sel) else {
            continue;
        };
        // Bind the selection before iterating: `nodes()` borrows it, so a temporary would be
        // dropped while the slice is still in use.
        let selection = doc.select_matcher(&matcher);
        for node in selection.nodes() {
            let Some(facts) = prose(node) else { continue };
            let score = facts.score();
            if score > best.map_or(0.0, |(s, _)| s) {
                best = Some((score, *node));
            }
            seen.push((*node, facts));
        }
    }

    let (_, richest) = best?;
    let richest_chars = prose(&richest).map_or(0, |f| f.chars);
    if richest_chars < MIN_CHARS {
        return None;
    }

    // The tightest container that keeps at least most of the prose wins; ties go to the earlier,
    // more specific selector, because `article` is a stronger claim than `div`. A container that
    // is mostly links is navigation, so it is never eligible at all.
    let chosen = seen
        .iter()
        .filter(|(_, facts)| {
            facts.chars * 10 >= richest_chars * 9 && facts.link_density <= MAX_LINK_DENSITY
        })
        .min_by_key(|(_, facts)| facts.chars)
        .map(|(node, _)| *node)
        .unwrap_or(richest);

    // Refuse a page whose best container is still mostly links: that is a directory, not an
    // article, and dressing it as prose would be worse than saying so.
    match prose(&chosen) {
        Some(facts) if facts.link_density <= MAX_LINK_DENSITY => {}
        _ => return None,
    }

    let blocks = crate::article::blocks_from_html(&chosen.html().to_string());
    if blocks.is_empty() {
        return None;
    }
    let mut found = metadata(&doc);
    found.blocks = blocks;
    Some(found)
}

/// What a container holds, measured once so the choice between containers is consistent.
#[derive(Clone, Copy, Debug)]
struct Prose {
    /// Non-whitespace characters of text it holds.
    chars: usize,
    /// Characters inside its direct `<a>` children, over `chars`: how much of it is nav.
    link_density: f32,
    /// Direct `<p>` children, a strong sign the container holds prose rather than a menu.
    paragraphs: usize,
    /// Commas and full stops, capped: sentences instead of a list of labels.
    commas: usize,
}

/// Measure a container, or `None` when it is too small to be an article at all.
fn prose(node: &NodeRef<'_>) -> Option<Prose> {
    let text = node.text().to_string();
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    if chars < 80 {
        return None;
    }
    let link_chars: usize = node
        .element_children()
        .iter()
        .filter(|c| c.node_name().is_some_and(|n| n.eq_ignore_ascii_case("a")))
        .map(|a| {
            a.text()
                .to_string()
                .chars()
                .filter(|c| !c.is_whitespace())
                .count()
        })
        .sum();
    Some(Prose {
        chars,
        link_density: link_chars as f32 / chars as f32,
        paragraphs: node
            .element_children()
            .iter()
            .filter(|c| c.node_name().is_some_and(|n| n.eq_ignore_ascii_case("p")))
            .count(),
        commas: (text.matches(',').count() + text.matches('.').count()).min(50),
    })
}

impl Prose {
    /// How much the container reads like prose.
    ///
    /// Length is the base signal, paragraphs and sentence punctuation raise confidence, and link
    /// density lowers it: a block of links is navigation however long it is.
    fn score(&self) -> f32 {
        let mut score = self.chars as f32 * (1.0 - self.link_density.min(1.0));
        score += self.paragraphs as f32 * 40.0;
        score += self.commas.min(50) as f32 * 3.0;
        // A container that is mostly one long paragraph is still readable; one that is mostly
        // links is not, whatever its length says.
        if self.link_density > 0.5 {
            score *= 0.3;
        }
        score
    }
}

/// Title, byline, site and description from the head, with the visible `<h1>` as a fallback.
fn metadata(doc: &Document) -> Found {
    let meta = |names: &[&str]| -> Option<String> {
        for name in names {
            let sel = format!("meta[property=\"{name}\"], meta[name=\"{name}\"]");
            let found = doc.select(&sel);
            if let Some(content) = found.nodes().first().and_then(|n| n.attr("content")) {
                let text = tidy(&content);
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    };

    let h1 = doc
        .select("h1")
        .nodes()
        .first()
        .map(|n| tidy(&n.text().to_string()))
        .filter(|t| !t.is_empty());
    let title = meta(&["og:title", "twitter:title"])
        .or_else(|| {
            let t = doc.select("title");
            t.nodes().first().map(|n| tidy(&n.text().to_string()))
        })
        .filter(|t| !t.is_empty())
        .or(h1);

    Found {
        title,
        byline: meta(&["author", "article:author", "og:article:author"]),
        site: meta(&["og:site_name", "application-name"]),
        excerpt: meta(&["description", "og:description", "twitter:description"]),
        blocks: Vec::new(),
    }
}

/// Whitespace to single spaces, edges trimmed.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page shaped like a real feed page: chrome everywhere, one article in the middle.
    const PAGE: &str = r#"
        <html><head>
          <title>Site Name — A Quiet Article</title>
          <meta property="og:title" content="A Quiet Article">
          <meta name="author" content="Dheo">
          <meta property="og:site_name" content="Example">
          <meta name="description" content="An article about quiet.">
        </head><body>
          <nav><a href="/">Home</a><a href="/about">About</a><a href="/x">Archive</a></nav>
          <header><a href="/login">Sign in</a><a href="/join">Join</a></header>
          <div class="sidebar"><a href="/a">Link one</a><a href="/b">Link two</a></div>
          <main>
            <h1>A Quiet Article</h1>
            <p>Reading is slow on purpose, and that is the point of the whole thing. A page that
               asks for nothing but attention is rare, and worth keeping, because attention is the
               only thing a reader really spends.</p>
            <p>The margins hold the text away from the edges the way a frame holds a picture, and
               in doing so they tell you where to look, and what to ignore.</p>
            <p>Everything else is a distraction wearing the costume of a feature, so it is removed.
               What remains is the article, and the reader, and the space between them.</p>
          </main>
          <footer><a href="/privacy">Privacy</a><a href="/terms">Terms</a></footer>
          <script>var tracking = 1;</script>
        </body></html>"#;

    #[test]
    fn the_article_container_wins_over_the_chrome() {
        let found = analyse(PAGE).expect("should find the article");
        let text = found
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { spans } | Block::Quote { spans } => {
                    Some(spans.iter().map(|s| s.text.as_str()).collect::<String>())
                }
                Block::Heading { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Reading is slow on purpose"), "got {text:?}");
        assert!(!text.contains("Privacy"), "chrome leaked in: {text:?}");
        assert!(!text.contains("Sign in"), "chrome leaked in: {text:?}");
    }

    #[test]
    fn metadata_comes_from_the_head() {
        let found = analyse(PAGE).unwrap();
        assert_eq!(found.title.as_deref(), Some("A Quiet Article"));
        assert_eq!(found.byline.as_deref(), Some("Dheo"));
        assert_eq!(found.site.as_deref(), Some("Example"));
        assert_eq!(found.excerpt.as_deref(), Some("An article about quiet."));
    }

    #[test]
    fn the_tightest_container_wins_over_body() {
        // `body` holds the article plus a sidebar of links; the reader should get the `div` that
        // holds the prose, not the whole page.
        let para = "The harbour was empty at that hour, and the water held the lights of the town \
                    without moving them, as if the whole bay were deciding whether to wake.";
        let html = format!(
            r#"<html><body>
                 <div class="side"><a href="/a">One</a><a href="/b">Two</a><a href="/c">Three</a></div>
                 <div class="entry"><p>{para}</p><p>{para}</p><p>{para}</p><p>{para}</p><p>{para}</p></div>
               </body></html>"#
        );
        let found = analyse(&html).expect("should read the entry");
        let text: String = found
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para { spans } | Block::Quote { spans } => {
                    Some(spans.iter().map(|s| s.text.as_str()).collect::<String>())
                }
                Block::Heading { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("harbour was empty"), "got {text:?}");
        assert!(!text.contains("Three"), "sidebar leaked in: {text:?}");
    }

    #[test]
    fn a_page_of_links_is_refused() {
        let links = (0..40)
            .map(|i| format!("<a href=\"/p{i}\">Post number {i}</a>"))
            .collect::<String>();
        let html = format!("<html><body><div>{links}</div></body></html>");
        assert!(analyse(&html).is_none(), "links must not become an article");
    }

    #[test]
    fn an_empty_page_is_refused() {
        assert!(analyse("<html><body></body></html>").is_none());
    }

    #[test]
    fn a_profile_container_is_read_and_its_excludes_removed() {
        let p = Profile::parse(
            "domain = \"example.com\"\n[extract]\nselector = \"article.post\"\nexclude = [\".ad-slot\"]",
            "t.toml",
        )
        .unwrap();
        let html = r#"<html><body>
            <article class="post">
              <p>The opening paragraph has enough words in it to be worth reading, and it keeps
                 going for a while so that the extractor has something to hold.</p>
              <div class="ad-slot"><p>Buy this thing today, it is very good and very cheap.</p></div>
              <p>The second paragraph carries on with the same subject, at the same length, which
                 is what a real article does.</p>
            </article></body></html>"#;
        let found = from_container(html, &p).expect("the pinned container should be read");
        let text = found
            .blocks
            .iter()
            .map(|b| match b {
                Block::Para { spans } | Block::Quote { spans } => {
                    spans.iter().map(|s| s.text.as_str()).collect::<String>()
                }
                Block::Heading { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("opening paragraph"), "got {text:?}");
        assert!(
            !text.contains("Buy this thing"),
            "exclude was not removed: {text:?}"
        );
    }

    #[test]
    fn a_selector_that_matches_nothing_falls_through() {
        let p = Profile::parse(
            "domain = \"example.com\"\n[extract]\nselector = \"article.post\"",
            "t.toml",
        )
        .unwrap();
        assert!(from_container("<html><body><p>No post here.</p></body></html>", &p).is_none());
    }
}
