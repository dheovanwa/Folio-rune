#[path = "../src/article.rs"]
mod article;
#[path = "../src/library.rs"]
mod library;

use article::{ArticleDoc, Block, Inline, InlineKind};
use library::{relative_time, Entry, Library, StoredDoc};

fn create_sample_doc(url: &str, title: &str) -> ArticleDoc {
    ArticleDoc {
        url: url.to_string(),
        title: title.to_string(),
        byline: Some("Author Name".to_string()),
        site: Some("Example Site".to_string()),
        excerpt: Some("Short excerpt...".to_string()),
        blocks: vec![
            Block::Heading { level: 2, text: "Sample Heading".to_string() },
            Block::Para {
                spans: vec![Inline {
                    text: "Sample paragraph text.".to_string(),
                    kind: InlineKind::Text,
                }],
            },
        ],
        plain: "Sample paragraph text.".to_string(),
        next_chapter_url: None,
    }
}

#[test]
fn test_library_upsert_and_find() {
    let mut lib = Library::default();
    let doc1 = create_sample_doc("https://example.com/art1", "Article One");
    lib.upsert(&doc1, 0.25);

    assert_eq!(lib.entries.len(), 1);
    let entry = lib.find("https://example.com/art1").expect("entry should exist");
    assert_eq!(entry.title, "Article One");
    assert_eq!(entry.progress, 0.25);

    // Updating existing preserves original saved_at
    let original_saved_at = entry.saved_at;
    let mut updated_doc = doc1.clone();
    updated_doc.title = "Article One (Updated)".to_string();
    lib.upsert(&updated_doc, 0.75);

    assert_eq!(lib.entries.len(), 1);
    let updated_entry = lib.find("https://example.com/art1").unwrap();
    assert_eq!(updated_entry.title, "Article One (Updated)");
    assert_eq!(updated_entry.progress, 0.75);
    assert_eq!(updated_entry.saved_at, original_saved_at);
}

#[test]
fn test_library_sorting_newest_first() {
    let mut lib = Library::default();
    let doc1 = create_sample_doc("https://example.com/art1", "Article 1");
    let doc2 = create_sample_doc("https://example.com/art2", "Article 2");
    let doc3 = create_sample_doc("https://example.com/art3", "Article 3");

    lib.upsert(&doc1, 0.0);
    lib.upsert(&doc2, 0.5);
    lib.upsert(&doc3, 1.0);

    // Manually adjust saved_at to verify sorting
    lib.entries[0].saved_at = 1000;
    lib.entries[1].saved_at = 3000;
    lib.entries[2].saved_at = 2000;

    let sorted = lib.sorted();
    assert_eq!(sorted.len(), 3);
    assert_eq!(sorted[0].url, "https://example.com/art2"); // 3000
    assert_eq!(sorted[1].url, "https://example.com/art3"); // 2000
    assert_eq!(sorted[2].url, "https://example.com/art1"); // 1000
}

#[test]
fn test_library_set_progress_and_remove() {
    let mut lib = Library::default();
    let doc = create_sample_doc("https://example.com/art1", "Article 1");
    lib.upsert(&doc, 0.1);

    lib.set_progress("https://example.com/art1", 0.95);
    assert_eq!(lib.find("https://example.com/art1").unwrap().progress, 0.95);

    lib.remove("https://example.com/art1");
    assert!(lib.find("https://example.com/art1").is_none());
    assert!(lib.entries.is_empty());
}

#[test]
fn test_library_json_serialization_roundtrip() {
    let mut lib = Library::default();
    let doc = create_sample_doc("https://example.com/roundtrip", "Roundtrip Article");
    lib.upsert(&doc, 0.42);

    let json_str = serde_json::to_string_pretty(&lib).expect("serialization should succeed");
    assert!(json_str.contains("https://example.com/roundtrip"));
    assert!(json_str.contains("Roundtrip Article"));

    let deserialized: Library = serde_json::from_str(&json_str).expect("deserialization should succeed");
    assert_eq!(deserialized.entries.len(), 1);
    let entry = &deserialized.entries[0];
    assert_eq!(entry.url, "https://example.com/roundtrip");
    assert_eq!(entry.title, "Roundtrip Article");
    assert_eq!(entry.progress, 0.42);
    assert_eq!(entry.doc.blocks.len(), 2);
}

#[test]
fn test_relative_time_formatting() {
    let now = library::now_secs();
    assert_eq!(relative_time(now), "just now");
    assert_eq!(relative_time(now - 120), "2m ago");
    assert_eq!(relative_time(now - 7200), "2h ago");
    assert_eq!(relative_time(now - 86400 * 3), "3d ago");
}
