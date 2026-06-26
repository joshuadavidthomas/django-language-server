use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticSeverity;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::Span;
use tower_lsp_server::ls_types;

use crate::completions::CompletionCandidate;
use crate::completions::CompletionCandidateKind;
use crate::completions::CompletionEdit;
use crate::completions::CompletionInsertFormat;
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

trait OffsetExt {
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

    fn to_lsp_range_with_encoding(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::Range;
}

impl SpanExt for Span {
    fn to_lsp_range(&self, line_index: &LineIndex) -> ls_types::Range {
        let start = self.start_offset().to_lsp_position(line_index);
        let end = self.end_offset().to_lsp_position(line_index);
        ls_types::Range { start, end }
    }

    fn to_lsp_range_with_encoding(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::Range {
        let start = self
            .start_offset()
            .to_lsp_position_with_encoding(source, line_index, encoding);
        let end = self
            .end_offset()
            .to_lsp_position_with_encoding(source, line_index, encoding);
        ls_types::Range { start, end }
    }
}

pub(crate) trait CompletionCandidateExt {
    fn to_lsp_completion_item(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::CompletionItem;
}

impl CompletionCandidateExt for CompletionCandidate {
    fn to_lsp_completion_item(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::CompletionItem {
        let kind = if self.edit.insert_format == CompletionInsertFormat::Snippet {
            ls_types::CompletionItemKind::SNIPPET
        } else {
            self.kind.to_lsp_completion_kind()
        };

        ls_types::CompletionItem {
            label: self.label.clone(),
            kind: Some(kind),
            detail: self.detail.clone(),
            documentation: self
                .documentation
                .as_ref()
                .map(|documentation| ls_types::Documentation::String(documentation.clone())),
            text_edit: Some(
                self.edit
                    .to_lsp_completion_text_edit(source, line_index, encoding),
            ),
            insert_text_format: Some(self.edit.insert_format.to_lsp_insert_text_format()),
            filter_text: Some(self.label.clone()),
            sort_text: Some(format!("{:02}_{}", self.kind.rank(), self.label)),
            ..Default::default()
        }
    }
}

trait CompletionCandidateKindExt {
    fn to_lsp_completion_kind(self) -> ls_types::CompletionItemKind;
}

impl CompletionCandidateKindExt for CompletionCandidateKind {
    fn to_lsp_completion_kind(self) -> ls_types::CompletionItemKind {
        match self {
            CompletionCandidateKind::TagName
            | CompletionCandidateKind::EndTag
            | CompletionCandidateKind::TagArgumentLiteral => ls_types::CompletionItemKind::KEYWORD,
            CompletionCandidateKind::TagArgumentChoice => ls_types::CompletionItemKind::ENUM_MEMBER,
            CompletionCandidateKind::TagArgumentPlaceholder => {
                ls_types::CompletionItemKind::VARIABLE
            }
            CompletionCandidateKind::TagArgumentSnippet => ls_types::CompletionItemKind::SNIPPET,
            CompletionCandidateKind::LibraryName => ls_types::CompletionItemKind::MODULE,
            CompletionCandidateKind::LoadSymbol | CompletionCandidateKind::Filter => {
                ls_types::CompletionItemKind::FUNCTION
            }
        }
    }
}

trait CompletionEditExt {
    fn to_lsp_completion_text_edit(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::CompletionTextEdit;
}

impl CompletionEditExt for CompletionEdit {
    fn to_lsp_completion_text_edit(
        &self,
        source: &str,
        line_index: &LineIndex,
        encoding: PositionEncoding,
    ) -> ls_types::CompletionTextEdit {
        ls_types::CompletionTextEdit::Edit(ls_types::TextEdit::new(
            self.replacement_span
                .to_lsp_range_with_encoding(source, line_index, encoding),
            self.insert_text.clone(),
        ))
    }
}

trait CompletionInsertFormatExt {
    fn to_lsp_insert_text_format(self) -> ls_types::InsertTextFormat;
}

impl CompletionInsertFormatExt for CompletionInsertFormat {
    fn to_lsp_insert_text_format(self) -> ls_types::InsertTextFormat {
        match self {
            CompletionInsertFormat::PlainText => ls_types::InsertTextFormat::PLAIN_TEXT,
            CompletionInsertFormat::Snippet => ls_types::InsertTextFormat::SNIPPET,
        }
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

trait FoldingRangeKindExt {
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
    fn completion_candidate_converts_edit_range_with_position_encoding() {
        let source = "éstatic";
        let line_index = LineIndex::from(source);
        let candidate = CompletionCandidate {
            label: "static".to_string(),
            kind: CompletionCandidateKind::LibraryName,
            edit: CompletionEdit {
                replacement_span: Span::new(2, 2),
                insert_text: "static %}".to_string(),
                insert_format: CompletionInsertFormat::PlainText,
            },
            detail: Some("Django template library (django.templatetags.static)".to_string()),
            documentation: Some("Loads static files.".to_string()),
        };

        let item = candidate.to_lsp_completion_item(source, &line_index, PositionEncoding::Utf16);
        let Some(ls_types::CompletionTextEdit::Edit(edit)) = item.text_edit else {
            panic!("expected edit completion text edit");
        };

        assert_eq!(item.label, "static");
        assert_eq!(item.kind, Some(ls_types::CompletionItemKind::MODULE));
        assert_eq!(item.sort_text.as_deref(), Some("01_static"));
        assert_eq!(
            item.detail.as_deref(),
            Some("Django template library (django.templatetags.static)")
        );
        assert_eq!(
            item.documentation,
            Some(ls_types::Documentation::String(
                "Loads static files.".to_string(),
            ))
        );
        assert_eq!(edit.range.start.character, 1);
        assert_eq!(edit.range.end.character, 3);
        assert_eq!(edit.new_text, "static %}");
    }

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
