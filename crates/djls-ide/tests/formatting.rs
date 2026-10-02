use camino::Utf8Path;
use djls_conf::FormatBackend;
use djls_ide::format_document;
use djls_source::PositionEncoding;
use djls_testing::TestDatabase;
use djls_testing::capture_events;
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

#[test]
fn format_document_logs_failures_by_cause() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
    std::fs::create_dir(root.join("configured")).expect("project directory");
    std::fs::write(root.join("configured/pyproject.toml"), "[tool.djangofmt\n")
        .expect("broken pyproject");
    let broken_config = root.join("configured/template.html");
    let broken_template = root.join("template.html");
    let db = TestDatabase::new();
    db.add_file(broken_config.as_str(), "<div>ok</div>\n")
        .expect("template fixture should be added");
    db.add_file(broken_template.as_str(), "<div class=\"\n")
        .expect("template fixture should be added");
    let options = formatting_options();

    let (edits, config_events) = capture_events(|| {
        format_document(
            &db,
            db.file(&broken_config)
                .expect("template fixture file should exist"),
            PositionEncoding::Utf16,
            FormatBackend::Djangofmt,
            &options,
        )
    });
    assert!(edits.is_empty());
    let visible = &config_events.default_visible;
    assert!(
        visible.contains("WARN message=Could not load formatter configuration"),
        "{visible}"
    );
    assert!(!visible.contains(root.as_str()), "leaked path: {visible}");
    assert!(
        config_events
            .debug
            .contains("Failed to parse pyproject.toml"),
        "{}",
        config_events.debug
    );

    let (edits, template_events) = capture_events(|| {
        format_document(
            &db,
            db.file(&broken_template)
                .expect("template fixture file should exist"),
            PositionEncoding::Utf16,
            FormatBackend::Djangofmt,
            &options,
        )
    });
    assert!(edits.is_empty());
    let visible = &template_events.default_visible;
    assert!(!visible.contains("WARN"), "{visible}");
    assert!(!visible.contains("Template"), "{visible}");
    assert!(
        template_events.debug.contains("Template formatting failed"),
        "{}",
        template_events.debug
    );
}
