//! Folio — a distraction-free article reader, rebuilt natively on Rune.
//!
//! The reading surface is the app: serif prose at a fixed measure, a hairline progress bar, a
//! TOC that slides in over the page, and a settings drawer. Keys are Folio's own.
//!
//! `cargo run` opens the window; `cargo run -- --snap reading.png --scene reading --tree t.json`
//! renders headlessly.

#![forbid(unsafe_code)]

mod article;
mod cache;
mod config;
mod fetcher;
mod library;
mod reader;
mod theme;
mod ui;

use rune::tok::motion;
use rune::{AppSpec, Cx, El, Key, KeyEvent, Live, Tx, col, el};

use crate::article::ArticleDoc;
use crate::config::Config;
use crate::library::Library;
use crate::reader::Reader;
use crate::ui::{drawer, empty, progress, tester, titlebar, toc};

/// Which surface the window shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Nothing open: the paste-a-link page.
    Blank,
    /// The reading surface.
    Reading,
    /// The saved list.
    Library,
}

pub struct State {
    pub cfg: Config,
    pub library: Library,
    pub view: View,
    pub doc: Option<ArticleDoc>,
    /// The address field. Rune binds `text_input` to a `TextEdit`, so it holds the typed text;
    /// `address_focus` tracks whether the field has the keyboard.
    pub edit: rune::TextEdit,
    pub address_focus: bool,
    pub show_toc: bool,
    pub show_settings: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// Reading progress 0..1, updated from the viewport's scroll offset.
    pub progress: Live<f32>,
    /// The scroll offset in points, so keys can move by a known amount.
    pub offset: Live<f32>,
    /// The progress bar's fill rectangle, in the bar's own points.
    pub bar_rect: Live<[f32; 4]>,
    /// The bar's measured width, reported by its layout pass.
    pub track_w: Live<f32>,
    /// The scroll bar's geometry, fed by the same scroll report as `progress`.
    pub scroll_bar: crate::ui::scrollbar::Bar,
    /// Which TOC heading is current, by block index.
    pub active_heading: usize,
    /// Pending fetch: the UI thread hands a URL to a worker and polls this.
    pub pending: Option<String>,
    /// A TOC entry waiting to be scrolled to. `None` means "nothing pending"; `Some(None)` is
    /// the top of the article, `Some(Some(i))` is block `i`.
    pub scroll_to: Option<Option<usize>>,
    /// The find bar's query. Bound to a `TextEdit`, so it holds the typed text.
    pub find_edit: rune::TextEdit,
    /// Whether the find bar has the keyboard.
    pub find_focus: bool,
    /// Every hit for the current query, recomputed when the query changes.
    pub hits: Vec<article::Hit>,
    /// Which hit is the current one, `0`-based into `hits`.
    pub hit_at: usize,
    /// The query the hits were computed from, so a redraw can tell whether to recompute.
    pub hit_query: String,
    /// Set when the current hit changes and cleared once it has been scrolled to. Revealing is a
    /// one-shot request: asking again every frame would fight the reader's own scrolling.
    pub reveal_hit: bool,
    /// The reading surface's own model: where the reader stopped, the auto-hiding chrome, and
    /// the selector tester.
    pub reader: Reader,
    /// A block to reveal once the next commit has built its row, from the saved position.
    pub restore_block: Option<usize>,
}

impl State {
    fn new(cfg: Config, library: Library) -> State {
        let view = match cfg.startup {
            config::Startup::RecentPage if library.entries.is_empty() => View::Blank,
            config::Startup::RecentPage => View::Library,
            config::Startup::BlankPage => View::Blank,
        };
        State {
            cfg,
            library,
            view,
            doc: None,
            edit: rune::TextEdit::new(""),
            address_focus: false,
            show_toc: false,
            show_settings: false,
            loading: false,
            error: None,
            progress: Live::new(0.0),
            offset: Live::new(0.0),
            bar_rect: Live::new([0.0; 4]),
            track_w: Live::new(0.0),
            scroll_bar: Default::default(),
            active_heading: 0,
            pending: None,
            scroll_to: None,
            find_edit: rune::TextEdit::new(""),
            find_focus: false,
            hits: Vec::new(),
            hit_at: 0,
            hit_query: String::new(),
            reveal_hit: false,
            reader: Reader::new(),
            restore_block: None,
        }
    }

    pub fn palette(&self) -> theme::Palette {
        self.cfg.preset().palette()
    }

    /// The prose style: serif at the configured size, on the configured leading.
    pub fn body_style(&self) -> rune::TextStyle {
        rune::TextStyle::new(self.cfg.font_size, 400.0)
            .family(rune::Family::Named(leak(&self.cfg.font_family)))
            .lh(self.cfg.line_px())
            .tracking(self.cfg.letter_spacing)
    }

    pub fn ui_style(&self, weight: f32) -> rune::TextStyle {
        rune::TextStyle::new(self.cfg.ui_font_size, weight)
    }

    pub fn toc_style(&self, weight: f32) -> rune::TextStyle {
        rune::TextStyle::new(self.cfg.toc_font_size, weight)
    }

    fn open(&mut self, doc: ArticleDoc) {
        // Leaving one article for another still counts as leaving: its place is worth keeping.
        self.save_place();
        // The address field mirrors the open article, so pressing it to edit starts from the URL
        // that is actually being read rather than from stale text.
        self.edit = rune::TextEdit::new(doc.url.clone());
        self.restore_block = self.reader.opened(&doc);
        self.doc = Some(doc);
        self.view = View::Reading;
        self.loading = false;
        self.error = None;
        self.address_focus = false;
        self.progress.set(0.0);
        self.offset.set(0.0);
        self.active_heading = 0;
        // A new article starts with a clean find bar and no stale hits from the last one.
        self.find_edit = rune::TextEdit::new("");
        self.find_focus = false;
        self.hits.clear();
        self.hit_at = 0;
        self.hit_query.clear();
        self.reveal_hit = false;
    }

    /// Recompute the hit list from the find bar's current query.
    ///
    /// Cheap enough to run on every keystroke (the article is a few hundred blocks), and doing
    /// it here keeps the count and the highlights in step with the field.
    pub fn refresh_hits(&mut self) {
        let query = self.find_edit.text.clone();
        if query == self.hit_query {
            return;
        }
        self.hit_query = query.clone();
        self.hits = match &self.doc {
            Some(doc) if !query.is_empty() => doc.find(&query),
            _ => Vec::new(),
        };
        self.hit_at = 0;
        // The first hit of a new query is the one to show, so the field never leaves the reader
        // looking at text they did not just search for.
        self.reveal_hit = !self.hits.is_empty();
    }

    /// The byte range the current hit covers, if any.
    pub fn current_hit(&self) -> Option<article::Hit> {
        self.hits.get(self.hit_at).copied()
    }

    /// Move to the next (`+1`) or previous (`-1`) hit, wrapping at the ends.
    pub fn step_hit(&mut self, by: i32) {
        if self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as i32;
        self.hit_at = ((self.hit_at as i32 + by).rem_euclid(n)) as usize;
        self.reveal_hit = true;
    }

    /// Scroll to and centre the current search hit, mounting the virtual row if needed.
    pub fn reveal_current_hit(&mut self, tx: &mut Tx) {
        if let Some(hit) = self.current_hit() {
            let key = if hit.block < self.reader.first {
                format!("headline/hit-{}-{}-{}", hit.block, hit.item, hit.start)
            } else {
                format!("block-{}/hit-{}-{}-{}", hit.block, hit.block, hit.item, hit.start)
            };
            tx.scroll_center(key);
            if hit.block >= self.reader.first {
                tx.scroll_into_view(format!("block-{}", hit.block));
            } else {
                tx.scroll_into_view("article-head");
            }
        }
    }

    /// Move to next/prev hit and immediately scroll to it.
    pub fn step_hit_tx(&mut self, by: i32, tx: &mut Tx) {
        self.step_hit(by);
        self.reveal_current_hit(tx);
    }

    /// A panel that needs the titlebar opened it, or closed it.
    pub fn chrome_panel(&mut self, open: bool) {
        self.reader.chrome.panel(open);
    }

    /// The pin chip: hold the bar open, or let it go.
    pub fn chrome_pin(&mut self) {
        let pinned = self.reader.chrome.is_pinned();
        self.reader.chrome.pin(!pinned);
    }

    /// Write where the reader is into the saved positions, if an article is open.
    ///
    /// The block comes from the rows the list has built and how far the viewport has gone
    /// through them: exact would need layout, and layout from a handler costs a commit.
    pub fn save_place(&mut self) {
        let url = match self.doc.as_ref() {
            Some(d) => d.url.clone(),
            None => return,
        };
        let (offset, frac) = (self.offset.get(), self.progress.get());
        self.reader.at_block = self.reader.block_at(self.reader.visible_rows, frac);
        self.reader.remember(&url, offset, frac);
        self.library.set_progress(&url, frac);
        self.library.save();
    }
}

/// `Family::Named` wants `&'static str`; the font list is a setting, so it is interned once per
/// distinct value. Settings change rarely, so the set stays tiny.
fn leak(s: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut seen = seen.lock().unwrap();
    if let Some(existing) = seen.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    seen.insert(leaked);
    leaked
}

fn init() -> State {
    State::new(Config::load(), Library::load())
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let p = s.palette();
    let hair = 1.0 / cx.env().scale;

    let surface = match s.view {
        View::Blank => empty::view(s, p, hair),
        View::Library => empty::library_view(s, p, hair),
        View::Reading => ui::reader::view(s, p, hair),
    };

    // The bar shows in three of the chrome's four states, and while it hides the article takes
    // the 56 points back: the surface's top padding is what moves, so nothing is hidden behind
    // a bar that is still there.
    let bar_visible = s.reader.chrome.is_visible();
    let top = if bar_visible { ui::BAR_H } else { 0.0 };

    let mut page = col()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.bg)
        .on_wheel(|s: &mut State, e: rune::WheelEvent, _tx: &mut rune::Tx| {
            if e.mods.ctrl || e.mods.cmd {
                if e.dy < -0.5 {
                    s.cfg.font_size = (s.cfg.font_size + 1.0).clamp(12.0, 36.0);
                    s.cfg.save();
                    return true;
                } else if e.dy > 0.5 {
                    s.cfg.font_size = (s.cfg.font_size - 1.0).clamp(12.0, 36.0);
                    s.cfg.save();
                    return true;
                }
            }
            false
        })
        .child(el().key("surface").grow(1.0).min_h(0.0).w_full().pt(top).child(surface));
    if s.cfg.show_progress_bar && s.view == View::Reading {
        page = page.child(progress(s, p));
    }

    // Overlays float above the page instead of joining its flex column: a pinned child is out of
    // flow, so opening the drawer leaves the article at full size underneath it. They start
    // below the titlebar so the address and controls stay reachable while a panel is open.
    let mut root = el()
        .key("root")
        .w_full()
        .h_full()
        .fill(p.bg)
        .child(page.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)));

    if bar_visible {
        root = root.child(titlebar(s, p, hair).pin(Some(0.0), Some(0.0), Some(0.0), None));
    }

    let below_bar = if bar_visible { ui::BAR_H + hair } else { hair };
    if s.show_toc && s.view == View::Reading {
        root = root.child(
            toc::overlay(s, p, hair).pin(Some(0.0), Some(below_bar), Some(0.0), Some(0.0)),
        );
    }
    if s.show_settings {
        root = root.child(
            drawer::overlay(s, p, hair).pin(Some(0.0), Some(below_bar), Some(0.0), Some(0.0)),
        );
    }
    // The tester sits over the reading surface's lower right corner, above the progress bar.
    if s.reader.tester.open && s.view == View::Reading {
        root = root.child(
            tester::panel(s, p, hair, cx).pin(Some(0.0), Some(below_bar), Some(0.0), Some(3.0)),
        );
    }
    root
}

/// Fetch a URL on a worker thread. The result lands in a shared slot the frame poll picks up.
fn start_fetch(url: String, slot: std::sync::Arc<std::sync::Mutex<Option<FetchResult>>>, wake: rune::Waker) {
    std::thread::spawn(move || {
        let result = article::fetch(&url);
        if let Ok(mut slot) = slot.lock() {
            *slot = Some(result);
        }
        wake();
    });
}

type FetchResult = Result<ArticleDoc, article::FetchError>;

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    // Reading is the main activity, and scrolling is the quietest way to do it: a wheel or a
    // glide that moved the page counts as the reader being here, without a per-move handler.
    let offset = s.offset.get();
    let scrolled = if (offset - s.reader.last_offset).abs() > 2.0 {
        s.reader.last_offset = offset;
        true
    } else {
        false
    };
    let mut chrome_changed = scrolled && s.reader.chrome.nudge();
    // The chrome's idle clock runs next: it is the one part of the frame that can hide or show
    // the titlebar, and its answer decides whether the rest of this poll needs to run the view.
    chrome_changed |= s.reader.chrome.advance(t, tx);
    let Some(slot) = SLOT.get() else { return chrome_changed };
    let taken = slot.lock().ok().and_then(|mut g| g.take());
    let Some(result) = taken else {
        // A saved place is revealed once the row it names has been built. A virtual list builds
        // only the rows near the viewport, so the request lands on the next commit after the
        // article opens, when the first window of rows exists.
        if let Some(block) = s.restore_block.take() {
            tx.scroll_into_view(format!("block-{block}"));
            return true;
        }
        if let Some(target) = s.scroll_to.take() {
            match target {
                // The title entry: back to the top of the article.
                None => tx.scroll_to_instant("article", 0.0, 0.0),
                Some(block) => tx.scroll_into_view(format!("block-{block}")),
            }
            return true;
        }
        // Keep the current search hit on screen. The highlight carries its own key, so the
        // reveal lands on the exact words rather than the block that holds them. Requested once
        // per hit change, not every frame, or it would fight the reader's own scrolling.
        if s.reveal_hit {
            s.reveal_hit = false;
            s.reveal_current_hit(tx);
            return true;
        }
        return chrome_changed;
    };
    match result {
        Ok(doc) => {
            s.library.upsert(&doc, 0.0);
            s.library.save();
            s.open(doc);
            tx.scroll_to_instant("article", 0.0, 0.0);
            // Hand the keyboard to the article, not to the titlebar: a page that opens with
            // nothing focused swallows the first Space, so the reader's first key does nothing.
            tx.focus("article", false);
        }
        Err(e) => {
            s.error = Some(e.to_string());
            s.loading = false;
        }
    }
    true
}

static SLOT: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Option<FetchResult>>>> =
    std::sync::OnceLock::new();
static WAKE: std::sync::OnceLock<rune::Waker> = std::sync::OnceLock::new();

fn setup(s: &mut State, wake: rune::Waker) {
    SLOT.set(std::sync::Arc::new(std::sync::Mutex::new(None))).ok();
    WAKE.set(wake).ok();
    // A URL on the command line opens straight away.
    if let Some(arg) = rune::extra_args().iter().find(|a| a.starts_with("http")) {
        s.pending = Some(arg.clone());
        s.loading = true;
    }
}

/// Start whatever fetch the state is asking for. Called from key and press handlers, which have
/// `&mut State` but no `Tx`.
pub fn kick_off(s: &mut State) {
    let Some(url) = s.pending.take() else { return };
    let (Some(slot), Some(wake)) = (SLOT.get(), WAKE.get()) else { return };
    s.loading = true;
    s.error = None;
    start_fetch(url, slot.clone(), wake.clone());
}

fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // While the find bar is open, it owns the keyboard: typing goes into the field, Enter and
    // the arrows step between hits, Escape closes. Everything else must not reach the vim
    // bindings, or typing "c" would open the TOC instead of searching for a "c".
    if s.find_focus {
        match ev.key {
            Key::Escape => {
                if let Some(hit) = s.current_hit() {
                    s.scroll_to = Some(Some(hit.block));
                    tx.scroll_into_view(format!("block-{}", hit.block));
                }
                s.find_focus = false;
                s.find_edit = rune::TextEdit::new("");
                s.hits.clear();
                s.hit_at = 0;
                s.hit_query.clear();
                tx.focus("article", false);
                return true;
            }
            Key::Enter => {
                s.step_hit_tx(if ev.mods.shift { -1 } else { 1 }, tx);
                return true;
            }
            Key::Down => {
                s.step_hit_tx(1, tx);
                return true;
            }
            Key::Up => {
                s.step_hit_tx(-1, tx);
                return true;
            }
            _ => return false,
        }
    }

    // While the address field is being edited, only Escape and Enter mean anything to Folio.
    if s.address_focus {
        match ev.key {
            Key::Escape => {
                s.address_focus = false;
                return true;
            }
            Key::Enter => {
                submit_address(s);
                kick_off(s);
                tx.focus("article", false);
                s.address_focus = false;
                return true;
            }
            _ => return false,
        }
    }

    match &ev.key {
        // Ctrl+F opens the find bar. On macOS Cmd is the command modifier and arrives as
        // `Key::Command`; elsewhere Ctrl+F arrives the same way, so one arm covers both.
        Key::Command('f') => {
            if s.view == View::Reading {
                s.find_focus = true;
                s.show_toc = false;
                tx.focus("find", false);
            }
            true
        }
        // The selector tester: Ctrl+Shift+D. Checked before the vim keys so `d` never scrolls
        // when the reader asked for the panel.
        _ if tester::is_open_chord(ev) => {
            if s.view == View::Reading {
                s.reader.tester.open = !s.reader.tester.open;
                if s.reader.tester.open {
                    tx.focus("tester-field", false);
                } else {
                    tx.focus("article", false);
                }
            }
            true
        }
        // Folio's own keys: t theme, s settings, c TOC, g library, / address, r reload.
        Key::Text(t) if t.eq_ignore_ascii_case("t") => {
            s.cfg.cycle_theme();
            s.cfg.save();
            tx.color_motion(motion::CALM);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("s") => {
            let open = !s.show_settings;
            s.show_settings = open;
            s.chrome_panel(open);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("c") => {
            if s.view == View::Reading {
                let open = !s.show_toc;
                s.show_toc = open;
                s.chrome_panel(open);
            }
            true
        }
        // `z` toggles distraction-free reading: the bar steps out of the way at once, and a
        // second press brings it back.
        Key::Text(t) if t.eq_ignore_ascii_case("z") => {
            s.reader.distraction_free = !s.reader.distraction_free;
            let on = s.reader.distraction_free;
            s.reader.chrome.set_enabled(on);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("g") => {
            s.save_place();
            s.view = View::Library;
            s.show_toc = false;
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("r") => {
            if let Some(doc) = &s.doc {
                s.pending = Some(doc.url.clone());
                kick_off(s);
            }
            true
        }
        Key::Text(t) if t == "/" || t.eq_ignore_ascii_case("o") => {
            // In reading view the address lives in the titlebar; on the empty page it is the
            // big field. Either way `/` hands it the keyboard.
            s.address_focus = true;
            tx.focus("address", false);
            true
        }
        // Vim scrolling, so the reading surface never needs the pointer.
        Key::Text(t) if t == "j" => {
            scroll_by(s, tx, s.cfg.scroll_step_px);
            true
        }
        Key::Text(t) if t == "k" => {
            scroll_by(s, tx, -s.cfg.scroll_step_px);
            true
        }
        Key::Text(t) if t == "d" => {
            scroll_by(s, tx, 400.0);
            true
        }
        Key::Text(t) if t == "u" => {
            scroll_by(s, tx, -400.0);
            true
        }
        Key::PageDown => {
            page_by(s, tx, 1.0);
            true
        }
        Key::PageUp => {
            page_by(s, tx, -1.0);
            true
        }
        // Space / Shift+Space / Backspace page the article the way a pager does. Space already means
        // "next page" everywhere else, so it costs nothing to learn. Space arrives as
        // `Key::Text(" ")`, not a named key.
        Key::Text(t) if t == " " => {
            if ev.mods.shift {
                page_by(s, tx, -1.0);
            } else {
                page_by(s, tx, 1.0);
            }
            true
        }
        Key::Backspace => {
            page_by(s, tx, -1.0);
            true
        }
        Key::Command('+') | Key::Command('=') => {
            s.cfg.font_size = (s.cfg.font_size + 1.0).clamp(12.0, 36.0);
            s.cfg.save();
            true
        }
        Key::Command('-') | Key::Command('_') => {
            s.cfg.font_size = (s.cfg.font_size - 1.0).clamp(12.0, 36.0);
            s.cfg.save();
            true
        }
        Key::Command('0') => {
            s.cfg.font_size = 18.0;
            s.cfg.save();
            true
        }
        Key::Escape => {
            if s.show_settings {
                s.show_settings = false;
            } else if s.show_toc {
                s.show_toc = false;
            } else if s.view == View::Reading {
                s.view = View::Blank;
            }
            true
        }
        Key::Command('q') => {
            tx.quit();
            true
        }
        Key::Command('w') => {
            tx.close_window();
            true
        }
        Key::Command('c') => {
            if let Some(doc) = &s.doc {
                tx.copy(doc.plain.clone());
            }
            true
        }
        _ => false,
    }
}

fn scroll_by(s: &mut State, tx: &mut Tx, dy: f32) {
    let target = (s.offset.get() + dy).max(0.0);
    tx.scroll_to_y("article", target);
}

/// Page the article by `pages` viewports, gliding there rather than cutting.
///
/// `Tx::scroll_page` is instant by design (it is what Page Down does in a text editor), which
/// makes a whole screen of prose jump under the eye. Paging a long read wants the same 90% of
/// the viewport, moved on Rune's glide spring so the eye can follow the text instead of
/// re-finding its place. A second press mid-glide keeps the speed, so holding Space rolls
/// through the chapter in one motion.
fn page_by(s: &mut State, tx: &mut Tx, pages: f32) {
    let page = (s.scroll_bar.view_h.get() * 0.9).max(1.0);
    let target = (s.offset.get() + page * pages).max(0.0);
    tx.scroll_to("article", 0.0, target);
}

fn submit_address(s: &mut State) {
    let raw = s.edit.text.trim().to_string();
    if raw.is_empty() {
        return;
    }
    let url = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw
    } else if raw.contains('.') && !raw.contains(' ') {
        format!("https://{raw}")
    } else {
        // Not a URL: treat it as a search on the article-friendly reader.
        format!("https://duckduckgo.com/html/?q={}", urlencode(&raw))
    };
    s.pending = Some(url);
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn main() {
    rune::set_app_id("dev.personal.folio");
    rune::run(AppSpec::<State> {
        min_size: Some((520.0, 400.0)),
        scenes: &[
            ("blank", |s| s.view = View::Blank),
            ("reading", |s| demo::reading(s)),
            ("toc", |s| {
                demo::reading(s);
                s.show_toc = true;
            }),
            ("settings", |s| {
                demo::reading(s);
                s.show_settings = true;
            }),
            ("library", |s| {
                demo::seed_library(s);
                s.view = View::Library;
            }),
            ("light", |s| {
                demo::reading(s);
                s.cfg.theme = "light".into();
            }),
            ("sepia", |s| {
                demo::reading(s);
                s.cfg.theme = "sepia".into();
            }),
            ("dark", |s| {
                demo::reading(s);
                s.cfg.theme = "dark".into();
            }),
            ("charcoal", |s| {
                demo::reading(s);
                s.cfg.theme = "charcoal".into();
            }),
            ("chapter", |s| {
                // The real animeanyway page, so a snapshot exercises the __NEXT_DATA__ path.
                if let Some(doc) = std::fs::read_to_string("/tmp/page.html")
                    .ok()
                    .and_then(|html| article::extract(&html, "https://animeanyway.com/y3v5/ch4").ok())
                {
                    s.open(doc);
                }
            }),
            ("address", |s| {
                demo::reading(s);
                s.address_focus = true;
            }),
            ("find", |s| {
                demo::reading(s);
                s.find_focus = true;
                s.find_edit = rune::TextEdit::new("the");
                s.refresh_hits();
            }),
            ("chapter-toc", |s| {
                // The real chapter, whose TOC is built from its section labels.
                if let Some(doc) = std::fs::read_to_string("/tmp/page.html")
                    .ok()
                    .and_then(|html| article::extract(&html, "https://animeanyway.com/y3v5/ch4").ok())
                {
                    s.open(doc);
                }
                s.show_toc = true;
            }),
            ("oled", |s| {
                demo::reading(s);
                s.cfg.theme = "oled".into();
            }),
            ("empty-lib", |s| {
                s.library = Library::default();
                s.view = View::Library;
            }),
        ],
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Folio", (900.0, 720.0), init, view)
    });
}

/// Sample content for the headless scenes: no network in a snapshot run.
mod demo {
    use super::*;
    use crate::article::{Block, Inline, InlineKind};

    const BODY: &str = "A quiet page is not an empty one. It is a page that has made up its mind \
about what it is for, and then removed everything else. The margins do most of the work: they \
hold the text away from the edges of the window the way a frame holds a picture, and in doing so \
they tell you where to look.";

    const MORE: &str = "Typography is the whole interface here. There is no toolbar to reach for, \
no chrome to decode, no icon that needs a tooltip. The size of the letters, the space between the \
lines, and the length of the measure are the only controls that matter, and they are all set once \
and then left alone.";

    pub fn reading(s: &mut State) {
        let blocks = vec![
            Block::Heading { level: 1, text: "The Quiet Page".into() },
            Block::Para {
                spans: vec![
                    Inline { text: "Reading is a ".into(), kind: InlineKind::Text },
                    Inline { text: "slow".into(), kind: InlineKind::Bold },
                    Inline { text: " activity, and that is the point. ".into(), kind: InlineKind::Text },
                    Inline { text: "Every reader worth keeping".into(), kind: InlineKind::Link("https://example.com/readers".into()) },
                    Inline { text: " gives the words room.".into(), kind: InlineKind::Text },
                ],
            },
            Block::Para { spans: vec![Inline { text: BODY.into(), kind: InlineKind::Text }] },
            Block::Heading { level: 2, text: "Measure".into() },
            Block::Para { spans: vec![Inline { text: MORE.into(), kind: InlineKind::Text }] },
            Block::Quote {
                spans: vec![Inline {
                    text: "Attention is the rarest and purest form of generosity.".into(),
                    kind: InlineKind::Text,
                }],
            },
            Block::Para {
                spans: vec![Inline {
                    text: "A line of type wants somewhere between sixty and seventy-five \
characters. Shorter and the eye jumps too often; longer and it loses the next line on the way \
back. Folio's measure is fixed at seven hundred and twenty points for exactly this reason."
                        .into(),
                    kind: InlineKind::Text,
                }],
            },
            Block::Heading { level: 2, text: "Restraint".into() },
            Block::List {
                ordered: false,
                start: 1,
                items: vec![
                    vec![Inline { text: "One column, never two.".into(), kind: InlineKind::Text }],
                    vec![Inline { text: "Serif for prose, sans for the frame.".into(), kind: InlineKind::Text }],
                    vec![Inline { text: "A progress bar three points tall, and no more.".into(), kind: InlineKind::Text }],
                ],
            },
            Block::Para {
                spans: vec![Inline {
                    text: "Everything else is a distraction wearing the costume of a feature. The \
reader should be able to forget the application entirely, and remember only the article."
                        .into(),
                    kind: InlineKind::Text,
                }],
            },
            Block::Code { text: "let measure = 720.0;\nlet leading = 1.7;".into() },
            Block::Para {
                spans: vec![Inline {
                    text: "That is the whole design. The rest is just making sure it holds at every \
window size, in every theme, at every font size a reader might need."
                        .into(),
                    kind: InlineKind::Text,
                }],
            },
        ];
        let plain = blocks
            .iter()
            .map(|b| match b {
                Block::Heading { text, .. } => text.clone(),
                Block::Para { spans } | Block::Quote { spans } => {
                    spans.iter().map(|s| s.text.as_str()).collect::<String>()
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
        s.doc = Some(ArticleDoc {
            url: "https://example.com/the-quiet-page".into(),
            title: "The Quiet Page".into(),
            byline: Some("Dheo".into()),
            site: Some("Example".into()),
            excerpt: None,
            blocks,
            plain,
            next_chapter_url: None,
        });
        // The demo bypasses `open`, so seed the address field by hand — otherwise the titlebar
        // shows its placeholder instead of the article being read.
        s.edit = rune::TextEdit::new("https://example.com/the-quiet-page");
        s.view = View::Reading;
        s.loading = false;
        s.error = None;
        s.progress.set(0.34);
    }

    pub fn seed_library(s: &mut State) {
        reading(s);
        if let Some(doc) = s.doc.clone() {
            s.library.upsert(&doc, 0.34);
        }
        let mut second = s.doc.clone().unwrap();
        second.title = "Against the Infinite Scroll".into();
        second.url = "https://example.com/against-the-scroll".into();
        second.site = Some("Example".into());
        s.library.upsert(&second, 0.0);
        s.view = View::Library;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_key_cycles_through_all_five_presets() {
        let mut cfg = Config::default();
        let mut seen = vec![cfg.theme.clone()];
        for _ in 0..4 {
            cfg.cycle_theme();
            seen.push(cfg.theme.clone());
        }
        assert_eq!(seen, vec!["light", "sepia", "dark", "charcoal", "oled"]);
        cfg.cycle_theme();
        assert_eq!(cfg.theme, "light", "and wraps back around");
    }

    #[test]
    fn bare_hosts_gain_a_scheme_and_other_text_becomes_a_search() {
        let mut s = State::new(Config::default(), Library::default());
        s.edit = rune::TextEdit::new("example.com/post");
        submit_address(&mut s);
        assert_eq!(s.pending.as_deref(), Some("https://example.com/post"));

        let mut s = State::new(Config::default(), Library::default());
        s.edit = rune::TextEdit::new("quiet page");
        submit_address(&mut s);
        assert!(s.pending.unwrap().starts_with("https://duckduckgo.com/html/?q=quiet+page"));
    }

    #[test]
    fn the_default_measure_matches_folios() {
        let cfg = Config::default();
        assert_eq!(cfg.max_width, 720.0);
        assert_eq!(cfg.margin, 32.0);
        assert_eq!(cfg.line_height, 1.7);
        assert_eq!(cfg.font_size, 18.0);
        assert_eq!(cfg.ui_font_size, 20.0);
        assert_eq!(cfg.paragraph_spacing_px, 16.0);
        assert!(!cfg.justified);
        assert_eq!((cfg.window_width, cfg.window_height), (900.0, 720.0));
    }
}
