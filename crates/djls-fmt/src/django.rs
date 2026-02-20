use djls_conf::FormatConfig;
use djls_templates::tokens::Token;
use djls_templates::Lexer;

/// Format Django template syntax in non-HTML templates.
///
/// Operates at the token level: lexes the source, applies formatting passes
/// to normalize Django tags/variables/comments, then reconstructs the output.
/// Text content between Django constructs is preserved unchanged.
#[must_use]
pub fn format_django_syntax(source: &str, config: &FormatConfig) -> String {
    let tokens = Lexer::new(source).tokenize();
    let mut fmt_tokens: Vec<FmtToken> = tokens
        .into_iter()
        .map(|t| FmtToken::from_token(t, source))
        .collect();

    // Multi-token passes (order matters: merge before sort so merged tags get sorted)
    if config.merge_load_tags() {
        merge_load_tags(&mut fmt_tokens);
    }
    if config.label_endblocks() {
        label_endblocks(&mut fmt_tokens);
    }

    // Per-token formatting during reconstruction
    let mut out = String::with_capacity(source.len());
    for token in &fmt_tokens {
        token.render_to(&mut out, config);
    }
    out
}

/// A token stripped of source positions, carrying only what the formatter needs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FmtToken {
    /// Django block tag content (between `{% %}`)
    Block(String),
    /// Django variable content (between `{{ }}`)
    Variable(String),
    /// Django comment content (between `{# #}`)
    Comment(String),
    /// Literal text (HTML, plain text, etc.)
    Text(String),
    /// Horizontal whitespace
    Whitespace(String),
    /// Line ending (`\n` or `\r\n`)
    Newline(String),
}

impl FmtToken {
    fn from_token(token: Token, source: &str) -> Self {
        match token {
            Token::Block { content, .. } => Self::Block(content),
            Token::Variable { content, .. } => Self::Variable(content),
            Token::Comment { content, .. } => Self::Comment(content),
            Token::Text { content, .. } | Token::Error { content, .. } => Self::Text(content),
            Token::Whitespace { span } => {
                let start = span.start() as usize;
                Self::Whitespace(source[start..start + span.length_usize()].to_string())
            }
            Token::Newline { span } => {
                let start = span.start() as usize;
                Self::Newline(source[start..start + span.length_usize()].to_string())
            }
            Token::Eof => Self::Text(String::new()),
        }
    }

    /// Write this token's formatted representation into `out`.
    ///
    /// Block and variable tokens have their content normalized (whitespace,
    /// filter chains, load sorting). Text, whitespace, and newlines pass
    /// through unchanged.
    fn render_to(&self, out: &mut String, config: &FormatConfig) {
        match self {
            Self::Block(content) => {
                let normalized = normalize_block_content(content, config);
                if normalized.is_empty() {
                    out.push_str("{%  %}");
                } else {
                    out.push_str("{% ");
                    out.push_str(&normalized);
                    out.push_str(" %}");
                }
            }
            Self::Variable(content) => {
                let normalized = normalize_variable_content(content);
                if normalized.is_empty() {
                    out.push_str("{{  }}");
                } else {
                    out.push_str("{{ ");
                    out.push_str(&normalized);
                    out.push_str(" }}");
                }
            }
            Self::Comment(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    out.push_str("{#  #}");
                } else {
                    out.push_str("{# ");
                    out.push_str(trimmed);
                    out.push_str(" #}");
                }
            }
            Self::Text(text) => out.push_str(text),
            Self::Whitespace(ws) => out.push_str(ws),
            Self::Newline(nl) => out.push_str(nl),
        }
    }

    fn is_whitespace_or_newline(&self) -> bool {
        matches!(self, Self::Whitespace(_) | Self::Newline(_))
    }
}

// Normalize block tag content: trim whitespace, collapse argument spacing
// to single spaces (preserving quoted strings), and optionally sort load
// tag libraries.
fn normalize_block_content(content: &str, config: &FormatConfig) -> String {
    let parts = split_on_whitespace(content.trim());
    if parts.is_empty() {
        return String::new();
    }

    // Sort load libraries (skip `{% load X from Y %}` syntax)
    let is_simple_load = parts[0] == "load" && parts.len() > 1 && !parts.contains(&"from");
    if config.sort_load_libraries() && is_simple_load {
        let mut libs = parts[1..].to_vec();
        libs.sort_unstable();
        libs.dedup();
        return format!("load {}", libs.join(" "));
    }

    parts.join(" ")
}

// Normalize variable content: trim whitespace and collapse spaces around
// `|` (filter separator) and `:` (filter argument), preserving quoted strings.
fn normalize_variable_content(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let segments = split_on_unquoted(trimmed, '|');
    let mut out = String::with_capacity(trimmed.len());

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        let segment = segment.trim();
        if i == 0 {
            // Variable name: preserve as-is (already trimmed)
            out.push_str(segment);
        } else {
            // Filter: normalize spaces around `:`
            out.push_str(&normalize_filter_segment(segment));
        }
    }

    out
}

/// Normalize a single filter segment, removing spaces around `:`.
fn normalize_filter_segment(segment: &str) -> String {
    let trimmed = segment.trim();
    match find_unquoted(trimmed, ':') {
        Some(pos) => {
            let name = trimmed[..pos].trim_end();
            let arg = trimmed[pos + 1..].trim_start();
            if arg.is_empty() {
                name.to_string()
            } else {
                format!("{name}:{arg}")
            }
        }
        None => trimmed.to_string(),
    }
}

/// Check whether a block token is a simple `{% load lib1 lib2 %}` (not `{% load X from Y %}`).
fn is_simple_load_block(token: &FmtToken) -> bool {
    match token {
        FmtToken::Block(content) => {
            let parts = split_on_whitespace(content.trim());
            parts.first().copied() == Some("load") && parts.len() > 1 && !parts.contains(&"from")
        }
        FmtToken::Variable(_)
        | FmtToken::Comment(_)
        | FmtToken::Text(_)
        | FmtToken::Whitespace(_)
        | FmtToken::Newline(_) => false,
    }
}

// Merge consecutive `{% load %}` tags into a single tag.
//
// Finds runs of simple load blocks (not `{% load X from Y %}`) separated
// only by whitespace/newlines, merges their libraries into one load tag,
// and removes the redundant tokens.
fn merge_load_tags(tokens: &mut Vec<FmtToken>) {
    // Collect groups of consecutive load block indices
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group: Vec<usize> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if is_simple_load_block(&tokens[i]) {
            current_group.push(i);
            // Look ahead past whitespace/newlines for another load
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].is_whitespace_or_newline() {
                j += 1;
            }
            if j < tokens.len() && is_simple_load_block(&tokens[j]) {
                i = j;
                continue;
            }
            // End of group
            if current_group.len() > 1 {
                groups.push(std::mem::take(&mut current_group));
            } else {
                current_group.clear();
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // Process groups in reverse to preserve indices
    for group in groups.into_iter().rev() {
        let first_idx = group[0];
        let last_idx = *group.last().expect("group is non-empty");

        // Collect all library names from the group
        let mut all_libs: Vec<&str> = Vec::new();
        for &idx in &group {
            if let FmtToken::Block(content) = &tokens[idx] {
                let parts = split_on_whitespace(content.trim());
                // Skip the "load" tag name, collect libraries
                all_libs.extend(parts.into_iter().skip(1));
            }
        }
        all_libs.sort_unstable();
        all_libs.dedup();

        // Replace the range [first_idx..=last_idx] with a single merged token
        let merged_content = format!("load {}", all_libs.join(" "));
        let merged = FmtToken::Block(merged_content);

        // Remove everything from first to last (inclusive) and insert merged
        tokens.splice(first_idx..=last_idx, std::iter::once(merged));
    }
}

// Label unlabeled `{% endblock %}` tags with the block name.
//
// When a closing `{% endblock %}` sits on a different line from its
// matching `{% block name %}`, and it doesn't already carry the block
// name, append the name: `{% endblock %}` → `{% endblock content %}`.
fn label_endblocks(tokens: &mut [FmtToken]) {
    // Stack of (block_name, crossed_newline)
    let mut block_stack: Vec<(String, bool)> = Vec::new();

    for token in tokens.iter_mut() {
        match token {
            FmtToken::Block(content) => {
                let parts = split_on_whitespace(content.trim());
                let tag_name = parts.first().copied();

                match tag_name {
                    Some("block") => {
                        let name = parts.get(1).copied().unwrap_or_default().to_string();
                        block_stack.push((name, false));
                    }
                    Some("endblock") => {
                        if let Some((name, crossed_newline)) = block_stack.pop() {
                            let has_label = parts.len() > 1;
                            if crossed_newline && !has_label && !name.is_empty() {
                                *token = FmtToken::Block(format!("endblock {name}"));
                            }
                        }
                    }
                    _ => {}
                }
            }
            FmtToken::Newline(_) => {
                for (_, crossed) in &mut block_stack {
                    *crossed = true;
                }
            }
            FmtToken::Variable(_)
            | FmtToken::Comment(_)
            | FmtToken::Text(_)
            | FmtToken::Whitespace(_) => {}
        }
    }
}

// Quote-aware string splitting utilities for Django template syntax.

/// Split `s` on whitespace while preserving quoted regions.
fn split_on_whitespace(s: &str) -> Vec<&str> {
    let mut pieces = Vec::with_capacity(4);
    let mut start: Option<usize> = None;
    let mut in_quote: Option<char> = None;

    for (i, ch) in s.char_indices() {
        match ch {
            '"' | '\'' if in_quote == Some(ch) => {
                in_quote = None;
                if start.is_none() {
                    start = Some(i);
                }
            }
            '"' | '\'' if in_quote.is_none() => {
                in_quote = Some(ch);
                if start.is_none() {
                    start = Some(i);
                }
            }
            _ if in_quote.is_some() => {
                if start.is_none() {
                    start = Some(i);
                }
            }
            _ if ch.is_whitespace() => {
                if let Some(s_start) = start.take() {
                    pieces.push(&s[s_start..i]);
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }
    if let Some(s_start) = start {
        pieces.push(&s[s_start..]);
    }
    pieces
}

/// Split `s` on a delimiter character, skipping occurrences inside quotes.
fn split_on_unquoted(s: &str, delim: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut in_quote: Option<char> = None;

    for (i, ch) in s.char_indices() {
        match ch {
            '"' | '\'' if in_quote == Some(ch) => in_quote = None,
            '"' | '\'' if in_quote.is_none() => in_quote = Some(ch),
            c if c == delim && in_quote.is_none() => {
                result.push(&s[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}

/// Find the first occurrence of `needle` in `s` outside quoted regions.
fn find_unquoted(s: &str, needle: char) -> Option<usize> {
    let mut in_quote: Option<char> = None;

    for (i, ch) in s.char_indices() {
        match ch {
            '"' | '\'' if in_quote == Some(ch) => in_quote = None,
            '"' | '\'' if in_quote.is_none() => in_quote = Some(ch),
            c if c == needle && in_quote.is_none() => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> FormatConfig {
        FormatConfig::default()
    }

    fn config_no_sort() -> FormatConfig {
        FormatConfig::default().with_sort_load_libraries(false)
    }

    fn config_no_merge() -> FormatConfig {
        FormatConfig::default().with_merge_load_tags(false)
    }

    fn config_no_endblock_label() -> FormatConfig {
        FormatConfig::default().with_label_endblocks(false)
    }

    mod delimiter_whitespace {
        use super::*;

        #[test]
        fn block_tag_normalized() {
            let input = "{%if user%}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% if user %}");
        }

        #[test]
        fn block_tag_extra_spaces() {
            let input = "{%   if   user   %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% if user %}");
        }

        #[test]
        fn variable_tag_normalized() {
            let input = "{{user.name}}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ user.name }}");
        }

        #[test]
        fn variable_tag_extra_spaces() {
            let input = "{{   user.name   }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ user.name }}");
        }

        #[test]
        fn comment_normalized() {
            let input = "{#note#}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{# note #}");
        }

        #[test]
        fn comment_extra_spaces() {
            let input = "{#   todo: fix this   #}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{# todo: fix this #}");
        }

        #[test]
        fn already_correct() {
            let input = "{% if user %}{{ name }}{# comment #}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }
    }

    mod argument_spacing {
        use super::*;

        #[test]
        fn multiple_spaces_collapsed() {
            let input = "{%  if  user.is_authenticated  %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% if user.is_authenticated %}");
        }

        #[test]
        fn quoted_strings_preserved() {
            let input = "{% url \"my-view\" arg1 %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% url \"my-view\" arg1 %}");
        }

        #[test]
        fn single_quoted_strings_preserved() {
            let input = "{% url 'my-view' arg1 %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% url 'my-view' arg1 %}");
        }

        #[test]
        fn spaces_inside_quotes_preserved() {
            let input = "{% trans \"hello world\" %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% trans \"hello world\" %}");
        }
    }

    mod filter_spacing {
        use super::*;

        #[test]
        fn spaces_around_pipe_removed() {
            let input = "{{ value | lower }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ value|lower }}");
        }

        #[test]
        fn spaces_around_colon_removed() {
            let input = "{{ value|default : \"nothing\" }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ value|default:\"nothing\" }}");
        }

        #[test]
        fn filter_chain_normalized() {
            let input = "{{ value | default : \"nothing\" | title }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ value|default:\"nothing\"|title }}");
        }

        #[test]
        fn already_correct_filter() {
            let input = "{{ value|default:\"nothing\"|title }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn quoted_pipe_preserved() {
            let input = "{{ \"a|b\"|title }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ \"a|b\"|title }}");
        }

        #[test]
        fn single_variable_no_filter() {
            let input = "{{ user.name }}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{{ user.name }}");
        }
    }

    mod load_sorting {
        use super::*;

        #[test]
        fn libraries_sorted() {
            let input = "{% load i18n humanize static %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load humanize i18n static %}");
        }

        #[test]
        fn already_sorted() {
            let input = "{% load humanize i18n static %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn single_library() {
            let input = "{% load i18n %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load i18n %}");
        }

        #[test]
        fn duplicates_removed() {
            let input = "{% load i18n humanize i18n %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load humanize i18n %}");
        }

        #[test]
        fn sorting_disabled() {
            let input = "{% load i18n humanize %}";
            let result = format_django_syntax(input, &config_no_sort());
            assert_eq!(result, "{% load i18n humanize %}");
        }

        #[test]
        fn load_from_syntax_preserved() {
            let input = "{% load i18n from django.utils %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load i18n from django.utils %}");
        }
    }

    mod load_merging {
        use super::*;

        #[test]
        fn consecutive_loads_merged() {
            let input = "{% load i18n %}\n{% load humanize %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load humanize i18n %}");
        }

        #[test]
        fn three_loads_merged() {
            let input = "{% load i18n %}\n{% load humanize %}\n{% load static %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load humanize i18n static %}");
        }

        #[test]
        fn non_adjacent_loads_not_merged() {
            let input = "{% load i18n %}\n<p>hello</p>\n{% load humanize %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load i18n %}\n<p>hello</p>\n{% load humanize %}");
        }

        #[test]
        fn merging_disabled() {
            let input = "{% load i18n %}\n{% load humanize %}";
            let result = format_django_syntax(input, &config_no_merge());
            // Sorting still applies within each tag
            assert_eq!(result, "{% load i18n %}\n{% load humanize %}");
        }

        #[test]
        fn loads_with_whitespace_between() {
            let input = "{% load i18n %}  \n  {% load humanize %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% load humanize i18n %}");
        }

        #[test]
        fn load_from_not_merged() {
            let input = "{% load i18n from django.utils %}\n{% load humanize %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(
                result,
                "{% load i18n from django.utils %}\n{% load humanize %}"
            );
        }

        #[test]
        fn simple_load_not_merged_with_from() {
            let input = "{% load humanize %}\n{% load i18n from django.utils %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(
                result,
                "{% load humanize %}\n{% load i18n from django.utils %}"
            );
        }
    }

    mod endblock_labeling {
        use super::*;

        #[test]
        fn endblock_labeled_multiline() {
            let input = "{% block content %}\n  <p>hello</p>\n{% endblock %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(
                result,
                "{% block content %}\n  <p>hello</p>\n{% endblock content %}"
            );
        }

        #[test]
        fn endblock_same_line_not_labeled() {
            let input = "{% block title %}Page Title{% endblock %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, "{% block title %}Page Title{% endblock %}");
        }

        #[test]
        fn endblock_already_labeled() {
            let input = "{% block content %}\n  <p>hello</p>\n{% endblock content %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn nested_blocks_labeled() {
            let input =
                "{% block outer %}\n  {% block inner %}\n    <p>hi</p>\n  {% endblock %}\n{% endblock %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(
                result,
                "{% block outer %}\n  {% block inner %}\n    <p>hi</p>\n  {% endblock inner %}\n{% endblock outer %}"
            );
        }

        #[test]
        fn deeply_nested_blocks_all_labeled() {
            let input = "{% block base %}\n{% block outer %}\n{% block inner %}\ncontent\n{% endblock %}\n{% endblock %}\n{% endblock %}";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(
                result,
                "{% block base %}\n{% block outer %}\n{% block inner %}\ncontent\n{% endblock inner %}\n{% endblock outer %}\n{% endblock base %}"
            );
        }

        #[test]
        fn labeling_disabled() {
            let input = "{% block content %}\n  <p>hello</p>\n{% endblock %}";
            let result = format_django_syntax(input, &config_no_endblock_label());
            assert_eq!(result, input);
        }
    }

    mod idempotency {
        use super::*;

        #[test]
        fn formatting_is_idempotent() {
            let cases = vec![
                "{%if user%}{{user.name}}{%endif%}",
                "{% load i18n humanize static %}",
                "{% load i18n %}\n{% load humanize %}",
                "{% load i18n from django.utils %}",
                "{% block content %}\n  <p>hello</p>\n{% endblock %}",
                "{% block base %}\n{% block outer %}\n{% block inner %}\ncontent\n{% endblock %}\n{% endblock %}\n{% endblock %}",
                "{{ value | default : \"nothing\" | title }}",
                "{#note#}",
            ];

            let config = default_config();
            for source in cases {
                let first = format_django_syntax(source, &config);
                let second = format_django_syntax(&first, &config);
                assert_eq!(
                    first, second,
                    "Not idempotent for input: {source:?}\nFirst pass: {first:?}\nSecond pass: {second:?}"
                );
            }
        }
    }

    mod text_preservation {
        use super::*;

        #[test]
        fn plain_text_unchanged() {
            let input = "Hello, world!\nThis is plain text.";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn mixed_content() {
            let input = "Hello {{ name }}, welcome to {% if show %}the site{% endif %}!";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn html_preserved() {
            let input =
                "<div class=\"container\">\n  {% if user %}\n    <p>{{ user.name }}</p>\n  {% endif %}\n</div>";
            let result = format_django_syntax(input, &default_config());
            assert_eq!(result, input);
        }

        #[test]
        fn empty_input() {
            let result = format_django_syntax("", &default_config());
            assert_eq!(result, "");
        }
    }

    mod snapshot {
        use super::*;

        #[test]
        fn full_template_formatting() {
            let input = r#"{%load i18n%}
{%load humanize%}
{%  block  content  %}
  <h1>{{  title | upper  }}</h1>
  {%if user.is_authenticated%}
    <p>Welcome, {{ user.name | default : "Anonymous" | title }}!</p>
    {#todo: add logout link#}
  {%endif%}
{%endblock%}
"#;
            let result = format_django_syntax(input, &default_config());
            insta::assert_snapshot!(result);
        }

        #[test]
        fn email_template_formatting() {
            let input = r#"{%load i18n%}
{%  autoescape  off  %}
Dear {{  name | title  }},

{%  blocktrans  with  amount=order.total  %}
Your order total is {{ amount | floatformat : 2 }}.
{%  endblocktrans  %}

{%  if  has_discount  %}
{#  show discount info  #}
Discount: {{  discount | default : "0%" }}
{%  endif  %}
{%  endautoescape  %}
"#;
            let result = format_django_syntax(input, &default_config());
            insta::assert_snapshot!(result);
        }
    }

    mod quote_utils {
        use super::*;

        #[test]
        fn split_whitespace_simple() {
            assert_eq!(
                split_on_whitespace("load i18n l10n"),
                vec!["load", "i18n", "l10n"]
            );
        }

        #[test]
        fn split_whitespace_quoted() {
            assert_eq!(
                split_on_whitespace(r#"if x == "hello world""#),
                vec!["if", "x", "==", r#""hello world""#]
            );
        }

        #[test]
        fn split_whitespace_empty() {
            assert!(split_on_whitespace("").is_empty());
            assert!(split_on_whitespace("   ").is_empty());
        }

        #[test]
        fn split_unquoted_pipe() {
            assert_eq!(split_on_unquoted("a|b|c", '|'), vec!["a", "b", "c"]);
        }

        #[test]
        fn split_unquoted_pipe_quoted() {
            assert_eq!(split_on_unquoted("\"a|b\"|c", '|'), vec!["\"a|b\"", "c"]);
        }

        #[test]
        fn find_unquoted_colon() {
            assert_eq!(find_unquoted("default:value", ':'), Some(7));
            assert_eq!(find_unquoted("\"a:b\":c", ':'), Some(5));
            assert_eq!(find_unquoted("\"a:b\"", ':'), None);
        }
    }
}
