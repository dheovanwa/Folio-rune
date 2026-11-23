#[path = "../src/article.rs"]
mod article;

use article::{extract, Block, FetchError, InlineKind};

#[test]
fn test_standard_article_extraction() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Test Article Title</title></head>
        <body>
            <article>
                <h2>Main Heading</h2>
                <p>This is a paragraph with <b>bold</b>, <i>italic</i>, and <a href="/sub/page">a link</a>.</p>
                <blockquote><p>A quoted block of text.</p></blockquote>
                <ul>
                    <li>First bullet</li>
                    <li>Second bullet</li>
                </ul>
                <pre><code>let x = 42;</code></pre>
            </article>
        </body>
        </html>
    "#;

    let doc = extract(html, "https://example.com/test").expect("extraction should succeed");
    assert_eq!(doc.title, "Test Article Title");
    assert!(!doc.blocks.is_empty());

    let mut has_heading = false;
    let mut has_para = false;
    let mut has_quote = false;
    let mut has_list = false;
    let mut has_code = false;

    for block in &doc.blocks {
        match block {
            Block::Heading { level, text } => {
                has_heading = true;
                assert!(matches!(*level, 1 | 2));
                assert_eq!(text, "Main Heading");
            }
            Block::Para { spans } => {
                has_para = true;
                assert!(!spans.is_empty());
                let link_span = spans.iter().find(|s| matches!(s.kind, InlineKind::Link(_)));
                assert!(link_span.is_some(), "link span should exist");
                if let Some(s) = link_span {
                    assert_eq!(s.kind, InlineKind::Link("https://example.com/sub/page".into()));
                }
            }
            Block::Quote { .. } => {
                has_quote = true;
            }
            Block::List { ordered, items, .. } => {
                has_list = true;
                assert!(!*ordered);
                assert_eq!(items.len(), 2);
            }
            Block::Code { text } => {
                has_code = true;
                assert!(text.contains("let x = 42;"));
            }
            _ => {}
        }
    }

    assert!(has_heading, "Heading block missing");
    assert!(has_para, "Para block missing");
    assert!(has_quote, "Quote block missing");
    assert!(has_list, "List block missing");
    assert!(has_code, "Code block missing");
}

#[test]
fn test_empty_and_unreadable_html_error_handling() {
    let empty_html = "<html><body></body></html>";
    let res = extract(empty_html, "https://example.com/empty");
    assert!(res.is_err());
    match res.unwrap_err() {
        FetchError::Empty | FetchError::NotReadable => {}
        err => panic!("Unexpected error: {err:?}"),
    }

    let script_only = "<html><body><script>var x = 1;</script></body></html>";
    let res2 = extract(script_only, "https://example.com/script");
    assert!(res2.is_err());
}

#[test]
fn test_next_data_extraction_and_fallback() {
    // Valid Next.js __NEXT_DATA__
    let next_data_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Fallback Title</title></head>
        <body>
            <script id="__NEXT_DATA__" type="application/json">
            {
                "props": {
                    "pageProps": {
                        "chapter": {
                            "title": "Synthetic Chapter 1",
                            "content": [
                                {
                                    "_type": "block",
                                    "style": "normal",
                                    "children": [
                                        { "_type": "span", "text": "This is synthetic prose from NextData." }
                                    ],
                                    "markDefs": []
                                },
                                {
                                    "_type": "readerSection",
                                    "label": "Section A"
                                },
                                {
                                    "_type": "block",
                                    "style": "normal",
                                    "children": [
                                        { "_type": "span", "text": "Paragraph following section break." }
                                    ],
                                    "markDefs": []
                                }
                            ]
                        }
                    }
                }
            }
            </script>
        </body>
        </html>
    "#;

    let doc = extract(next_data_html, "https://example.com/ch1").expect("should extract NextData");
    assert_eq!(doc.title, "Synthetic Chapter 1");
    assert_eq!(doc.blocks.len(), 3);
    assert_eq!(doc.blocks[1], Block::Heading { level: 2, text: "Section A".into() });

    // Next.js script with invalid JSON falls back to readability parser
    let fallback_html = r#"
        <!DOCTYPE html>
        <html>
        <head><title>Readability Fallback</title></head>
        <body>
            <script id="__NEXT_DATA__" type="application/json">invalid json</script>
            <article>
                <p>Readable content when Next.js parsing fails.</p>
            </article>
        </body>
        </html>
    "#;

    let fallback_doc = extract(fallback_html, "https://example.com/fallback").expect("fallback to readability");
    assert_eq!(fallback_doc.title, "Readability Fallback");
    assert!(fallback_doc.plain.contains("Readable content when Next.js parsing fails."));
}

#[test]
fn test_missing_title_falls_back_to_url() {
    let no_title_html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <article>
                <p>Some readable text content that is sufficiently long for readability parser.</p>
            </article>
        </body>
        </html>
    "#;

    let doc = extract(no_title_html, "https://example.com/article-slug").expect("extraction should succeed");
    assert!(!doc.title.is_empty());
}
