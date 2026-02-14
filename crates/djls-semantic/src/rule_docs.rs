use djls_source::Span;
use djls_templates::TemplateError;

use crate::ValidationError;

/// Metadata for a single diagnostic rule, used to generate documentation.
pub struct RuleDoc {
    pub code: &'static str,
    pub name: &'static str,
    pub what: &'static str,
    pub why: Option<&'static str>,
    pub fix: Option<&'static str>,
    pub example: Option<&'static str>,
    pub requires_inspector: bool,
}

/// Returns documentation metadata for all diagnostic rules (template + validation),
/// in code order.
#[must_use]
pub fn all_rule_docs() -> Vec<RuleDoc> {
    let mut docs = template_error_docs();
    docs.extend(validation_error_docs());
    docs
}

fn template_rule(error: &TemplateError, what: &'static str, fix: Option<&'static str>) -> RuleDoc {
    RuleDoc {
        code: error.code(),
        name: error.name(),
        what,
        why: None,
        fix,
        example: None,
        requires_inspector: false,
    }
}

fn template_error_docs() -> Vec<RuleDoc> {
    vec![
        template_rule(
            &TemplateError::Parser(String::new()),
            "Reports syntax errors in Django template markup — unclosed tags, \
             malformed expressions, or invalid template syntax that prevents parsing.",
            Some(
                "Correct the template syntax. Common causes include unclosed `{{` or \
                 `{%` delimiters, unmatched quotes in tag arguments, and malformed \
                 filter expressions.",
            ),
        ),
        template_rule(
            &TemplateError::Io(String::new()),
            "Reports file I/O errors encountered when reading a template file.",
            Some(
                "Check that the file exists, is readable, and is not locked by \
                 another process.",
            ),
        ),
        template_rule(
            &TemplateError::Config(String::new()),
            "Reports configuration errors that prevent template processing.",
            Some(
                "Check your djls configuration. See \
                 [Configuration](./configuration/index.md) for details.",
            ),
        ),
    ]
}

#[derive(Clone, Copy)]
struct ValidationDoc {
    what: &'static str,
    why: Option<&'static str>,
    fix: Option<&'static str>,
    example: Option<&'static str>,
    requires_inspector: bool,
}

fn validation_rule(error: &ValidationError, doc: ValidationDoc) -> RuleDoc {
    RuleDoc {
        code: error.code(),
        name: error.name(),
        what: doc.what,
        why: doc.why,
        fix: doc.fix,
        example: doc.example,
        requires_inspector: doc.requires_inspector,
    }
}

fn dummy_span() -> Span {
    Span::new(0, 0)
}

#[allow(clippy::too_many_lines)]
fn validation_error_docs() -> Vec<RuleDoc> {
    vec![
        validation_rule(
            &ValidationError::UnclosedTag {
                tag: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that block tags have their required closing tag. For \
                       example, `{% block %}` must have a matching `{% endblock %}`, and \
                       `{% if %}` must have `{% endif %}`.",
                why: Some(
                    "Django raises `TemplateSyntaxError` at render time for unclosed \
                     block tags.",
                ),
                fix: None,
                example: Some("{% block content %}\n  <h1>Hello</h1>"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::UnbalancedStructure {
                opening_tag: String::new(),
                expected_closing: String::new(),
                opening_span: dummy_span(),
                closing_span: None,
            },
            ValidationDoc {
                what: "Checks that block tags are properly matched. Detects cases where \
                       a closing tag doesn't correspond to its opening tag — for example, \
                       `{% if %}` closed by `{% endfor %}`.",
                why: Some("Django raises `TemplateSyntaxError` for mismatched block tags."),
                fix: None,
                example: Some("{% if user.is_authenticated %}\n  <p>Welcome</p>\n{% endfor %}"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::OrphanedTag {
                tag: String::new(),
                context: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that intermediate tags (like `{% else %}`, `{% elif %}`, \
                       `{% empty %}`) appear inside their expected parent block.",
                why: Some(
                    "Django raises `TemplateSyntaxError` for intermediate tags that \
                     appear outside their parent structure.",
                ),
                fix: None,
                example: Some("{% else %}\n  <p>Fallback</p>"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::UnmatchedBlockName {
                expected: String::new(),
                got: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that named `{% endblock %}` tags match their opening \
                       `{% block %}` name. Django allows `{% endblock name %}` as \
                       documentation — this rule verifies the name is correct.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when the endblock name doesn't \
                     match. This usually indicates a copy-paste error or a refactoring \
                     that renamed one block but not the other.",
                ),
                fix: None,
                example: Some("{% block sidebar %}\n  <nav>...</nav>\n{% endblock content %}"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::UnknownTag {
                tag: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that a template tag exists in at least one known library. \
                       If the tag isn't found in any installed package or built-in \
                       library, it's reported as unknown.",
                why: Some("Django raises `TemplateSyntaxError` for tags it doesn't recognize."),
                fix: Some(
                    "Check the tag name for typos. If the tag comes from a third-party \
                     package, install it with `pip install <package>`.\n\n\
                     See [Three-Layer Resolution]\
                     (./template-validation.md#three-layer-resolution) \
                     for how djls determines tag availability.",
                ),
                example: Some("{% nonexistent_tag %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::UnloadedTag {
                tag: String::new(),
                library: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that a template tag's library has been loaded with \
                       `{% load %}`. The tag exists and is available — it just hasn't \
                       been imported into this template.",
                why: Some(
                    "Django raises `TemplateSyntaxError` because the tag isn't registered \
                     in the template's tag namespace without a `{% load %}`.",
                ),
                fix: None,
                example: Some("{% cache 500 sidebar %}\n  ...\n{% endcache %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::AmbiguousUnloadedTag {
                tag: String::new(),
                libraries: Vec::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks for tags that exist in multiple libraries but none have \
                       been loaded. djls can't determine which library you intended.",
                why: Some(
                    "Without a `{% load %}`, Django doesn't know which library to use, \
                     and loading the wrong one could give unexpected behavior.",
                ),
                fix: None,
                example: Some(
                    "{# 'my_tag' exists in both 'library_a' and 'library_b' #}\n\
                     {% my_tag %}",
                ),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::UnknownFilter {
                filter: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that a template filter exists in at least one known \
                       library. If the filter isn't found in any installed package or \
                       built-in library, it's reported as unknown.",
                why: Some(
                    "Django raises `TemplateSyntaxError` for filters it doesn't \
                     recognize.",
                ),
                fix: Some(
                    "Check the filter name for typos. If the filter comes from a \
                     third-party package, install it with `pip install <package>`.\n\n\
                     See [Three-Layer Resolution]\
                     (./template-validation.md#three-layer-resolution) \
                     for how djls determines filter availability.",
                ),
                example: Some("{{ value|nonexistent_filter }}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::UnloadedFilter {
                filter: String::new(),
                library: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that a template filter's library has been loaded with \
                       `{% load %}`. The filter exists and is available — it just hasn't \
                       been imported into this template.",
                why: Some(
                    "Django raises `TemplateSyntaxError` because the filter isn't \
                     registered in the template's filter namespace without a \
                     `{% load %}`.",
                ),
                fix: None,
                example: Some("{{ value|intcomma }}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::AmbiguousUnloadedFilter {
                filter: String::new(),
                libraries: Vec::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks for filters that exist in multiple libraries but none have \
                       been loaded. djls can't determine which library you intended.",
                why: Some(
                    "Without a `{% load %}`, Django doesn't know which library to use, \
                     and loading the wrong one could give unexpected behavior.",
                ),
                fix: None,
                example: Some(
                    "{# 'my_filter' exists in both 'library_a' and 'library_b' #}\n\
                     {{ value|my_filter }}",
                ),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::ExpressionSyntaxError {
                tag: String::new(),
                message: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Validates operator usage in `{% if %}` and `{% elif %}` \
                       expressions. Catches missing operands, consecutive operators, and \
                       other syntax errors in boolean expressions.",
                why: Some(
                    "Django raises `TemplateSyntaxError` for malformed `{% if %}` \
                     expressions.",
                ),
                fix: None,
                example: Some("{% if and x %}\n  <p>Hello</p>\n{% endif %}"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::FilterMissingArgument {
                filter: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that filters which require an argument are called with \
                       one. Filter arity is determined by static analysis of the \
                       filter's Python source code.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when a filter that requires an \
                     argument is called without one.",
                ),
                fix: None,
                example: Some("{{ value|default }}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::FilterUnexpectedArgument {
                filter: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that filters which don't accept an argument aren't called \
                       with one. Filter arity is determined by static analysis of the \
                       filter's Python source code.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when a filter that doesn't \
                     accept arguments is called with one.",
                ),
                fix: None,
                example: Some("{{ value|title:\"arg\" }}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::ExtractedRuleViolation {
                tag: String::new(),
                message: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Validates tag arguments against rules extracted from the tag's \
                       Python source code. The extraction engine reads \
                       `split_contents()` guard conditions, function signatures, and \
                       keyword position checks directly from tag implementations.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when a tag is called with the \
                     wrong number or kind of arguments.",
                ),
                fix: None,
                example: Some("{% for item %}\n  {{ item }}\n{% endfor %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::TagNotInInstalledApps {
                tag: String::new(),
                app: String::new(),
                load_name: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks for tags that exist in an installed Python package but \
                       whose Django app isn't in `INSTALLED_APPS`. The package is \
                       installed — the app just needs to be activated.",
                why: Some(
                    "Django won't discover the template tag library unless the app is \
                     listed in `INSTALLED_APPS`.",
                ),
                fix: Some(
                    "Add the app to `INSTALLED_APPS` in your Django settings.\n\n\
                     See [Three-Layer Resolution]\
                     (./template-validation.md#three-layer-resolution) \
                     for how djls determines tag availability.",
                ),
                example: Some("{% load crispy_forms_tags %}\n{% crispy form %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::FilterNotInInstalledApps {
                filter: String::new(),
                app: String::new(),
                load_name: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks for filters that exist in an installed Python package but \
                       whose Django app isn't in `INSTALLED_APPS`. The package is \
                       installed — the app just needs to be activated.",
                why: Some(
                    "Django won't discover the template tag library (and its filters) \
                     unless the app is listed in `INSTALLED_APPS`.",
                ),
                fix: Some(
                    "Add the app to `INSTALLED_APPS` in your Django settings.\n\n\
                     See [Three-Layer Resolution]\
                     (./template-validation.md#three-layer-resolution) \
                     for how djls determines filter availability.",
                ),
                example: Some("{% load some_filters %}\n{{ value|custom_filter }}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::UnknownLibrary {
                name: String::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks that the library name in a `{% load %}` tag refers to a \
                       known template tag library. If the library isn't found in any \
                       installed package or built-in, it's reported as unknown.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when `{% load %}` references a \
                     library that doesn't exist.",
                ),
                fix: Some(
                    "Check the library name for typos. If the library comes from a \
                     third-party package, install it with `pip install <package>`.",
                ),
                example: Some("{% load nonexistent_lib %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::LibraryNotInInstalledApps {
                name: String::new(),
                app: String::new(),
                candidates: Vec::new(),
                span: dummy_span(),
            },
            ValidationDoc {
                what: "Checks for `{% load %}` library names that exist in an installed \
                       Python package but whose Django app isn't in `INSTALLED_APPS`.",
                why: Some(
                    "Django won't discover the template tag library unless the app is \
                     listed in `INSTALLED_APPS`, and `{% load %}` will fail with \
                     `TemplateSyntaxError`.",
                ),
                fix: Some("Add the app to `INSTALLED_APPS` in your Django settings."),
                example: Some("{% load crispy_forms_tags %}"),
                requires_inspector: true,
            },
        ),
        validation_rule(
            &ValidationError::ExtendsMustBeFirst { span: dummy_span() },
            ValidationDoc {
                what: "Checks that `{% extends %}` is the first tag in the template. \
                       Text and `{# comments #}` are allowed before it, but no other \
                       tags or variable expressions (`{{ ... }}`) may appear first.",
                why: Some(
                    "Django enforces this at parse time — `{% extends %}` must come \
                     before any other template tags or variables.",
                ),
                fix: None,
                example: Some("{% load i18n %}\n{% extends \"base.html\" %}"),
                requires_inspector: false,
            },
        ),
        validation_rule(
            &ValidationError::MultipleExtends { span: dummy_span() },
            ValidationDoc {
                what: "Checks that `{% extends %}` appears at most once in a template.",
                why: Some(
                    "A template can only extend one parent. Django raises \
                     `TemplateSyntaxError` if multiple `{% extends %}` tags are found.",
                ),
                fix: None,
                example: Some("{% extends \"base.html\" %}\n{% extends \"other.html\" %}"),
                requires_inspector: false,
            },
        ),
    ]
}
