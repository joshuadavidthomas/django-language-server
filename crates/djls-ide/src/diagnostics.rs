use djls_semantic::ValidationError;
use djls_templates::LineOffsets;
use djls_templates::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use djls_workspace::SourceFile;
use tower_lsp_server::lsp_types;

/// Convert a Span to an LSP Range using line offsets.
fn span_to_lsp_range(span: Span, line_offsets: &LineOffsets) -> lsp_types::Range {
    let start_pos = span.start as usize;
    let end_pos = (span.start + span.length) as usize;

    let (start_line, start_char) = line_offsets.position_to_line_col(start_pos);
    let (end_line, end_char) = line_offsets.position_to_line_col(end_pos);

    lsp_types::Range {
        start: lsp_types::Position {
            line: u32::try_from(start_line - 1).unwrap_or(u32::MAX), // LSP is 0-based, LineOffsets is 1-based
            character: u32::try_from(start_char).unwrap_or(u32::MAX),
        },
        end: lsp_types::Position {
            line: u32::try_from(end_line - 1).unwrap_or(u32::MAX),
            character: u32::try_from(end_char).unwrap_or(u32::MAX),
        },
    }
}

/// Convert a template error to an LSP diagnostic.
fn template_error_to_diagnostic(
    error: &TemplateError,
    line_offsets: &LineOffsets,
) -> lsp_types::Diagnostic {
    let range = error
        .span()
        .map(|(start, length)| {
            let span = Span::new(start, length);
            span_to_lsp_range(span, line_offsets)
        })
        .unwrap_or_default();

    lsp_types::Diagnostic {
        range,
        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
        code: Some(lsp_types::NumberOrString::String(
            error.diagnostic_code().to_string(),
        )),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: error.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert a validation error (`ValidationError`) to an LSP diagnostic.
fn validation_error_to_diagnostic(
    error: &ValidationError,
    line_offsets: &LineOffsets,
) -> lsp_types::Diagnostic {
    let range = error
        .span()
        .map(|(start, length)| {
            let span = Span::new(start, length);
            span_to_lsp_range(span, line_offsets)
        })
        .unwrap_or_default();

    lsp_types::Diagnostic {
        range,
        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
        code: Some(lsp_types::NumberOrString::String(
            error.diagnostic_code().to_string(),
        )),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: error.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Collect all diagnostics (syntax and semantic) for a template file.
///
/// This function:
/// 1. Parses the template file
/// 2. If parsing succeeds, runs semantic validation
/// 3. Collects syntax diagnostics from the parser
/// 4. Collects semantic diagnostics from the validator
/// 5. Converts errors to LSP diagnostic types
#[must_use]
pub fn collect_diagnostics(
    db: &dyn djls_semantic::Db,
    file: SourceFile,
) -> Vec<lsp_types::Diagnostic> {
    let mut diagnostics = Vec::new();

    // Parse and get template errors
    let nodelist = djls_templates::parse_template(db, file);
    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    // Get line offsets for conversion
    let line_offsets = if let Some(ref nl) = nodelist {
        nl.line_offsets(db).clone()
    } else {
        LineOffsets::default()
    };

    // Convert template errors to diagnostics
    for error_acc in template_errors {
        diagnostics.push(template_error_to_diagnostic(&error_acc.0, &line_offsets));
    }

    // If parsing succeeded, run validation
    if let Some(nodelist) = nodelist {
        djls_semantic::validate_nodelist(db, nodelist);
        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nodelist);

        // Convert validation errors to diagnostics
        for error_acc in validation_errors {
            diagnostics.push(validation_error_to_diagnostic(&error_acc.0, &line_offsets));
        }
    }

    diagnostics
}
