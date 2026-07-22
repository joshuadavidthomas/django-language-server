use camino::Utf8Path;
use djls_conf::FormatBackend;
use djls_ide::format_document;
use djls_source::PositionEncoding;
use djls_testing::TestDatabase;
use tower_lsp_server::ls_types;

fn formatting_options() -> ls_types::FormattingOptions {
    ls_types::FormattingOptions {
        tab_size: 4,
        insert_spaces: true,
        ..Default::default()
    }
}

#[test]
fn format_document_returns_full_document_edit() {
    let source = "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}');\">\n    Content\n</div>\n";
    let db = TestDatabase::new();
    db.add_file("template.html", source)
        .expect("template fixture should be added");
    let file = db
        .file(Utf8Path::new("template.html"))
        .expect("template fixture file should exist");
    let options = formatting_options();

    let edits = format_document(
        &db,
        file,
        PositionEncoding::Utf16,
        FormatBackend::Djangofmt,
        &options,
    );

    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0].range,
        ls_types::Range::new(ls_types::Position::new(0, 0), ls_types::Position::new(3, 0)),
    );
    assert_eq!(
        edits[0].new_text,
        "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}')\">\n    Content\n</div>\n",
    );
}
