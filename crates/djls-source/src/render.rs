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
pub(crate) struct DiagnosticAnnotation<'a> {
    span: Span,
    label: &'a str,
    primary: bool,
}

/// A diagnostic ready for rendering.
///
/// Collects all the pieces needed to produce formatted output, then renders
/// via `annotate-snippets`. Generic over any diagnostic type — callers extract
/// span/code/message from their error types and build this struct.
#[derive(Debug)]
pub struct Diagnostic<'a> {
    source: &'a str,
    path: &'a str,
    code: &'a str,
    message: &'a str,
    severity: Severity,
    annotations: Vec<DiagnosticAnnotation<'a>>,
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
            Severity::Hint => Level::HELP.with_name("hint"),
        };

        let source = diagnostic.source;
        let window =
            LineWindow::covering(source, diagnostic.annotations.iter().map(|ann| ann.span));

        let mut snippet = Snippet::source(window.text(source))
            .path(diagnostic.path)
            .line_start(window.first_line);

        for ann in &diagnostic.annotations {
            let start = window.offset(ann.span.start_usize());
            let end = start + ann.span.length_usize();
            let kind = if ann.primary {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            snippet = snippet.annotation(kind.span(start..end).label(ann.label));
        }

        let title = level
            .primary_title(diagnostic.message)
            .id(diagnostic.code)
            .element(snippet);

        let report = &[title];
        self.renderer.render(report)
    }
}

/// The whole lines of a source that cover a set of spans, plus one line before
/// them.
///
/// annotate-snippets scans every line of the source it is given, so rendering
/// only these lines keeps each render proportional to the diagnostic rather
/// than the file. The extra line before keeps the line-number gutter as wide as
/// it would be for the whole source: annotate-snippets sizes the gutter from
/// the lines before the last annotation's end, and an annotation ending at the
/// window start would otherwise count one line fewer.
///
/// `first_line` counts `\n` before the window rather than coming from
/// [`LineIndex`](crate::LineIndex), because annotate-snippets numbers lines by
/// `\n` only while `LineIndex` also breaks lines at a lone `\r`.
struct LineWindow {
    start: usize,
    end: usize,
    first_line: usize,
}

impl LineWindow {
    fn covering(source: &str, spans: impl Iterator<Item = Span>) -> Self {
        let (first, last) = spans
            .fold(None, |bounds, span| {
                let (start, end) = (span.start_usize(), span.end_usize());
                Some(
                    bounds.map_or((start, end), |(first, last): (usize, usize)| {
                        (first.min(start), last.max(end))
                    }),
                )
            })
            .unwrap_or((0, 0));
        let first = first.min(source.len());
        let last = last.min(source.len());

        let bytes = source.as_bytes();
        let line_start_before =
            |offset: usize| memchr::memrchr(b'\n', &bytes[..offset]).map_or(0, |index| index + 1);
        let mut start = line_start_before(first);
        if start > 0 {
            start = line_start_before(start - 1);
        }
        let end =
            memchr::memchr(b'\n', &bytes[last..]).map_or(source.len(), |index| last + index + 1);

        Self {
            start,
            end,
            first_line: 1 + memchr::memchr_iter(b'\n', &bytes[..start]).count(),
        }
    }

    fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    fn offset(&self, absolute: usize) -> usize {
        absolute - self.start
    }
}

#[cfg(test)]
mod tests {
    //! Renderer capability tests.
    //!
    //! These exercise the rendering engine itself — multi-span annotations,
    //! severity levels, long-line truncation, etc. The diagnostic
    //! scenarios are synthetic; real validation output (produced by
    //! `djls-semantic`) currently uses single-span diagnostics with empty
    //! labels. See `djls-semantic/src/testing.rs` for how actual errors are
    //! rendered.

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
        let second_and = source
            .find("and user")
            .expect("test source should contain the second and operator");
        let first_and = source[..second_and]
            .rfind("and")
            .expect("test source should contain the first and operator");
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
    fn warning_severity() {
        let source = "{% load i18n %}\n{% load i18n %}\n";
        let second_load = source[1..]
            .find("{% load i18n %}")
            .expect("test source should contain a second load tag")
            + 1;
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

    #[test]
    fn rendering_only_annotated_lines_matches_whole_source() {
        let source = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11 é\r\n12\n";
        let line_10 = source
            .find("10")
            .expect("test source should contain line 10");
        let line_12 = source
            .find("12")
            .expect("test source should contain line 12");
        for (start, end) in [
            (0, 1),
            (line_10, line_10),
            (line_10, line_12 + 2),
            (source.len(), source.len()),
        ] {
            let diag = Diagnostic::new(
                source,
                "t.html",
                "S100",
                "msg",
                Severity::Error,
                Span::saturating_from_bounds_usize(start, end),
                "label",
            );
            let whole = Snippet::source(source)
                .path("t.html")
                .line_start(1)
                .annotation(AnnotationKind::Primary.span(start..end).label("label"));
            let expected = Renderer::plain()
                .render(&[Level::ERROR.primary_title("msg").id("S100").element(whole)]);
            assert_eq!(plain().render(&diag), expected, "span {start}..{end}");
        }
    }
}
