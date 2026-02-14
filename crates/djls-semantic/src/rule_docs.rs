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
    docs.extend(ValidationError::rule_docs());
    docs
}

fn template_error_docs() -> Vec<RuleDoc> {
    vec![
        RuleDoc {
            code: "T100",
            name: "parser-error",
            what: "Reports syntax errors in Django template markup — unclosed tags, \
                       malformed expressions, or invalid template syntax that prevents \
                       parsing.",
            why: None,
            fix: Some(
                "Correct the template syntax. Common causes include unclosed `{{` or \
                     `{%` delimiters, unmatched quotes in tag arguments, and malformed \
                     filter expressions.",
            ),
            example: None,
            requires_inspector: false,
        },
        RuleDoc {
            code: "T900",
            name: "io-error",
            what: "Reports file I/O errors encountered when reading a template file.",
            why: None,
            fix: Some(
                "Check that the file exists, is readable, and is not locked by \
                     another process.",
            ),
            example: None,
            requires_inspector: false,
        },
        RuleDoc {
            code: "T901",
            name: "config-error",
            what: "Reports configuration errors that prevent template processing.",
            why: None,
            fix: Some(
                "Check your djls configuration. See \
                     [Configuration](./configuration/index.md) for details.",
            ),
            example: None,
            requires_inspector: false,
        },
    ]
}

impl ValidationError {
    #[allow(clippy::too_many_lines)]
    fn rule_docs() -> Vec<RuleDoc> {
        vec![
            RuleDoc {
                code: "S100",
                name: "unclosed-tag",
                what: "Checks that block tags have their required closing tag. For example, \
                       `{% block %}` must have a matching `{% endblock %}`, and `{% if %}` \
                       must have `{% endif %}`.",
                why: Some(
                    "Django raises `TemplateSyntaxError` at render time for unclosed block \
                     tags.",
                ),
                fix: None,
                example: Some("{% block content %}\n  <h1>Hello</h1>"),
                requires_inspector: false,
            },
            RuleDoc {
                code: "S101",
                name: "unbalanced-structure",
                what: "Checks that block tags are properly matched. Detects cases where a \
                       closing tag doesn't correspond to its opening tag — for example, \
                       `{% if %}` closed by `{% endfor %}`.",
                why: Some("Django raises `TemplateSyntaxError` for mismatched block tags."),
                fix: None,
                example: Some("{% if user.is_authenticated %}\n  <p>Welcome</p>\n{% endfor %}"),
                requires_inspector: false,
            },
            RuleDoc {
                code: "S102",
                name: "orphaned-tag",
                what: "Checks that intermediate tags (like `{% else %}`, `{% elif %}`, \
                       `{% empty %}`) appear inside their expected parent block.",
                why: Some(
                    "Django raises `TemplateSyntaxError` for intermediate tags that appear \
                     outside their parent structure.",
                ),
                fix: None,
                example: Some("{% else %}\n  <p>Fallback</p>"),
                requires_inspector: false,
            },
            RuleDoc {
                code: "S103",
                name: "unmatched-block-name",
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
            RuleDoc {
                code: "S108",
                name: "unknown-tag",
                what: "Checks that a template tag exists in at least one known library. \
                       If the tag isn't found in any installed package or built-in library, \
                       it's reported as unknown.",
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
            RuleDoc {
                code: "S109",
                name: "unloaded-tag",
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
            RuleDoc {
                code: "S110",
                name: "ambiguous-unloaded-tag",
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
            RuleDoc {
                code: "S111",
                name: "unknown-filter",
                what: "Checks that a template filter exists in at least one known library. \
                       If the filter isn't found in any installed package or built-in \
                       library, it's reported as unknown.",
                why: Some("Django raises `TemplateSyntaxError` for filters it doesn't recognize."),
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
            RuleDoc {
                code: "S112",
                name: "unloaded-filter",
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
            RuleDoc {
                code: "S113",
                name: "ambiguous-unloaded-filter",
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
            RuleDoc {
                code: "S114",
                name: "expression-syntax-error",
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
            RuleDoc {
                code: "S115",
                name: "filter-missing-argument",
                what: "Checks that filters which require an argument are called with one. \
                       Filter arity is determined by static analysis of the filter's \
                       Python source code.",
                why: Some(
                    "Django raises `TemplateSyntaxError` when a filter that requires an \
                     argument is called without one.",
                ),
                fix: None,
                example: Some("{{ value|default }}"),
                requires_inspector: true,
            },
            RuleDoc {
                code: "S116",
                name: "filter-unexpected-argument",
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
            RuleDoc {
                code: "S117",
                name: "extracted-rule-violation",
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
            RuleDoc {
                code: "S118",
                name: "tag-not-in-installed-apps",
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
            RuleDoc {
                code: "S119",
                name: "filter-not-in-installed-apps",
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
            RuleDoc {
                code: "S120",
                name: "unknown-library",
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
            RuleDoc {
                code: "S121",
                name: "library-not-in-installed-apps",
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
            RuleDoc {
                code: "S122",
                name: "extends-must-be-first",
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
            RuleDoc {
                code: "S123",
                name: "multiple-extends",
                what: "Checks that `{% extends %}` appears at most once in a template.",
                why: Some(
                    "A template can only extend one parent. Django raises \
                     `TemplateSyntaxError` if multiple `{% extends %}` tags are found.",
                ),
                fix: None,
                example: Some("{% extends \"base.html\" %}\n{% extends \"other.html\" %}"),
                requires_inspector: false,
            },
        ]
    }
}
