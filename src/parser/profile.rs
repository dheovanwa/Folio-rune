//! Reading `profiles/*.toml` into compiled [`Profile`]s, and saying what is wrong when a file is
//! not usable.
//!
//! Validation is strict about meaning and lenient about consequences: a profile with a bad
//! selector or a malformed pattern is reported and skipped, and the rest still load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dom_query::Matcher;
use serde::Deserialize;

use super::pattern::Pattern;

/// The file a problem came from, and the sentence that explains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileError {
    pub source: String,
    pub message: String,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source, self.message)
    }
}

/// A validated profile: hosts it answers to, the container to read, and the cleanup rules.
#[derive(Clone, Debug)]
pub struct Profile {
    domain: String,
    name: Option<String>,
    /// Every host it answers to, lowercased. An entry starting with `*.` also matches the bare
    /// host and any subdomain.
    hosts: Vec<String>,
    extract: Option<Matcher>,
    exclude: Vec<Matcher>,
    next: Option<Matcher>,
    drop_blocks: Vec<Pattern>,
    strip_text: Vec<(Pattern, String)>,
}

impl Profile {
    /// The host as written in the file, for messages and tests.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether `host` (no scheme, no path) is one this profile was written for.
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        self.hosts.iter().any(|h| {
            match h.strip_prefix("*.") {
                // `*.example.com` covers example.com and every subdomain of it.
                Some(rest) => host == rest || host.ends_with(&format!(".{rest}")),
                None => host == *h,
            }
        })
    }

    /// The CSS selector naming the article container, when the profile pins one.
    pub fn extract_selector(&self) -> Option<&Matcher> {
        self.extract.as_ref()
    }

    /// Selectors removed from the container before its text is read.
    pub fn exclude(&self) -> &[Matcher] {
        &self.exclude
    }

    /// The selector for the next chapter's link, when the site needs one named.
    pub fn next_selector(&self) -> Option<&Matcher> {
        self.next.as_ref()
    }

    /// Patterns that drop a whole block when the block's text matches.
    pub fn drop_blocks(&self) -> &[Pattern] {
        &self.drop_blocks
    }

    /// Patterns paired with the literal to cut out of a block's text.
    pub fn strip_text(&self) -> &[(Pattern, String)] {
        &self.strip_text
    }

    /// Whether this profile has any cleanup work to do.
    pub fn is_empty_rules(&self) -> bool {
        self.drop_blocks.is_empty() && self.strip_text.is_empty()
    }
}

/// Every profile that loaded, plus every file that did not.
#[derive(Clone, Debug, Default)]
pub struct ProfileSet {
    profiles: Vec<Profile>,
    errors: Vec<ProfileError>,
}

impl ProfileSet {
    /// Read every `*.toml` in `dir`. A missing directory is not an error: profiles are optional.
    pub fn load_dir(dir: &Path) -> ProfileSet {
        let mut set = ProfileSet::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return set;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
            })
            .collect();
        // Directory order is arbitrary; sorting keeps error messages and matches stable.
        files.sort();
        for path in files {
            let source = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            match std::fs::read_to_string(&path) {
                Ok(text) => match Profile::parse(&text, &source) {
                    Ok(profile) => set.profiles.push(profile),
                    Err(e) => set.errors.push(e),
                },
                Err(e) => set.errors.push(ProfileError {
                    source,
                    message: format!("could not be read: {e}"),
                }),
            }
        }
        // A host claimed twice is a configuration mistake, not a crash: keep the first file and
        // say which ones clash.
        let clashes = duplicate_hosts(&set.profiles);
        if let Some((host, owners)) = clashes.into_iter().next() {
            set.errors.push(ProfileError {
                source: "profiles/".to_string(),
                message: format!(
                    "{host} is claimed by more than one profile: {}",
                    owners.join(", ")
                ),
            });
        }
        set
    }

    /// Build a set from profile sources, for tests and for callers with no directory.
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> ProfileSet {
        let mut set = ProfileSet::default();
        for (source, text) in sources {
            match Profile::parse(text, source) {
                Ok(p) => set.profiles.push(p),
                Err(e) => set.errors.push(e),
            }
        }
        set
    }

    /// The first profile whose hosts include `url`'s host.
    pub fn find(&self, url: &str) -> Option<&Profile> {
        let host = host_of(url)?;
        self.profiles.iter().find(|p| p.matches_host(&host))
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// Every problem found while loading, in file order.
    pub fn errors(&self) -> &[ProfileError] {
        &self.errors
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// One sentence for a selector that did not compile.
///
/// `cssparser`'s error does not implement `Display` for every selector kind, so it is printed
/// with `Debug`; what it prints is the parse error plus the offending token, which is what the
/// reader needs to fix the file.
fn selector_error(field: &str, selector: &str, err: &impl std::fmt::Debug) -> String {
    format!("{field} {selector:?} is not a CSS selector: {err:?}")
}

/// The lowercased host of a URL, without userinfo, port or path.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    // Drop any `user:pass@` prefix and the port, keeping a bracketed IPv6 host intact.
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(end) = host.find(']') {
        &host[..=end]
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

/// The on-disk shape of a profile file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    domain: String,
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    extract: Extract,
    #[serde(default)]
    sanitize: Sanitize,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Extract {
    selector: Option<String>,
    #[serde(default)]
    exclude: Vec<String>,
    /// A selector for the "next chapter" link, when the page does not label it usefully.
    next: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Sanitize {
    #[serde(default)]
    drop_blocks: Vec<String>,
    #[serde(default)]
    strip_text: Vec<String>,
}

impl Profile {
    /// Parse and validate one profile file.
    pub fn parse(text: &str, source: &str) -> Result<Profile, ProfileError> {
        let fail = |message: String| ProfileError {
            source: source.to_string(),
            message,
        };
        let file: File = toml::from_str(text).map_err(|e| {
            // `toml`'s message already names the line and column.
            fail(format!("invalid TOML: {}", e.message()))
        })?;

        let domain = file.domain.trim().to_ascii_lowercase();
        if domain.is_empty() {
            return Err(fail("domain must not be empty".to_string()));
        }
        if domain.contains('/') || domain.contains(' ') {
            return Err(fail(format!("domain {domain:?} is not a bare host")));
        }

        let mut hosts = vec![domain.clone()];
        for alias in &file.aliases {
            let alias = alias.trim().to_ascii_lowercase();
            if alias.is_empty() {
                return Err(fail("aliases must not contain empty entries".to_string()));
            }
            if alias.contains('/') || alias.contains(' ') {
                return Err(fail(format!("alias {alias:?} is not a bare host")));
            }
            hosts.push(alias);
        }
        // `dedup` only folds neighbours, so the list is ordered first; matching does not care
        // about the order.
        hosts.sort();
        hosts.dedup();

        let extract = match file.extract.selector.as_deref().map(str::trim) {
            Some(sel) if !sel.is_empty() => Some(
                Matcher::new(sel).map_err(|e| fail(selector_error("extract.selector", sel, &e)))?,
            ),
            _ => None,
        };

        let mut exclude = Vec::with_capacity(file.extract.exclude.len());
        for sel in &file.extract.exclude {
            let sel = sel.trim();
            if sel.is_empty() {
                return Err(fail(
                    "extract.exclude must not contain empty selectors".to_string(),
                ));
            }
            exclude.push(
                Matcher::new(sel).map_err(|e| fail(selector_error("extract.exclude", sel, &e)))?,
            );
        }

        let next = match file.extract.next.as_deref().map(str::trim) {
            Some(sel) if !sel.is_empty() => {
                Some(Matcher::new(sel).map_err(|e| fail(selector_error("extract.next", sel, &e)))?)
            }
            _ => None,
        };

        let mut drop_blocks = Vec::with_capacity(file.sanitize.drop_blocks.len());
        for pat in &file.sanitize.drop_blocks {
            drop_blocks.push(Pattern::new(pat).map_err(|e| fail(format!("drop_blocks: {e}")))?);
        }

        let mut strip_text = Vec::with_capacity(file.sanitize.strip_text.len());
        for pat in &file.sanitize.strip_text {
            let compiled = Pattern::new(pat).map_err(|e| fail(format!("strip_text: {e}")))?;
            // A strip rule edits text inside a block, so it has to name a plain string; a
            // wildcard cannot say what to remove.
            let Some(literal) = compiled.literal().map(str::to_string) else {
                return Err(fail(format!(
                    "strip_text: {pat:?} has a wildcard, but a strip rule must be literal text"
                )));
            };
            strip_text.push((compiled, literal));
        }

        // Two profiles claiming one host would make lookup depend on file order.
        Ok(Profile {
            domain,
            name: file
                .name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            hosts,
            extract,
            exclude,
            next,
            drop_blocks,
            strip_text,
        })
    }
}

/// Where profiles live: next to the executable's project root, or beside the settings file.
///
/// Folio looks for `profiles/` relative to the current directory first (how it is run during
/// development), then under the user's config directory, so a packaged install still finds them.
pub fn profiles_dir() -> Option<PathBuf> {
    let local = PathBuf::from("profiles");
    if local.is_dir() {
        return Some(local);
    }
    let cfg = dirs::config_dir()?.join("folio").join("profiles");
    cfg.is_dir().then_some(cfg)
}

/// Every host claimed by more than one profile, with the profiles that claim it.
///
/// Two profiles for one host would make the match depend on file order, so the first file wins
/// and the clash is reported as a load error.
fn duplicate_hosts(profiles: &[Profile]) -> Vec<(String, Vec<String>)> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for profile in profiles {
        for host in &profile.hosts {
            seen.entry(host.clone())
                .or_default()
                .push(profile.name().unwrap_or(profile.domain()).to_string());
        }
    }
    seen.into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        domain = "example.com"
        name = "Example"
        aliases = ["www.example.com", "*.mirror.example.com"]

        [extract]
        selector = "article.post"
        exclude = [".ad-slot", "nav"]

        [sanitize]
        drop_blocks = ["Advertisement", "Sponsored *"]
        strip_text = ["(Advertisement)", "© 2026 Example"]
    "#;

    #[test]
    fn a_full_profile_compiles() {
        let p = Profile::parse(EXAMPLE, "example.toml").unwrap();
        assert_eq!(p.domain(), "example.com");
        assert_eq!(p.name(), Some("Example"));
        assert!(p.extract_selector().is_some());
        assert_eq!(p.exclude().len(), 2);
        assert_eq!(p.drop_blocks().len(), 2);
        assert_eq!(p.strip_text().len(), 2);
        assert_eq!(p.strip_text()[1].1, "© 2026 Example");
    }

    #[test]
    fn hosts_match_aliases_and_subdomains() {
        let p = Profile::parse(EXAMPLE, "example.toml").unwrap();
        assert!(p.matches_host("example.com"));
        assert!(p.matches_host("EXAMPLE.COM"));
        assert!(p.matches_host("www.example.com"));
        assert!(p.matches_host("mirror.example.com"));
        assert!(p.matches_host("a.b.mirror.example.com"));
        assert!(!p.matches_host("notexample.com"));
        assert!(!p.matches_host("example.com.evil.net"));
    }

    #[test]
    fn a_minimal_profile_needs_only_a_domain() {
        let p = Profile::parse("domain = \"x.test\"", "x.toml").unwrap();
        assert!(p.extract_selector().is_none());
        assert!(p.is_empty_rules());
    }

    #[test]
    fn bad_input_names_the_file_and_the_problem() {
        let err = Profile::parse("name = \"no domain\"", "bad.toml").unwrap_err();
        assert_eq!(err.source, "bad.toml");
        assert!(err.message.contains("domain"), "was {:?}", err.message);

        let err = Profile::parse("domain = \"x.test\"\ndomain2 = 1", "bad.toml").unwrap_err();
        assert!(err.message.contains("invalid TOML") || err.message.contains("unknown field"));

        let err = Profile::parse(
            "domain = \"x.test\"\n[extract]\nselector = \"p:::\"",
            "bad.toml",
        )
        .unwrap_err();
        assert!(
            err.message.contains("not a CSS selector"),
            "was {:?}",
            err.message
        );

        let err = Profile::parse(
            "domain = \"x.test\"\n[sanitize]\ndrop_blocks = [\"\"]",
            "bad.toml",
        )
        .unwrap_err();
        assert!(err.message.contains("drop_blocks"), "was {:?}", err.message);

        let err = Profile::parse(
            "domain = \"x.test\"\n[sanitize]\nstrip_text = [\"Read more *\"]",
            "bad.toml",
        )
        .unwrap_err();
        assert!(
            err.message.contains("must be literal"),
            "was {:?}",
            err.message
        );
    }

    #[test]
    fn one_broken_file_does_not_stop_the_others() {
        let set = ProfileSet::from_sources([
            ("good.toml", "domain = \"good.test\""),
            ("bad.toml", "domain = "),
            ("also.toml", "domain = \"also.test\""),
        ]);
        assert_eq!(set.profiles().len(), 2);
        assert_eq!(set.errors().len(), 1);
        assert_eq!(set.errors()[0].source, "bad.toml");
        assert!(set.find("https://also.test/a").is_some());
        assert!(set.find("https://missing.test/a").is_none());
    }

    #[test]
    fn urls_resolve_to_their_host() {
        assert_eq!(
            host_of("https://Example.COM:8443/a/b").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("https://user:pw@news.example.com/x").as_deref(),
            Some("news.example.com")
        );
        assert_eq!(host_of("http://[::1]:8080/").as_deref(), Some("[::1]"));
        assert_eq!(
            host_of("https://example.com./x").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("example.com/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("   "), None);
    }

    #[test]
    fn a_host_claimed_twice_is_reported() {
        let profiles = [
            Profile::parse("domain = \"same.test\"\nname = \"A\"", "a.toml").unwrap(),
            Profile::parse(
                "domain = \"other.test\"\naliases = [\"same.test\"]",
                "b.toml",
            )
            .unwrap(),
        ];
        let dups = duplicate_hosts(&profiles);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].0, "same.test");
        assert_eq!(dups[0].1, vec!["A".to_string(), "other.test".to_string()]);
    }

    #[test]
    fn the_shipped_profiles_all_load() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("profiles");
        if !dir.is_dir() {
            return;
        }
        let set = ProfileSet::load_dir(&dir);
        assert!(
            set.errors().is_empty(),
            "shipped profiles must be valid: {:?}",
            set.errors()
        );
        assert!(
            !set.is_empty(),
            "profiles/ should hold at least one example"
        );
    }
}
