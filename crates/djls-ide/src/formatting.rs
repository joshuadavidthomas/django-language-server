use djls_conf::FormatBackend;
use djls_format::FormatOptions;
use djls_format::FormatOutcome;
use djls_format::IndentStyle;
use djls_format::IndentWidth;
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
    formatting_options: &ls_types::FormattingOptions,
) -> Vec<ls_types::TextEdit> {
    let source = file.source(db);
    let path = file.path(db);

    let indent_width = u8::try_from(formatting_options.tab_size)
        .ok()
        .and_then(|width| IndentWidth::try_from(width).ok());
    let indent_style = if formatting_options.insert_spaces {
        IndentStyle::Spaces
    } else {
        IndentStyle::Tabs
    };
    let format_options = FormatOptions::new(indent_width, Some(indent_style))
        .trim_trailing_whitespace(formatting_options.trim_trailing_whitespace.unwrap_or(false))
        .insert_final_newline(formatting_options.insert_final_newline.unwrap_or(false))
        .trim_final_newlines(formatting_options.trim_final_newlines.unwrap_or(false));

    let formatted =
        match djls_format::format_template(source.as_str(), path, backend, format_options) {
            Ok(FormatOutcome::Changed(formatted)) => formatted,
            Ok(FormatOutcome::Unchanged | FormatOutcome::Ignored) => return Vec::new(),
            Err(error) => {
                tracing::debug!("Formatting failed for {path}: {error}");
                return Vec::new();
            }
        };

    let (line, character) = file.end_line_col(db, encoding).into();
    vec![ls_types::TextEdit::new(
        ls_types::Range::new(
            ls_types::Position::new(0, 0),
            ls_types::Position::new(line, character),
        ),
        formatted,
    )]
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_conf::FormatBackend;
    use djls_source::Db as _;
    use djls_source::FileSystem;
    use djls_source::InMemoryFileSystem;
    use djls_source::PositionEncoding;
    use djls_source::SourceFiles;
    use tower_lsp_server::ls_types;

    use super::format_document;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        fs: InMemoryFileSystem,
    }

    impl TestDb {
        fn new(source: impl Into<String>) -> Self {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("template.html".into(), source.into());
            Self {
                storage: salsa::Storage::new(None),
                files: SourceFiles::default(),
                fs,
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

        fn file_system(&self) -> &dyn FileSystem {
            &self.fs
        }
    }

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
        let db = TestDb::new(source);
        let file = db.get_or_create_file(Utf8Path::new("template.html"));
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
}
