# Folio site profiles

One TOML file per host. [`read.example.toml`](read.example.toml) is a commented template: copy it,
change the `domain`, and delete the sections you do not need. Every section is optional except
`domain`.

A profile is never required. With no file for a host, Folio reads the page with its zero-config
heuristic and shows the article without any cleanup.

## What each key does

| Key | Type | Effect |
| --- | --- | --- |
| `domain` | string | The host the profile applies to. Required. |
| `name` | string | A label for messages. Optional. |
| `aliases` | list of strings | Other hosts to match. `*.read.example` covers the bare host and subdomains. |
| `extract.selector` | CSS selector | The container holding the article. Skips the container scoring. |
| `extract.exclude` | list of CSS selectors | Subtrees removed from that container before reading. |
| `extract.next` | CSS selector | The link to the next chapter, for sites whose label does not say so. |
| `sanitize.drop_blocks` | list of patterns | Drop a block whose whole text matches. |
| `sanitize.strip_text` | list of literal strings | Cut this text out of any block that contains it. |

## Patterns

Patterns are globs, not regexes: `*` is any run of characters, `?` is exactly one character, and
`|` separates alternatives. Matching is case-insensitive and ignores repeated spaces, so
`"READ   more at"` matches `"Read more at"`.

`strip_text` compares the exact string, because it has to name what to remove, so it cannot
contain `*` or `?`.

## Checking a profile

A broken file never stops a fetch: Folio reports it and falls back to the heuristic. To see the
report, run the test that loads every shipped profile:

```sh
cargo test parser::profile::tests::the_shipped_profiles_all_load
```
