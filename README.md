# Folio

> **A distraction-free article & light novel reader, rebuilt natively with [Rune](https://github.com/anomalyco/rune).**

Folio provides a quiet, typography-focused reading surface for long-form web articles, serial novels, and documentation: serif prose at a fixed measure, a hairline reading progress indicator, a sliding Table of Contents (TOC), instant distraction-free toggling, and rich keyboard navigation.

---

## Key Features & Architecture

### 1. Native Rune Reading Surface & UX (`src/reader/`, `src/ui/`)
- **Viewport Virtualization**: Long chapters (100k+ words) and multi-section documents render only visible paragraphs, keeping CPU and memory overhead minimal with fluid 60+ FPS scrolling.
- **Distraction-Free Auto-Hide Toolbar**: The top navigation chrome slides away during focused reading and smoothly returns on scroll nudge, mouse movement, or hotkey (`z`).
- **Reading Position Memory**: Tracks reading progress down to the specific paragraph block index per URL.
- **Visual Themes**: Built-in support for Light, Warm Sepia, Dark, OLED (pure black), and Charcoal themes (`t`).
- **Live Selector Tester (`Ctrl+Shift+D`)**: Integrated floating panel for interactively testing CSS selectors on open web pages without leaving the reader.

### 2. Smart Parser & Site Profiles (`src/parser/`, `src/article.rs`, `profiles/`)
- **Zero-Config Readability Heuristics**: Automatic content density extraction when visiting sites without custom profiles.
- **Configurable Site Profiles (`profiles/*.toml`)**: Declarative configuration for custom title, byline, content container, exclude rules, and regex pattern stripping.
- **Next Chapter Auto-Detection**: Extracts sequential chapter links (`ArticleDoc::next_chapter_url`) via selectors, `rel="next"`, or textual navigation cues for seamless reading.
- **Regex Content Sanitization**: Strips in-text advertisements, site watermarks, and junk lines while preserving typographic formatting.

### 3. Polite Fetcher & Local Cache (`src/fetcher/`, `src/cache/`)
- **Connection Pooling**: Process-wide singleton `ureq::Agent` for reusing TCP/TLS connections.
- **Adaptive Rate Limiting & Backoff**: Per-domain request spacing and exponential backoff honoring HTTP `Retry-After` (HTTP 429 prevention).
- **Speculative Background Prefetching**: Automatically queues and pre-caches Chapter N+1 and N+2 in the background.
- **Crash-Resilient Local Cache**: Zero-dependency `FileStore` with atomic rename operations, size budgeting, and oldest-first eviction.

### 4. Headless Visual Snapshot Testing & QA (`tests/`, `snaps/`)
- Headless rendering mode (`--snap <file.png> --scene <name> --tree <tree.json>`) enabling automated pixel-level visual regression validation across 15 standard UI scenes.

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `t` | Cycle reading theme (Light, Sepia, Dark, OLED, Charcoal) |
| `s` | Toggle Settings drawer |
| `c` | Toggle Table of Contents (TOC) overlay |
| `g` | Open local Library view |
| `z` | Toggle distraction-free mode (hide/show navigation bar) |
| `/` | Focus address bar to enter a URL |
| `f` / `Ctrl+F` | Find in article |
| `Ctrl+Shift+D` | Open / close Live Selector Tester panel |
| `Space` | Page down with smooth spring glide |
| `Shift+Space` | Page up with smooth spring glide |
| `j` / `Down` | Scroll down |
| `k` / `Up` | Scroll up |
| `r` | Reload current article |
| `Esc` | Close active overlay, panel, or return to reading surface |
| `Cmd+W` / `Ctrl+W` | Close window |
| `Cmd+Q` / `Ctrl+Q` | Quit application |

---

## Getting Started

### Prerequisites
- [Rust toolchain](https://www.rust-lang.org/) (Rust 1.85+ / Edition 2024 recommended).
- Platform graphics libraries supported by `wgpu` (Vulkan, Metal, or DirectX 12).

### Installation & Running

Clone the repository and run the application:

```bash
git clone https://github.com/dheovanwa/Folio-rune.git
cd Folio-rune
cargo run
```

### Running Tests

Run the full automated test suite:

```bash
cargo test
```

### Headless Snapshot Verification

To test headless rendering and visual snapshot capture:

```bash
# Render a specific scene headlessly to PNG and JSON tree
cargo run -- --snap snaps/reading.png --scene reading --tree snaps/reading.json

# Execute the complete automated visual verifier suite
cargo test --test visual_verifier
```

---

## Site Profiles (`profiles/`)

Custom extraction profiles are placed inside the `profiles/` directory in TOML format:

```toml
name = "Example Web Novel"
host = "example.com"
aliases = ["m.example.com"]

[extract]
title = "h1.chapter-title"
byline = ".author-name"
container = ".chapter-content"
next = "a.next-chapter-link"
exclude = [
    ".inline-ad",
    ".social-share"
]

[strip]
patterns = [
    "Read more at example.com",
    "Support the author on Patreon"
]
```

---

## License

This project is licensed under the [MIT License](LICENSE).
