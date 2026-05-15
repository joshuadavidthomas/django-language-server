use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticSeverity;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::folding::FoldKind;
use crate::folding::FoldSpan;

pub(crate) trait TemplateOutlineExt {
    fn to_lsp_document_symbols(&self, line_index: &LineIndex) -> Vec<ls_types::DocumentSymbol>;
}

impl TemplateOutlineExt for djls_semantic::TemplateOutline {
    fn to_lsp_document_symbols(&self, line_index: &LineIndex) -> Vec<ls_types::DocumentSymbol> {
        self.items
            .iter()
            .map(|item| item.to_lsp_document_symbol(line_index))
            .collect()
    }
}

pub(crate) trait OutlineItemExt {
    fn to_lsp_document_symbol(&self, line_index: &LineIndex) -> ls_types::DocumentSymbol;
}

impl OutlineItemExt for djls_semantic::OutlineItem {
    fn to_lsp_document_symbol(&self, line_index: &LineIndex) -> ls_types::DocumentSymbol {
        let children = (!self.children.is_empty()).then(|| {
            self.children
                .iter()
                .map(|child| child.to_lsp_document_symbol(line_index))
                .collect()
        });

        ls_types::DocumentSymbol {
            name: self.label.clone(),
            detail: self.detail.clone(),
            kind: self.kind.to_lsp_symbol_kind(),
            tags: None,
            // `deprecated` is itself deprecated by LSP 3.15 in favor of `tags`, but
            // `ls_types::DocumentSymbol` still includes the field for wire compatibility.
            // We set both to `None` because template outline items are not deprecated.
            #[allow(deprecated)]
            deprecated: None,
            range: self.span.to_lsp_range(line_index),
            selection_range: self.selection_span.to_lsp_range(line_index),
            children,
        }
    }
}

pub(crate) trait OutlineKindExt {
    fn to_lsp_symbol_kind(self) -> ls_types::SymbolKind;
}

impl OutlineKindExt for djls_semantic::OutlineKind {
    fn to_lsp_symbol_kind(self) -> ls_types::SymbolKind {
        match self {
            djls_semantic::OutlineKind::NamedRegion => ls_types::SymbolKind::NAMESPACE,
            djls_semantic::OutlineKind::ControlFlow => ls_types::SymbolKind::OPERATOR,
            djls_semantic::OutlineKind::TemplateReference
            | djls_semantic::OutlineKind::FileReference => ls_types::SymbolKind::FILE,
            djls_semantic::OutlineKind::LibraryImport => ls_types::SymbolKind::MODULE,
            djls_semantic::OutlineKind::Callable
            | djls_semantic::OutlineKind::RouteReference
            | djls_semantic::OutlineKind::Filter => ls_types::SymbolKind::FUNCTION,
            djls_semantic::OutlineKind::Variable => ls_types::SymbolKind::VARIABLE,
        }
    }
}

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
