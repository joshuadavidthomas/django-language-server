use annotate_snippets::AnnotationKind;
use annotate_snippets::Level;
use annotate_snippets::Renderer;
use annotate_snippets::Snippet;

use crate::Span;

/// Severity level for rendered diagnostics.
///
/// This is deliberately separate from both `djls_conf::DiagnosticSeverity` and
/// LSP severity types — the renderer only needs to know what label to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A single annotation to render on a source snippet.
///
/// Each annotation highlights a span of source text with a label message.
/// The `primary` flag controls whether it gets `^^^` (primary) or `---` (context)
/// underline treatment.
#[derive(Debug, Clone)]
pub struct DiagnosticAnnotation<'a> {
    pub span: Span,
    pub label: &'a str,
    pub primary: bool,
}

/// A diagnostic ready for rendering.
///
/// Collects all the pieces needed to produce formatted output, then renders
/// via `annotate-snippets`. Generic over any diagnostic type — callers extract
/// span/code/message from their error types and build this struct.
#[derive(Debug)]
pub struct Diagnostic<'a> {
    pub source: &'a str,
    pub path: &'a str,
    pub code: &'a str,
    pub message: &'a str,
    pub severity: Severity,
    pub annotations: Vec<DiagnosticAnnotation<'a>>,
    pub notes: Vec<&'a str>,
}

impl<'a> Diagnostic<'a> {
    /// Create a diagnostic with a single primary annotation.
    ///
    /// This is the common case — one error pointing at one span.
    #[must_use]
    pub fn new(
        source: &'a str,
        path: &'a str,
        code: &'a str,
        message: &'a str,
        severity: Severity,
        span: Span,
        label: &'a str,
    ) -> Self {
        Self {
            source,
            path,
            code,
            message,
            severity,
            annotations: vec![DiagnosticAnnotation {
                span,
                label,
                primary: true,
            }],
            notes: Vec::new(),
        }
    }

    /// Add an additional annotation to this diagnostic.
    #[must_use]
    pub fn annotation(mut self, span: Span, label: &'a str, primary: bool) -> Self {
        self.annotations.push(DiagnosticAnnotation {
            span,
            label,
            primary,
        });
        self
    }

    /// Add a note to this diagnostic.
    #[must_use]
    pub fn note(mut self, note: &'a str) -> Self {
        self.notes.push(note);
        self
    }
}

/// Renders diagnostics as formatted text using `annotate-snippets`.
///
/// Supports two modes:
/// - **Plain**: No ANSI colors — use for snapshot tests and piped output
/// - **Styled**: ANSI colors and bold — use for terminal display
#[derive(Debug)]
pub struct DiagnosticRenderer {
    renderer: Renderer,
}

impl DiagnosticRenderer {
    /// Create a renderer that produces plain text (no ANSI colors).
    ///
    /// Use for snapshot tests and non-terminal output.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            renderer: Renderer::plain(),
        }
    }

    /// Create a renderer that produces styled output with ANSI colors.
    ///
    /// Use for terminal display.
    #[must_use]
    pub fn styled() -> Self {
        Self {
            renderer: Renderer::styled(),
        }
    }

    /// Render a diagnostic to a string.
    #[must_use]
    pub fn render(&self, diagnostic: &Diagnostic<'_>) -> String {
        let level = match diagnostic.severity {
            Severity::Error => Level::ERROR,
            Severity::Warning => Level::WARNING,
            Severity::Info => Level::INFO,
            Severity::Hint => Level::HELP,
        };

        let mut snippet = Snippet::source(diagnostic.source)
            .path(diagnostic.path)
            .line_start(1);

        for ann in &diagnostic.annotations {
            let start = ann.span.start_usize();
            let end = start + ann.span.length_usize();
            let kind = if ann.primary {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            snippet = snippet.annotation(kind.span(start..end).label(ann.label));
        }

        let mut title = level
            .primary_title(diagnostic.message)
            .id(diagnostic.code)
            .element(snippet);

        for note in &diagnostic.notes {
            title = title.element(Level::NOTE.message(*note));
        }

        let report = &[title];
        self.renderer.render(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> DiagnosticRenderer {
        DiagnosticRenderer::plain()
    }

    fn span_of(source: &str, needle: &str) -> Span {
        let start = source.find(needle).expect("needle not found in source");
        Span::saturating_from_bounds_usize(start, start + needle.len())
    }

    #[test]
    fn single_line_span() {
        let source = "{% block content %}\n<p>Hello</p>\n{% endblock %}\n";
        let diag = Diagnostic::new(
            source,
            "templates/page.html",
            "S100",
            "Unclosed tag: block",
            Severity::Error,
            span_of(source, "{% block content %}"),
            "this block tag is never closed",
        );
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn two_annotations_different_lines() {
        let source = "{% block sidebar %}\n<nav>Links</nav>\n{% endblock content %}\n";
        let diag = Diagnostic::new(
            source,
            "templates/layout.html",
            "S103",
            "'content' does not match 'sidebar'",
            Severity::Error,
            span_of(source, "{% endblock content %}"),
            "closing tag says 'content'",
        )
        .annotation(
            span_of(source, "{% block sidebar %}"),
            "opening tag is 'sidebar'",
            false,
        );
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn two_annotations_same_line() {
        let source = "{% if user.is_authenticated and and user.is_staff %}\n{% endif %}\n";
        let second_and = source.find("and user").unwrap();
        let first_and = source[..second_and].rfind("and").unwrap();
        let diag = Diagnostic::new(
            source,
            "templates/admin.html",
            "S114",
            "Invalid syntax in {% if %} expression",
            Severity::Error,
            Span::saturating_from_bounds_usize(second_and, second_and + 3),
            "unexpected operator",
        )
        .annotation(
            Span::saturating_from_bounds_usize(first_and, first_and + 3),
            "previous operator here",
            false,
        );
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn with_note() {
        let source = "<p>{{ value|intcomma }}</p>\n{% crispy form %}\n";
        let diag = Diagnostic::new(
            source,
            "templates/form.html",
            "S109",
            "Tag 'crispy' requires {% load crispy_forms_tags %}",
            Severity::Error,
            span_of(source, "{% crispy form %}"),
            "tag not loaded",
        )
        .note("add {% load crispy_forms_tags %} at the top of this template");
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn warning_severity() {
        let source = "{% load i18n %}\n{% load i18n %}\n";
        let second_load = source[1..].find("{% load i18n %}").unwrap() + 1;
        let diag = Diagnostic::new(
            source,
            "templates/dupes.html",
            "W001",
            "Duplicate {% load i18n %}",
            Severity::Warning,
            Span::saturating_from_bounds_usize(second_load, second_load + 15),
            "already loaded on line 1",
        );
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn long_line_truncation() {
        let long_prefix = "x".repeat(200);
        let source = format!("<div class=\"{long_prefix}\">{{{{ value|bogus_filter }}}}</div>\n");
        let diag = Diagnostic::new(
            &source,
            "templates/long.html",
            "S111",
            "Unknown filter 'bogus_filter'",
            Severity::Error,
            span_of(&source, "bogus_filter"),
            "not a built-in or loaded filter",
        );
        insta::assert_snapshot!(plain().render(&diag));
    }

    #[test]
    fn styled_produces_ansi() {
        let source = "{% block content %}\n";
        let renderer = DiagnosticRenderer::styled();
        let diag = Diagnostic::new(
            source,
            "test.html",
            "S100",
            "Unclosed tag",
            Severity::Error,
            span_of(source, "{% block content %}"),
            "never closed",
        );
        let output = renderer.render(&diag);
        assert!(
            output.contains("\x1b["),
            "styled output should contain ANSI escape codes"
        );
    }

    #[test]
    fn plain_no_ansi() {
        let source = "{% block content %}\n";
        let diag = Diagnostic::new(
            source,
            "test.html",
            "S100",
            "Unclosed tag",
            Severity::Error,
            span_of(source, "{% block content %}"),
            "never closed",
        );
        let output = plain().render(&diag);
        assert!(
            !output.contains("\x1b["),
            "plain output should not contain ANSI escape codes"
        );
    }
}
