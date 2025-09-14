//! Diagnostic collection and conversion for IDE features

use djls_templates::ast::Span;

/// Internal diagnostic representation (no LSP types)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeDiagnostic {
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub span: Span,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Collect all diagnostics for a file from parsing and semantic analysis
///
/// This assumes the relevant analysis functions have already been called
/// to accumulate diagnostics. This function merely collects them.
pub fn collect_diagnostics<DB>(db: &DB, file: djls_workspace::SourceFile) -> Vec<IdeDiagnostic>
where
    DB: djls_templates::Db + djls_semantic::SemanticDb,
{
    let mut diagnostics = vec![];

    let syntax_diagnostics = djls_templates::analyze_template::accumulated::<
        djls_templates::SyntaxDiagnosticAccumulator,
    >(db, file);

    for syntax_diag in syntax_diagnostics {
        diagnostics.push(IdeDiagnostic {
            message: syntax_diag.0.message.clone(),
            severity: DiagnosticSeverity::Error, // All syntax errors are errors
            span: syntax_diag.0.span,
            code: syntax_diag.0.code.to_string(),
        });
    }

    // TODO: Collect semantic diagnostics when semantic validation is implemented
    // This would look similar:
    // let semantic_diagnostics = djls_semantic::validate::accumulated::<
    //     djls_semantic::SemanticDiagnosticAccumulator
    // >(db, file);
    //
    // for semantic_diag in semantic_diagnostics {
    //     diagnostics.push(IdeDiagnostic {
    //         message: semantic_diag.0.message,
    //         severity: match semantic_diag.0.severity { ... },
    //         span: semantic_diag.0.span,
    //         code: semantic_diag.0.code.to_string(),
    //     });
    // }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostic_types() {
        let diagnostic = IdeDiagnostic {
            message: "Test error".to_string(),
            severity: DiagnosticSeverity::Error,
            span: djls_templates::ast::Span::new(0, 10),
            code: "TEST001".to_string(),
        };

        assert_eq!(diagnostic.message, "Test error");
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span.start, 0);
        assert_eq!(diagnostic.span.length, 10);
        assert_eq!(diagnostic.code, "TEST001");
    }
}
