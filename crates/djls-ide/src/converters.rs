//! Convert internal IDE types to LSP types

use djls_templates::ast::LineOffsets;
use djls_templates::ast::Span;
use tower_lsp_server::lsp_types;

use crate::diagnostics::DiagnosticSeverity;
use crate::diagnostics::IdeDiagnostic;

/// Create `LineOffsets` from source text
#[must_use]
pub fn line_offsets_from_text(text: &str) -> LineOffsets {
    let mut offsets = LineOffsets::default();
    for (i, c) in text.char_indices() {
        if c == '\n' {
            offsets.add_line(u32::try_from(i + 1).unwrap_or(u32::MAX));
        } else if c == '\r' {
            // Handle CRLF
            offsets.add_line(u32::try_from(i + 1).unwrap_or(u32::MAX));
            if text.chars().nth(i + 1) == Some('\n') {
                // Skip the \n in CRLF
            }
        }
    }
    offsets
}

/// Convert internal diagnostic to LSP diagnostic
#[must_use]
pub fn ide_diagnostic_to_lsp(
    diagnostic: &IdeDiagnostic,
    line_offsets: &LineOffsets,
) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: span_to_lsp_range(&diagnostic.span, line_offsets),
        severity: Some(match diagnostic.severity {
            DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
            DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
            DiagnosticSeverity::Information => lsp_types::DiagnosticSeverity::INFORMATION,
            DiagnosticSeverity::Hint => lsp_types::DiagnosticSeverity::HINT,
        }),
        code: Some(lsp_types::NumberOrString::String(diagnostic.code.clone())),
        code_description: None,
        source: Some("django-language-server".to_string()),
        message: diagnostic.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert span to LSP range
#[must_use]
pub fn span_to_lsp_range(span: &Span, line_offsets: &LineOffsets) -> lsp_types::Range {
    let start_pos = line_offsets.position_to_line_col(span.start as usize);
    let end_pos = line_offsets.position_to_line_col((span.start + span.length) as usize);

    lsp_types::Range {
        start: lsp_types::Position {
            line: (start_pos.0 - 1) as u32, // LSP is 0-based
            character: (start_pos.1) as u32,
        },
        end: lsp_types::Position {
            line: (end_pos.0 - 1) as u32,
            character: (end_pos.1) as u32,
        },
    }
}
