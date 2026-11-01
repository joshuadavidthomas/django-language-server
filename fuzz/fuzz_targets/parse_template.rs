#![no_main]

use djls_source::Span;
use djls_templates::Node;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        check(source);
    }
});

fn check(source: &str) {
    let (nodes, errors) = djls_templates::parse_template_impl(source);

    let assert_char_range = |span: Span, what: &str| {
        assert!(
            span.end_usize() <= source.len()
                && source.is_char_boundary(span.start_usize())
                && source.is_char_boundary(span.end_usize()),
            "{what} {span:?} is not a char range of {source:?}"
        );
    };

    let mut cursor = 0;
    for node in &nodes {
        let full = node.full_span();
        assert_char_range(full, "full span");
        assert!(
            full.start_usize() == cursor,
            "nodes must tile the source without gaps or overlaps at {node:?} in {source:?}"
        );
        cursor = full.end_usize();

        let assert_inside = |span: Span, what: &str| {
            assert_char_range(span, what);
            assert!(
                full.start() <= span.start() && span.end() <= full.end(),
                "{what} {span:?} escapes its node {node:?} in {source:?}"
            );
        };
        match node {
            Node::Tag {
                name_span,
                bits,
                span,
                ..
            } => {
                assert_inside(*span, "tag span");
                assert_inside(*name_span, "tag name span");
                for bit in bits {
                    assert_inside(bit.span, "tag bit span");
                }
            }
            Node::Variable {
                var_span,
                filters,
                span,
                ..
            } => {
                assert_inside(*span, "variable span");
                assert_inside(*var_span, "variable name span");
                for filter in filters {
                    assert_inside(filter.span, "filter span");
                }
            }
            Node::Comment { span, .. } | Node::Error { span, .. } => {
                assert_inside(*span, "node span");
            }
            Node::Text { .. } => {}
        }
    }
    assert!(cursor == source.len(), "nodes must cover {source:?}");

    for error in &errors {
        if let Some((start, length)) = error.primary_span() {
            assert!(
                start.saturating_add(length) <= u32::try_from(source.len()).unwrap_or(u32::MAX),
                "error {error:?} points past the end of {source:?}"
            );
        }
    }

    check_against_django(source, &nodes);
}

/// Django's lexer is the oracle: every `{% %}`, `{{ }}`, and `{# #}` it finds must be a node with
/// the same span, and DJLS must not find a construct Django treats as text.
fn check_against_django(source: &str, nodes: &[Node]) {
    // Django's Lexer switches to text inside `{% verbatim %}`, which DJLS leaves to djls-semantic.
    if source.contains("verbatim") {
        return;
    }
    // Known bug: in `{{% if x %}`, Django's regex fails `{{` and retries one byte later, finding
    // the `{%` tag. DJLS reports an unclosed `{{` and loses the tag. Excluded until fixed.
    if source.contains("{{%") || source.contains("{{#") {
        return;
    }
    let djls: Vec<(Option<u8>, Span)> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Tag { .. } => Some((Some(b'%'), node.full_span())),
            Node::Variable { .. } => Some((Some(b'{'), node.full_span())),
            Node::Comment { .. } => Some((Some(b'#'), node.full_span())),
            Node::Error { .. } => Some((None, node.full_span())),
            Node::Text { .. } => None,
        })
        .collect();
    let django = django_tokens(source);

    for &(kind, start, end) in &django {
        let found = djls.iter().any(|&(djls_kind, span)| {
            span.start_usize() == start
                && span.end_usize() == end
                // An Error node is a syntax error Django raises when it compiles the token.
                && djls_kind.is_none_or(|djls_kind| djls_kind == kind)
        });
        assert!(
            found,
            "Django lexes {:?} at {start}..{end}, DJLS does not; DJLS nodes {nodes:?} in {source:?}",
            &source[start..end]
        );
    }
    for &(kind, span) in &djls {
        if kind.is_some() {
            assert!(
                django
                    .iter()
                    .any(|&(_, start, end)| start == span.start_usize() && end == span.end_usize()),
                "DJLS lexes {:?} at {span:?}, Django treats it as text in {source:?}",
                &source[range(span)]
            );
        }
    }
}

/// Port of `django.template.base.Lexer.tokenize` without verbatim handling: the matches of
/// `tag_re = {%.*?%}|{{.*?}}|{#.*?#}`. Returns the opener's second byte and the byte range of
/// each match.
///
/// DJLS lexes like `tag_re` compiled with `re.DOTALL`, a common project patch that allows
/// multi-line tags. Django's default stops a match at a newline.
fn django_tokens(source: &str) -> Vec<(u8, usize, usize)> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    // A closer absent from one suffix is absent from every later suffix.
    let mut missing: Vec<&[u8]> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let closer: &[u8] = match (bytes[i], bytes[i + 1]) {
            (b'{', b'%') => b"%}",
            (b'{', b'{') => b"}}",
            (b'{', b'#') => b"#}",
            _ => {
                i += 1;
                continue;
            }
        };
        if missing.contains(&closer) {
            i += 1;
            continue;
        }
        if let Some(position) = bytes[i + 2..]
            .windows(2)
            .position(|window| window == closer)
        {
            let end = i + 2 + position + 2;
            tokens.push((bytes[i + 1], i, end));
            i = end;
        } else {
            missing.push(closer);
            i += 1;
        }
    }
    tokens
}

fn range(span: Span) -> std::ops::Range<usize> {
    span.start_usize()..span.end_usize()
}
