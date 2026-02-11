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
        self.renderer.render(report).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> DiagnosticRenderer {
        DiagnosticRenderer::plain()
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
            Span::new(0, 19),
            "this block tag is never closed",
        );
        let output = plain().render(&diag);

        assert!(output.contains("error[S100]"), "should have error header");
        assert!(
            output.contains("Unclosed tag: block"),
            "should have message"
        );
        assert!(
            output.contains("templates/page.html"),
            "should have file path"
        );
        assert!(
            output.contains("{% block content %}"),
            "should show source line"
        );
        assert!(
            output.contains("this block tag is never closed"),
            "should have label"
        );
        assert!(output.contains("^^^"), "should have underline carets");
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
            Span::new(36, 22),
            "closing tag says 'content'",
        )
        .annotation(Span::new(0, 19), "opening tag is 'sidebar'", false);

        let output = plain().render(&diag);

        assert!(output.contains("error[S103]"));
        assert!(output.contains("closing tag says 'content'"));
        assert!(output.contains("opening tag is 'sidebar'"));
        assert!(output.contains("{% block sidebar %}"));
        assert!(output.contains("{% endblock content %}"));
    }

    #[test]
    fn two_annotations_same_line() {
        let source =
            "{% if user.is_authenticated and and user.is_staff %}\n{% endif %}\n";

        let diag = Diagnostic::new(
            source,
            "templates/admin.html",
            "S114",
            "Invalid syntax in {% if %} expression",
            Severity::Error,
            Span::new(35, 3),
            "unexpected operator",
        )
        .annotation(Span::new(31, 3), "previous operator here", false);

        let output = plain().render(&diag);

        assert!(output.contains("error[S114]"));
        assert!(output.contains("unexpected operator"));
        assert!(output.contains("previous operator here"));
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
            Span::new(27, 17),
            "tag not loaded",
        )
        .note("add {% load crispy_forms_tags %} at the top of this template");

        let output = plain().render(&diag);

        assert!(output.contains("error[S109]"));
        assert!(output.contains("tag not loaded"));
        assert!(output.contains("note: add {% load crispy_forms_tags %}"));
    }

    #[test]
    fn warning_severity() {
        let source = "{% load i18n %}\n{% load i18n %}\n";

        let diag = Diagnostic::new(
            source,
            "templates/dupes.html",
            "W001",
            "Duplicate {% load i18n %}",
            Severity::Warning,
            Span::new(16, 15),
            "already loaded on line 1",
        );
        let output = plain().render(&diag);

        assert!(
            output.contains("warning[W001]"),
            "should use warning level"
        );
        assert!(output.contains("Duplicate {% load i18n %}"));
    }

    #[test]
    fn long_line_truncation() {
        let long_prefix = "x".repeat(200);
        let source =
            format!("<div class=\"{long_prefix}\">{{{{ value|bogus_filter }}}}</div>\n");
        let filter_start = source.find("bogus_filter").unwrap();

        let diag = Diagnostic::new(
            &source,
            "templates/long.html",
            "S111",
            "Unknown filter 'bogus_filter'",
            Severity::Error,
            Span::saturating_from_bounds_usize(filter_start, filter_start + 12),
            "not a built-in or loaded filter",
        );
        let output = plain().render(&diag);

        assert!(output.contains("error[S111]"));
        assert!(output.contains("bogus_filter"));
        assert!(
            output.contains("..."),
            "should truncate long line with ellipsis"
        );
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
            Span::new(0, 19),
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
            Span::new(0, 19),
            "never closed",
        );
        let output = plain().render(&diag);

        assert!(
            !output.contains("\x1b["),
            "plain output should not contain ANSI escape codes"
        );
    }
}
