use djls_macros::Diagnostic;
use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Error, Diagnostic, PartialEq, Eq, Serialize)]
pub enum ValidationError {
    /// Unclosed Tag
    ///
    /// This error occurs when a template tag is opened but never closed. Django template
    /// tags that require closing include blocks like `{% if %}`, `{% for %}`, `{% block %}`,
    /// etc.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% if user.is_authenticated %}
    ///     Welcome, {{ user.username }}!
    /// {# Missing {% endif %} here
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% if user.is_authenticated %}
    ///     Welcome, {{ user.username }}!
    /// {% endif %}
    /// ```
    #[diagnostic(code = "S100", category = "semantic")]
    #[error("Unclosed tag: {tag}")]
    UnclosedTag { tag: String, span: Span },

    /// Orphaned Tag
    ///
    /// This error occurs when a closing tag appears without a matching opening tag. This
    /// usually happens when:
    /// - A closing tag is present but the opening tag is missing
    /// - Tags are closed in the wrong order
    /// - A closing tag appears in the wrong context
    ///
    /// # Examples
    ///
    /// ```django
    /// <div>Content</div>
    /// {% endif %}  {# No corresponding {% if %} #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% if condition %}
    ///     <div>Content</div>
    /// {% endif %}
    /// ```
    #[diagnostic(code = "S102", category = "semantic")]
    #[error("Orphaned tag '{tag}' - {context}")]
    OrphanedTag {
        tag: String,
        context: String,
        span: Span,
    },

    /// Unbalanced Structure
    ///
    /// This error occurs when template block structures are not properly balanced. For example,
    /// opening a block with `{% if %}` but closing with `{% endfor %}` instead of `{% endif %}`.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% if condition %}
    ///     <div>Content</div>
    /// {% endfor %}  {# Wrong closing tag! #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% if condition %}
    ///     <div>Content</div>
    /// {% endif %}
    /// ```
    #[diagnostic(code = "S101", category = "semantic")]
    #[error("Unbalanced structure: '{opening_tag}' missing closing '{expected_closing}'")]
    UnbalancedStructure {
        opening_tag: String,
        expected_closing: String,
        opening_span: Span,
        closing_span: Option<Span>,
    },

    /// Unmatched Block Name
    ///
    /// This error occurs specifically with named blocks when the `{% endblock %}` tag
    /// specifies a name that doesn't match any currently open block.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% block header %}
    ///     <h1>Title</h1>
    /// {% endblock footer %}  {# Name mismatch! #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% block header %}
    ///     <h1>Title</h1>
    /// {% endblock header %}
    /// {# or simply #}
    /// {% endblock %}
    /// ```
    #[diagnostic(code = "S103", category = "semantic")]
    #[error("endblock '{name}' does not match any open block")]
    UnmatchedBlockName { name: String, span: Span },

    /// Missing Required Arguments
    ///
    /// This error occurs when a template tag is called without all its required arguments.
    /// Different Django template tags have different argument requirements.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% extends %}  {# Missing template name #}
    /// {% load %}     {# Missing library name #}
    /// {% include %}  {# Missing template name #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% extends "base.html" %}
    /// {% load static %}
    /// {% include "partials/header.html" %}
    /// ```
    #[diagnostic(code = "S104", category = "semantic")]
    #[error("Tag '{tag}' requires at least {min} argument{}", if *.min == 1 { "" } else { "s" })]
    MissingRequiredArguments { tag: String, min: usize, span: Span },

    /// Too Many Arguments
    ///
    /// This error occurs when a template tag is called with more arguments than it accepts.
    /// Each Django template tag has a specific signature defining the maximum number of
    /// arguments it can receive.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% csrf_token extra_arg %}  {# csrf_token takes no arguments #}
    /// {% now "Y-m-d" "extra" %}    {# now only takes one format string #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% csrf_token %}
    /// {% now "Y-m-d" %}
    /// ```
    ///
    /// Note: Some tags accept variable numbers of arguments using varargs. This error
    /// only appears when you exceed the maximum even for varargs tags.
    #[diagnostic(code = "S105", category = "semantic")]
    #[error("Tag '{tag}' accepts at most {max} argument{}", if *.max == 1 { "" } else { "s" })]
    TooManyArguments { tag: String, max: usize, span: Span },

    /// Missing Argument
    ///
    /// This error occurs when a template tag is missing a specific required argument by name.
    /// This is similar to MissingRequiredArguments but for named/keyword arguments.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% url 'view-name' %}  {# Missing required 'as' argument #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% url 'view-name' as my_url %}
    /// ```
    #[diagnostic(code = "S104", category = "semantic")]
    #[error("Tag '{tag}' is missing required argument '{argument}'")]
    MissingArgument {
        tag: String,
        argument: String,
        span: Span,
    },

    /// Invalid Literal Argument
    ///
    /// This error occurs when a template tag expects a specific literal keyword but receives
    /// something else. Some Django tags require exact keyword matches at specific positions.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% for item on items %}  {# Should be 'in', not 'on' #}
    ///     {{ item }}
    /// {% endfor %}
    ///
    /// {% if user is_not active %}  {# Should be 'not', not 'is_not' #}
    ///     ...
    /// {% endif %}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% for item in items %}
    ///     {{ item }}
    /// {% endfor %}
    ///
    /// {% if user is not active %}
    ///     ...
    /// {% endif %}
    /// ```
    #[diagnostic(code = "S106", category = "semantic")]
    #[error("Tag '{tag}' expects literal '{expected}'")]
    InvalidLiteralArgument {
        tag: String,
        expected: String,
        span: Span,
    },

    /// Invalid Argument Choice
    ///
    /// This error occurs when a template tag argument must be one of a predefined set of
    /// values, but receives something else. Unlike InvalidLiteralArgument which expects a
    /// single literal, this applies when multiple valid options exist.
    ///
    /// # Examples
    ///
    /// ```django
    /// {% cycle 'odd' 'even' as rowcolors silent=true %}  {# invalid; should be 'silent' without '=' #}
    ///
    /// {% autoescape yes %}  {# valid #}
    /// {% autoescape maybe %}  {# invalid - must be 'on' or 'off' #}
    /// ```
    ///
    /// # Fix
    ///
    /// ```django
    /// {% cycle 'odd' 'even' as rowcolors silent %}
    /// {% autoescape on %}
    /// ```
    #[diagnostic(code = "S107", category = "semantic")]
    #[error("Tag '{tag}' argument '{argument}' must be one of {choices:?}")]
    InvalidArgumentChoice {
        tag: String,
        argument: String,
        choices: Vec<String>,
        value: String,
        span: Span,
    },
}
