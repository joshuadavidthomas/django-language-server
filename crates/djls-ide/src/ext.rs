use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::Span;
use tower_lsp_server::lsp_types;
use tower_lsp_server::UriExt;

pub(crate) trait OffsetExt {
    fn to_lsp_position(&self, line_index: &LineIndex) -> lsp_types::Position;
}

impl OffsetExt for Offset {
    fn to_lsp_position(&self, line_index: &LineIndex) -> lsp_types::Position {
        let (line, character) = line_index.to_line_col(*self).into();
        lsp_types::Position { line, character }
    }
}

pub(crate) trait SpanExt {
    fn to_lsp_range(&self, line_index: &LineIndex) -> lsp_types::Range;
}

impl SpanExt for Span {
    fn to_lsp_range(&self, line_index: &LineIndex) -> lsp_types::Range {
        let start = self.start_offset().to_lsp_position(line_index);
        let end = self.end_offset().to_lsp_position(line_index);
        lsp_types::Range { start, end }
    }
}

pub(crate) trait Utf8PathExt {
    fn to_lsp_uri(&self) -> Option<lsp_types::Uri>;
}

impl Utf8PathExt for Utf8Path {
    fn to_lsp_uri(&self) -> Option<lsp_types::Uri> {
        lsp_types::Uri::from_file_path(self.as_std_path())
    }
}

impl Utf8PathExt for Utf8PathBuf {
    fn to_lsp_uri(&self) -> Option<lsp_types::Uri> {
        lsp_types::Uri::from_file_path(self.as_std_path())
    }
}
