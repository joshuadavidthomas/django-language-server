use djls_conf::FormatBackend;
use djls_format::FormatOutcome;
use djls_source::Db;
use djls_source::File;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

#[must_use]
pub fn format_document(
    db: &dyn Db,
    file: File,
    encoding: PositionEncoding,
    backend: FormatBackend,
) -> Vec<ls_types::TextEdit> {
    let source = file.source(db);
    let path = file.path(db);

    match djls_format::format_template(source.as_str(), path, backend) {
        Ok(FormatOutcome::Changed(formatted)) => {
            let (line, character) = file.end_line_col(db, encoding).into();
            vec![ls_types::TextEdit::new(
                ls_types::Range::new(
                    ls_types::Position::new(0, 0),
                    ls_types::Position::new(line, character),
                ),
                formatted,
            )]
        }
        Ok(FormatOutcome::Unchanged | FormatOutcome::Ignored) => Vec::new(),
        Err(error) => {
            tracing::debug!("Formatting failed for {path}: {error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_conf::FormatBackend;
    use djls_source::Db as _;
    use djls_source::PositionEncoding;
    use djls_source::SourceFiles;
    use tower_lsp_server::ls_types;

    use super::format_document;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        source: String,
    }

    impl TestDb {
        fn new(source: impl Into<String>) -> Self {
            Self {
                storage: salsa::Storage::new(None),
                files: SourceFiles::default(),
                source: source.into(),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(self.source.clone())
        }
    }

    #[test]
    fn format_document_returns_full_document_edit() {
        let source = "<div style=\"background-image: url('{{ MEDIA_URL }}{{ picture }}');\">\n    Content\n</div>\n";
        let db = TestDb::new(source);
        let file = db.create_file(Utf8Path::new("template.html"));

        let edits = format_document(&db, file, PositionEncoding::Utf16, FormatBackend::Djangofmt);

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
}
