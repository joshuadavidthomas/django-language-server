#![no_main]

use djls_source::Span;
use djls_templates::LexerMode;
use djls_templates::Node;
use libfuzzer_sys::Corpus;
use libfuzzer_sys::fuzz_target;

fn do_fuzz(case: &[u8]) -> Corpus {
    let Ok(source) = std::str::from_utf8(case) else {
        return Corpus::Reject;
    };
    let in_source = |span: Span| {
        span.end_usize() <= source.len()
            && source.is_char_boundary(span.start_usize())
            && source.is_char_boundary(span.end_usize())
    };

    for mode in [LexerMode::SingleLine, LexerMode::MultiLine] {
        let (nodes, _) = djls_templates::parse_template_impl(source, mode);
        let mut cursor = 0;
        for node in &nodes {
            let full = node.full_span();
            let inner: Vec<Span> = match node {
                Node::Tag {
                    name_span, bits, ..
                } => bits
                    .iter()
                    .map(|bit| bit.span)
                    .chain([*name_span])
                    .collect(),
                Node::Variable {
                    var_span, filters, ..
                } => filters.iter().map(|f| f.span).chain([*var_span]).collect(),
                Node::Comment { .. } | Node::Error { .. } | Node::Text { .. } => Vec::new(),
            };
            for span in inner.into_iter().chain([full]) {
                assert!(in_source(span), "{mode:?}: {span:?} of {node:?}");
            }
            assert_eq!(full.start_usize(), cursor, "{mode:?}: gap at {node:?}");
            cursor = full.end_usize();
        }
        assert_eq!(cursor, source.len(), "{mode:?}: uncovered tail");
    }

    Corpus::Keep
}

fuzz_target!(|case: &[u8]| -> Corpus { do_fuzz(case) });
