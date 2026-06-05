use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticSeverity;
use djls_semantic::TemplateSymbol;
use djls_semantic::TemplateSymbolKind;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::folding::FoldKind;
use crate::folding::FoldSpan;

pub(crate) trait OutlineKindExt {
    fn to_lsp_symbol_kind(self) -> ls_types::SymbolKind;
}

impl OutlineKindExt for djls_semantic::OutlineKind {
    fn to_lsp_symbol_kind(self) -> ls_types::SymbolKind {
        match self {
            djls_semantic::OutlineKind::TemplateBlock => ls_types::SymbolKind::NAMESPACE,
            djls_semantic::OutlineKind::ControlTag => ls_types::SymbolKind::OPERATOR,
            djls_semantic::OutlineKind::TemplateReference
            | djls_semantic::OutlineKind::StaticAssetReference => ls_types::SymbolKind::FILE,
            djls_semantic::OutlineKind::TemplateLibrary => ls_types::SymbolKind::MODULE,
            djls_semantic::OutlineKind::TemplateLibrarySymbol
            | djls_semantic::OutlineKind::TemplateTag
            | djls_semantic::OutlineKind::RouteReference
            | djls_semantic::OutlineKind::Filter => ls_types::SymbolKind::FUNCTION,
            djls_semantic::OutlineKind::Variable => ls_types::SymbolKind::VARIABLE,
        }
    }
}

pub(crate) trait TemplateSymbolExt {
    fn to_lsp_completion_kind(&self) -> ls_types::CompletionItemKind;
}

impl TemplateSymbolExt for TemplateSymbol {
    fn to_lsp_completion_kind(&self) -> ls_types::CompletionItemKind {
        match self.kind {
            TemplateSymbolKind::Tag => ls_types::CompletionItemKind::KEYWORD,
            TemplateSymbolKind::Filter => ls_types::CompletionItemKind::FUNCTION,
        }
    }
}

pub(crate) trait OffsetExt {
    fn to_lsp_position(&self, line_index: &LineIndex) -> ls_types::Position;

    fn to_lsp_position_with_encoding(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::Position;
}

impl OffsetExt for Offset {
    fn to_lsp_position(&self, line_index: &LineIndex) -> ls_types::Position {
        let (line, character) = line_index.to_line_col(*self).into();
        ls_types::Position { line, character }
    }

    fn to_lsp_position_with_encoding(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::Position {
        let Some(source_line) = line_index.line_at_offset(source, *self) else {
            return self.to_lsp_position(line_index);
        };
        let byte_offset = source_line.byte_offset(*self);
        let line_prefix = &source_line.text()[..byte_offset];
        let character = match encoding {
            PositionEncoding::Utf8 => u32::try_from(line_prefix.len()).unwrap_or(u32::MAX),
            PositionEncoding::Utf16 => line_prefix
                .chars()
                .map(|character| u32::try_from(character.len_utf16()).unwrap_or_default())
                .sum(),
            PositionEncoding::Utf32 => {
                u32::try_from(line_prefix.chars().count()).unwrap_or(u32::MAX)
            }
        };

        ls_types::Position {
            line: source_line.line(),
            character,
        }
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
            start_character: None,
            end_line: range.end.line,
            end_character: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_span_converts_to_line_folding_range() {
        let source = "  {% if items %}\n    body\n  {% endif %}\n";
        let line_index = LineIndex::from(source);
        let fold = FoldSpan {
            span: Span::saturating_from_bounds_usize(2, 39),
            kind: FoldKind::Region,
        };

        let range = fold.to_lsp_folding_range(&line_index).unwrap();

        assert_eq!(range.start_line, 0);
        assert_eq!(range.start_character, None);
        assert_eq!(range.end_line, 2);
        assert_eq!(range.end_character, None);
        assert_eq!(range.kind, Some(ls_types::FoldingRangeKind::Region));
    }

    #[test]
    fn fold_span_ignores_single_line_ranges() {
        let source = "{% for item in items %}<li>{{ item }}</li>{% endfor %}";
        let line_index = LineIndex::from(source);
        let fold = FoldSpan {
            span: Span::saturating_from_bounds_usize(0, source.len()),
            kind: FoldKind::Region,
        };

        assert_eq!(fold.to_lsp_folding_range(&line_index), None);
    }
}
