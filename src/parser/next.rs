//! Finding the next chapter of a serialised read.
//!
//! The prefetcher wants chapter N+1 and N+2 queued while N is being read, so extraction records
//! one link per article. Nothing here is guessed from a page's layout: only a link the page
//! itself declares is used, either
//!
//! * a profile's `[extract] next` selector, for sites whose label is unusual, or
//! * `rel="next"`, which is the standard way to declare it, or
//! * an anchor whose own text says it is the next chapter ("Next chapter", "Next part", "»").
//!
//! A bare "Next" is deliberately not enough on its own: on a paginated article it means the next
//! page of the same chapter, and prefetching that would queue a duplicate.

use dom_query::{Document, Matcher};

use super::profile::Profile;

/// Labels that name a next chapter, most specific first.
const LABELS: &[&str] = &[
    "next chapter",
    "next part",
    "next section",
    "continue reading",
    "read next",
    "next volume",
    "chapter continue",
    "下一章",
    "次へ",
    "다음 화",
];

/// The single-character labels sites use for the same thing.
const GLYPHS: &[&str] = &["»", "›", "→", ">>", ">"];

/// The next chapter's absolute URL, when the page declares one.
///
/// Relative hrefs are resolved against `url` here rather than by the caller, so a prefetcher
/// always gets something it can fetch.
pub fn next_url(html: &str, url: &str, profile: Option<&Profile>) -> Option<String> {
    if let Some(found) = from_profile(html, profile) {
        return Some(resolve(url, &found));
    }
    if let Some(found) = from_rel(html) {
        return Some(resolve(url, &found));
    }
    from_label(html).map(|href| resolve(url, &href))
}

/// A profile's own selector wins, because it is the only rule written for this site.
fn from_profile(html: &str, profile: Option<&Profile>) -> Option<String> {
    let matcher: &Matcher = profile?.next_selector()?;
    let doc = Document::from(html.to_string());
    let found = doc.select_matcher(matcher).first();
    let href = found.nodes().first()?.attr("href")?;
    usable(&href).map(|h| h.to_string())
}

/// `rel="next"` on a link or an anchor.
fn from_rel(html: &str) -> Option<String> {
    let doc = Document::from(html.to_string());
    for sel in ["link[href]", "a[href]"] {
        // Bind the selection: `nodes()` borrows it for as long as the loop runs.
        let links = doc.select(sel);
        for node in links.nodes() {
            let rel = node.attr("rel").unwrap_or_default().to_ascii_lowercase();
            if rel.split_whitespace().any(|t| t == "next") {
                if let Some(href) = node
                    .attr("href")
                    .and_then(|h| usable(&h).map(str::to_string))
                {
                    return Some(href);
                }
            }
        }
    }
    None
}

/// An anchor whose text says "next chapter".
fn from_label(html: &str) -> Option<String> {
    let doc = Document::from(html.to_string());
    let mut best: Option<(u8, String)> = None;
    let anchors = doc.select("a[href]");
    for node in anchors.nodes() {
        let Some(href) = node
            .attr("href")
            .and_then(|h| usable(&h).map(str::to_string))
        else {
            continue;
        };
        let label = tidy(&node.text().to_string()).to_ascii_lowercase();
        if label.is_empty() || label.chars().count() > 40 {
            continue;
        }
        // A more specific label beats a glyph, whatever the document order.
        if let Some(rank) = rank_label(&label)
            && best.as_ref().is_none_or(|(r, _)| rank > *r)
        {
            best = Some((rank, href));
        }
    }
    best.map(|(_, href)| href)
}

fn rank_label(label: &str) -> Option<u8> {
    if LABELS.iter().any(|l| label.contains(l)) {
        return Some(3);
    }
    if label == "next" || label == "next page" {
        // Ambiguous on its own: only trusted when the page marks it up as the next chapter.
        return None;
    }
    if GLYPHS.contains(&label) {
        return Some(1);
    }
    None
}

/// A href worth following: not empty, not a fragment, not a script or mail target.
fn usable(href: &str) -> Option<&str> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("mailto:") {
        return None;
    }
    Some(href)
}

/// The href resolved against the article's own URL, absolute.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return href.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    // `//host/path` keeps the scheme and replaces the authority.
    if let Some(after) = href.strip_prefix("//") {
        return format!("{scheme}://{after}");
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if href.starts_with('/') {
        return format!("{scheme}://{host}{href}");
    }
    // A relative path resolves against the directory the article lives in. `rest` still carries
    // the host, so the path is what follows it.
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let path = path.get(host.len()..).unwrap_or("");
    let dir = match path.rfind('/') {
        Some(i) => &path[..i + 1],
        None => "/",
    };
    let href = href.trim_start_matches("./");
    format!("{scheme}://{host}{dir}{href}")
}

/// Whitespace to single spaces, edges trimmed.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(anchor: &str) -> String {
        format!("<html><body><article><p>Prose.</p></article>{anchor}</body></html>")
    }

    #[test]
    fn a_rel_next_link_is_used() {
        let html = page(r#"<a rel="next" href="/book/ch5">Later</a>"#);
        assert_eq!(
            next_url(&html, "https://example.com/book/ch4", None).as_deref(),
            Some("https://example.com/book/ch5")
        );
        assert_eq!(
            resolve("https://example.com/book/ch4", "/book/ch5"),
            "https://example.com/book/ch5"
        );
    }

    #[test]
    fn a_labelled_anchor_is_used_and_a_bare_next_is_not() {
        let labelled = page(r#"<a href="/b/ch5">Next Chapter</a>"#);
        assert_eq!(
            next_url(&labelled, "https://example.com/b/ch4", None).as_deref(),
            Some("https://example.com/b/ch5")
        );

        let vague = page(r#"<a href="/b/ch4?page=2">Next</a>"#);
        assert_eq!(next_url(&vague, "https://example.com/b/ch4", None), None);
    }

    #[test]
    fn the_most_specific_label_wins_over_a_glyph() {
        let html = page(
            r#"<a href="/b/ch4?page=2">»</a><a href="/b/ch5">Next part</a><a href="/b/ch3">Previous</a>"#,
        );
        assert_eq!(
            next_url(&html, "https://example.com/b/ch4", None).as_deref(),
            Some("https://example.com/b/ch5")
        );
    }

    #[test]
    fn a_profile_selector_beats_every_guess() {
        let profile = Profile::parse(
            "domain = \"example.com\"\n[extract]\nnext = \"a.ch-next\"",
            "t.toml",
        )
        .unwrap();
        let html =
            page(r#"<a rel="next" href="/wrong">x</a><a class="ch-next" href="/right">onward</a>"#);
        assert_eq!(
            next_url(&html, "https://example.com/b/ch4", Some(&profile)).as_deref(),
            Some("https://example.com/right")
        );
    }

    #[test]
    fn nothing_to_follow_yields_none() {
        let html = page("");
        assert_eq!(next_url(&html, "https://example.com/b/ch4", None), None);
        let scripts = page(
            r##"<a href="javascript:void(0)">Next chapter</a><a href="#top">Next chapter</a>"##,
        );
        assert_eq!(next_url(&scripts, "https://example.com/b/ch4", None), None);
    }

    #[test]
    fn an_absolute_href_is_left_alone() {
        let html = page(r#"<a rel="next" href="https://other.test/ch5">Later</a>"#);
        assert_eq!(
            next_url(&html, "https://example.com/b/ch4", None).as_deref(),
            Some("https://other.test/ch5")
        );
    }

    #[test]
    fn hrefs_resolve_the_way_a_browser_would() {
        assert_eq!(resolve("https://x.test/a/b/c", "d"), "https://x.test/a/b/d");
        assert_eq!(resolve("https://x.test/a/b/", "d"), "https://x.test/a/b/d");
        assert_eq!(
            resolve("https://x.test/a/b/c", "./d"),
            "https://x.test/a/b/d"
        );
        assert_eq!(resolve("https://x.test/a/b/c", "/d"), "https://x.test/d");
        assert_eq!(
            resolve("https://x.test/a", "//y.test/z"),
            "https://y.test/z"
        );
        assert_eq!(
            resolve("https://x.test/a/b", "https://y.test/z"),
            "https://y.test/z"
        );
        assert_eq!(
            resolve("https://x.test/a/b?x=1#y", "c"),
            "https://x.test/a/c"
        );
        assert_eq!(resolve("https://x.test", "c"), "https://x.test/c");
        assert_eq!(resolve("example.com/a/b", "c"), "https://example.com/a/c");
    }
}
