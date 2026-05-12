use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticSeverity;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::folding::FoldKind;
use crate::folding::FoldSpan;

pub(crate) trait OffsetExt {
    fn to_lsp_position(&self, line_index: &LineIndex) -> ls_types::Position;
}

impl OffsetExt for Offset {
    fn to_lsp_position(&self, line_index: &LineIndex) -> ls_types::Position {
        let (line, character) = line_index.to_line_col(*self).into();
        ls_types::Position { line, character }
    }
}

pub(crate) trait SpanExt {
    fn to_lsp_range(&self, line_index: &LineIndex) -> ls_types::Range;
}

impl SpanExt for Span {
    fn to_lsp_range(&self, line_index: &LineIndex) -> ls_types::Range {
        let start = self.start_offset().to_lsp_position(line_index);
        let end = self.end_offset().to_lsp_position(line_index);
        ls_types::Range { start, end }
    }
}

pub(crate) trait Utf8PathExt {
    fn to_lsp_uri(&self) -> Option<ls_types::Uri>;
}

impl Utf8PathExt for Utf8Path {
    fn to_lsp_uri(&self) -> Option<ls_types::Uri> {
        ls_types::Uri::from_file_path(self.as_std_path())
    }
}

impl Utf8PathExt for Utf8PathBuf {
    fn to_lsp_uri(&self) -> Option<ls_types::Uri> {
        ls_types::Uri::from_file_path(self.as_std_path())
    }
}

pub(crate) trait FoldingRangeKindExt {
    fn to_lsp_kind(self) -> ls_types::FoldingRangeKind;
}

impl FoldingRangeKindExt for FoldKind {
    fn to_lsp_kind(self) -> ls_types::FoldingRangeKind {
        match self {
            FoldKind::Region => ls_types::FoldingRangeKind::Region,
            FoldKind::Comment => ls_types::FoldingRangeKind::Comment,
            FoldKind::Imports => ls_types::FoldingRangeKind::Imports,
        }
    }
}

pub(crate) trait FoldSpanExt {
    fn to_lsp_folding_range(self, line_index: &LineIndex) -> Option<ls_types::FoldingRange>;
}

impl FoldSpanExt for FoldSpan {
    fn to_lsp_folding_range(self, line_index: &LineIndex) -> Option<ls_types::FoldingRange> {
        let range = self.span.to_lsp_range(line_index);

        if range.start.line >= range.end.line {
            return None;
        }

        Some(ls_types::FoldingRange {
            start_line: range.start.line,
            start_character: Some(range.start.character),
            end_line: range.end.line,
            end_character: Some(range.end.character),
            kind: Some(self.kind.to_lsp_kind()),
            collapsed_text: None,
        })
    }
}

pub(crate) trait DiagnosticSeverityExt {
    fn to_lsp_severity(self) -> Option<ls_types::DiagnosticSeverity>;
}

impl DiagnosticSeverityExt for DiagnosticSeverity {
    fn to_lsp_severity(self) -> Option<ls_types::DiagnosticSeverity> {
        match self {
            DiagnosticSeverity::Off => None,
            DiagnosticSeverity::Error => Some(ls_types::DiagnosticSeverity::ERROR),
            DiagnosticSeverity::Warning => Some(ls_types::DiagnosticSeverity::WARNING),
            DiagnosticSeverity::Info => Some(ls_types::DiagnosticSeverity::INFORMATION),
            DiagnosticSeverity::Hint => Some(ls_types::DiagnosticSeverity::HINT),
        }
    }
}
