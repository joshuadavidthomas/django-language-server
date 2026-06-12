use camino::Utf8Path;
use djls_source::FileKind;
use djls_source::LineCol;
use djls_source::PositionEncoding;
use djls_source::Range;
use djls_testing::TestDatabase;

use super::*;

fn text_document(content: &str, version: i32) -> TextDocument {
    let db = TestDatabase::new();
    let path = Utf8Path::new("/test.txt");
    let file = db.get_or_create_file(path);
    TextDocument::new(
        path.to_path_buf(),
        content.to_string(),
        version,
        FileKind::Other,
        file,
    )
}

#[test]
fn incremental_update_single_change() {
    let mut doc = text_document("Hello world", 1);

    let changes = vec![DocumentChange::new(
        Some(Range::new(LineCol::new(0, 6), LineCol::new(0, 11))),
        "Rust".to_string(),
    )];

    doc.update(changes, 2, PositionEncoding::Utf16);
    assert_eq!(doc.content(), "Hello Rust");
    assert_eq!(doc.version(), 2);
}

#[test]
fn incremental_update_multiple_changes() {
    let mut doc = text_document("First line\nSecond line\nThird line", 1);

    let changes = vec![
        DocumentChange::new(
            Some(Range::new(LineCol::new(0, 0), LineCol::new(0, 5))),
            "1st".to_string(),
        ),
        DocumentChange::new(
            Some(Range::new(LineCol::new(2, 0), LineCol::new(2, 5))),
            "3rd".to_string(),
        ),
    ];

    doc.update(changes, 2, PositionEncoding::Utf16);
    assert_eq!(doc.content(), "1st line\nSecond line\n3rd line");
    assert_eq!(doc.version(), 2);
}

#[test]
fn full_document_replacement() {
    let mut doc = text_document("Old content", 1);

    let changes = vec![DocumentChange::new(
        None,
        "Completely new content".to_string(),
    )];

    doc.update(changes, 2, PositionEncoding::Utf16);
    assert_eq!(doc.content(), "Completely new content");
    assert_eq!(doc.version(), 2);
}

#[test]
fn incremental_update_with_emoji() {
    let mut doc = text_document("Hello 🌍 world", 1);

    let changes = vec![DocumentChange::new(
        Some(Range::new(LineCol::new(0, 9), LineCol::new(0, 14))),
        "Rust".to_string(),
    )];

    doc.update(changes, 2, PositionEncoding::Utf16);
    assert_eq!(doc.content(), "Hello 🌍 Rust");
}
