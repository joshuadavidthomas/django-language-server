//! Tag argument positions that Django compiles as template expressions.
//!
//! A [`TagExpressionGrammar`] belongs to a [`TagSpec`]: builtin specs name the hand-written Django
//! compile function they mirror, and extracted `parse_bits()` / `token_kwargs()` argument syntax
//! supplies it for other libraries. Each grammar locates the [`ExpressionOperand`]s that Django
//! passes to `parser.compile_filter()` for one tag occurrence.

mod filter_expression;

use djls_project::AssignmentMode;
use djls_project::TagArgumentSyntax;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::FilterArgument;
use djls_templates::TagBit;
pub(crate) use filter_expression::parse_filter_expression;

use crate::scoping::ScopedTagFacts;
use crate::structure::ActiveTemplateTag;
use crate::tags::TagSpec;

/// The expression grammar for one active tag occurrence.
///
/// Captured intermediates have no Tag Definition of their own; their grammar comes from the
/// opening tag's spec. A tag that an unresolved load could shadow has no known grammar.
pub(crate) fn tag_expression_grammar(
    facts: &ScopedTagFacts,
    tag: ActiveTemplateTag<'_>,
) -> Option<TagExpressionGrammar> {
    let fact = match tag.opener_name_span {
        Some(opener) => facts.for_name_span(opener)?,
        None => facts.for_tag(tag)?,
    };
    if fact.unknown_load_can_shadow {
        return None;
    }
    let spec = fact.spec.as_ref()?;
    match tag.opener_name_span {
        Some(_) => spec.intermediate_expression_grammar(tag.tag),
        None => TagExpressionGrammar::for_spec(spec),
    }
}

/// Filter uses inside the syntactically valid expressions of one tag occurrence.
pub(crate) fn tag_expression_filters(
    grammar: TagExpressionGrammar,
    bits: &[TagBit],
) -> Vec<Filter> {
    let mut filters = Vec::new();
    for operand in grammar.operands(bits) {
        let Ok(parsed) = parse_filter_expression(&operand.text) else {
            continue;
        };
        for filter in parsed {
            let Some(span) = operand.source_span(filter.range.clone()) else {
                continue;
            };
            let arg = filter.argument.and_then(|range| {
                let span = operand.source_span(range.clone())?;
                Some(FilterArgument::new(operand.text[range].to_string(), span))
            });
            filters.push(Filter {
                name: filter.name,
                arg,
                span,
            });
        }
    }
    filters
}

/// How a tag's arguments reach Django's expression compilers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagExpressionGrammar {
    /// `{% if %}` / `{% elif %}`: the arguments form one boolean condition whose operands are
    /// filter expressions.
    Condition,
    /// Every argument, after an optional trailing `as name`.
    EachArgument {
        strips_as_var: bool,
    },
    /// The first argument.
    FirstArgument,
    /// `parse_bits()` arguments of `simple_tag`, `inclusion_tag`, and `simple_block_tag`.
    CallArguments {
        strips_as_var: bool,
    },
    /// `token_kwargs()` assignments over every argument.
    Assignments {
        legacy: bool,
    },
    BlockTranslate,
    Cache,
    Cycle,
    Filter,
    For,
    Include,
    LanguageInfo,
    Lorem,
    Regroup,
    Translate,
    Url,
    WidthRatio,
}

impl TagExpressionGrammar {
    /// The grammar for a tag, from its builtin meaning or its extracted argument syntax.
    fn for_spec(spec: &TagSpec) -> Option<Self> {
        if let Some(grammar) = spec.expression_grammar() {
            return Some(grammar);
        }
        let rules = spec.extracted_rules()?;
        match &rules.argument_syntax {
            TagArgumentSyntax::Signature { .. } => Some(Self::CallArguments {
                strips_as_var: rules.as_var.strips_suffix(),
            }),
            TagArgumentSyntax::Assignments { operand } => Some(Self::Assignments {
                legacy: operand.mode == AssignmentMode::ModernOrLegacy,
            }),
            TagArgumentSyntax::Unknown
            | TagArgumentSyntax::Parameters(_)
            | TagArgumentSyntax::Forms { .. } => None,
        }
    }

    /// The filter expressions Django compiles for one occurrence with these arguments.
    pub(crate) fn operands(self, bits: &[TagBit]) -> Vec<ExpressionOperand> {
        let mut operands = Vec::new();
        match self {
            Self::Condition => {
                let mut index = 0;
                while let Some(bit) = bits.get(index) {
                    let next = bits.get(index + 1).map(TagBit::as_str);
                    match (bit.as_str(), next) {
                        ("is", Some("not")) | ("not", Some("in")) => index += 1,
                        (
                            "or" | "and" | "not" | "in" | "is" | "==" | "!=" | ">" | ">=" | "<"
                            | "<=",
                            _,
                        ) => {}
                        _ => operands.push(ExpressionOperand::bit(bit)),
                    }
                    index += 1;
                }
            }
            Self::EachArgument { strips_as_var } => {
                let bits = if strips_as_var {
                    strip_as_var(bits)
                } else {
                    bits
                };
                operands.extend(bits.iter().map(ExpressionOperand::bit));
            }
            Self::FirstArgument => operands.extend(bits.first().map(ExpressionOperand::bit)),
            Self::CallArguments { strips_as_var } => {
                let bits = if strips_as_var {
                    strip_as_var(bits)
                } else {
                    bits
                };
                operands.extend(bits.iter().map(ExpressionOperand::keyword_value_or_bit));
            }
            Self::Assignments { legacy } => {
                token_kwargs(bits, legacy, &mut operands);
            }
            Self::BlockTranslate => block_translate(bits, &mut operands),
            Self::Cache => cache(bits, &mut operands),
            Self::Cycle => cycle(bits, &mut operands),
            Self::Filter => operands.extend(filter(bits)),
            Self::For => operands.extend(for_sequence(bits)),
            Self::Include => include(bits, &mut operands),
            Self::LanguageInfo => {
                if let [first, code, as_, _] = bits
                    && first.as_str() == "for"
                    && as_.as_str() == "as"
                {
                    operands.push(ExpressionOperand::bit(code));
                }
            }
            Self::Lorem => operands.extend(lorem_count(bits)),
            Self::Regroup => regroup(bits, &mut operands),
            Self::Translate => translate(bits, &mut operands),
            Self::Url => url(bits, &mut operands),
            Self::WidthRatio => match bits {
                [this, max, width] | [this, max, width, _, _]
                    if (bits.len() == 3 || bits[3].as_str() == "as") =>
                {
                    operands.extend([this, max, width].map(ExpressionOperand::bit));
                }
                _ => {}
            },
        }
        operands
    }
}

/// Text that Django passes to `parser.compile_filter()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpressionOperand {
    pub(crate) text: String,
    /// Where diagnostics about the whole expression point.
    pub(crate) span: Span,
    /// Runs of `text` copied verbatim from the source: `(text offset, source span)`.
    segments: Vec<(usize, Span)>,
    /// Django's replacement message when a caller wraps the compile error.
    pub(crate) message: Option<&'static str>,
}

impl ExpressionOperand {
    fn bit(bit: &TagBit) -> Self {
        Self::suffix(bit, 0)
    }

    /// The part of `bit` from byte `start`.
    fn suffix(bit: &TagBit, start: usize) -> Self {
        let text = bit.as_str()[start..].to_string();
        let span = Span::saturating_from_parts_usize(bit.span.start_usize() + start, text.len());
        Self {
            text,
            span,
            segments: vec![(0, span)],
            message: None,
        }
    }

    /// The value of a `key=value` bit, or the whole bit, as `token_kwargs()` / `kwarg_re` read it.
    fn keyword_value_or_bit(bit: &TagBit) -> Self {
        match keyword_value_start(bit.as_str()) {
            Some(start) => Self::suffix(bit, start),
            None => Self::bit(bit),
        }
    }

    /// Source span for a byte range of `text`, when that range was copied from the source.
    pub(crate) fn source_span(&self, range: std::ops::Range<usize>) -> Option<Span> {
        self.segments.iter().find_map(|(offset, span)| {
            let start = range.start.checked_sub(*offset)?;
            let end = range.end.checked_sub(*offset)?;
            (end <= span.length_usize())
                .then(|| Span::saturating_from_parts_usize(span.start_usize() + start, end - start))
        })
    }
}

fn strip_as_var(bits: &[TagBit]) -> &[TagBit] {
    match bits {
        [rest @ .., as_, _] if as_.as_str() == "as" => rest,
        _ => bits,
    }
}

/// Byte offset of the value in a `key=value` bit matched by Django's `kwarg_re`.
fn keyword_value_start(bit: &str) -> Option<usize> {
    let (key, value) = bit.split_once('=')?;
    (!value.is_empty() && crate::tags::is_python_word_key(key)).then_some(key.len() + 1)
}

/// Django's `token_kwargs()`: consumes leading assignments from `bits`, returning how many bits
/// it consumed.
fn token_kwargs(bits: &[TagBit], legacy: bool, operands: &mut Vec<ExpressionOperand>) -> usize {
    let Some(first) = bits.first() else {
        return 0;
    };
    let mut consumed = 0;
    if keyword_value_start(first.as_str()).is_some() {
        while let Some(bit) = bits.get(consumed)
            && let Some(start) = keyword_value_start(bit.as_str())
        {
            operands.push(ExpressionOperand::suffix(bit, start));
            consumed += 1;
        }
    } else if legacy {
        while let [value, as_, _, ..] = &bits[consumed..]
            && as_.as_str() == "as"
        {
            operands.push(ExpressionOperand::bit(value));
            consumed += 3;
            if bits.get(consumed).is_some_and(|bit| bit.as_str() == "and") {
                consumed += 1;
            } else {
                break;
            }
        }
    }
    consumed
}

fn for_sequence(bits: &[TagBit]) -> Option<ExpressionOperand> {
    if bits.len() < 3 {
        return None;
    }
    let reversed = bits.last().is_some_and(|bit| bit.as_str() == "reversed");
    let in_index = bits.len() - if reversed { 3 } else { 2 };
    (bits[in_index].as_str() == "in").then(|| ExpressionOperand::bit(&bits[in_index + 1]))
}

fn cycle(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    // A single argument names an existing cycle instead of compiling a value.
    if bits.len() < 2 {
        return;
    }
    let mut values = bits;
    if bits.len() > 3 {
        if bits[bits.len() - 3].as_str() == "as" {
            if bits[bits.len() - 1].as_str() != "silent" {
                return;
            }
            values = &bits[..bits.len() - 3];
        } else if bits[bits.len() - 2].as_str() == "as" {
            values = &bits[..bits.len() - 2];
        }
    }
    operands.extend(values.iter().map(ExpressionOperand::bit));
}

fn regroup(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    let [target, by, grouper, as_, name] = bits else {
        return;
    };
    operands.push(ExpressionOperand::bit(target));
    if by.as_str() != "by" || as_.as_str() != "as" {
        return;
    }
    // Django compiles `name.grouper` so each item resolves under the target name.
    let prefix = format!("{}.", name.as_str());
    let text = format!("{prefix}{}", grouper.as_str());
    let span =
        Span::saturating_from_bounds_usize(grouper.span.start_usize(), name.span.end_usize());
    operands.push(ExpressionOperand {
        text,
        span,
        segments: vec![(prefix.len(), grouper.span)],
        message: None,
    });
}

fn url(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    let Some((viewname, arguments)) = bits.split_first() else {
        return;
    };
    operands.push(ExpressionOperand::bit(viewname));
    operands.extend(
        strip_as_var(arguments)
            .iter()
            .map(ExpressionOperand::keyword_value_or_bit),
    );
}

fn filter(bits: &[TagBit]) -> Option<ExpressionOperand> {
    let (first, last) = (bits.first()?, bits.last()?);
    // Django compiles `var|<rest of the tag>`; whitespace between bits is not significant to
    // the filter grammar outside the reported text.
    let mut text = "var|".to_string();
    let mut segments = Vec::new();
    for (index, bit) in bits.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        segments.push((text.len(), bit.span));
        text.push_str(bit.as_str());
    }
    Some(ExpressionOperand {
        text,
        span: Span::saturating_from_bounds_usize(first.span.start_usize(), last.span.end_usize()),
        segments,
        message: None,
    })
}

fn lorem_count(bits: &[TagBit]) -> Option<ExpressionOperand> {
    let mut bits = bits;
    if let [rest @ .., last] = bits
        && last.as_str() == "random"
    {
        bits = rest;
    }
    if let [rest @ .., last] = bits
        && matches!(last.as_str(), "w" | "p" | "b")
    {
        bits = rest;
    }
    bits.last().map(ExpressionOperand::bit)
}

fn include(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    let Some((template, options)) = bits.split_first() else {
        return;
    };
    let mut index = 0;
    let mut seen = Vec::new();
    while let Some(option) = options.get(index) {
        index += 1;
        if seen.contains(&option.as_str()) {
            return;
        }
        seen.push(option.as_str());
        match option.as_str() {
            "with" => index += token_kwargs(&options[index..], false, operands),
            "only" => {}
            _ => return,
        }
    }
    operands.push(ExpressionOperand::bit(template));
}

fn translate(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    let Some((message, options)) = bits.split_first() else {
        return;
    };
    operands.push(ExpressionOperand::bit(message));
    let mut index = 0;
    let mut seen = Vec::new();
    while let Some(option) = options.get(index) {
        index += 1;
        if seen.contains(&option.as_str()) {
            return;
        }
        seen.push(option.as_str());
        match option.as_str() {
            "noop" => {}
            "context" => {
                let Some(value) = options.get(index) else {
                    return;
                };
                index += 1;
                if matches!(value.as_str(), "as" | "noop") {
                    return;
                }
                operands.push(ExpressionOperand::bit(value));
            }
            "as" => index += 1,
            _ => return,
        }
    }
}

fn block_translate(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    let mut index = 0;
    let mut seen = Vec::new();
    while let Some(option) = bits.get(index) {
        index += 1;
        if seen.contains(&option.as_str()) {
            return;
        }
        seen.push(option.as_str());
        match option.as_str() {
            "with" | "count" => index += token_kwargs(&bits[index..], true, operands),
            "context" => {
                let Some(value) = bits.get(index) else {
                    return;
                };
                index += 1;
                let mut operand = ExpressionOperand::bit(value);
                operand.message = Some("\"context\" in '{tag}' tag expected exactly one argument.");
                operands.push(operand);
            }
            "trimmed" => {}
            "asvar" => index += 1,
            _ => return,
        }
    }
}

fn cache(bits: &[TagBit], operands: &mut Vec<ExpressionOperand>) {
    if bits.len() < 2 {
        return;
    }
    let mut bits = bits;
    if let [rest @ .., last] = bits
        && last.as_str().starts_with("using=")
    {
        operands.push(ExpressionOperand::suffix(last, "using=".len()));
        bits = rest;
    }
    if let [timeout, _fragment, vary_on @ ..] = bits {
        operands.push(ExpressionOperand::bit(timeout));
        operands.extend(vary_on.iter().map(ExpressionOperand::bit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tag bits for `arguments` as they appear after a 3-byte `{% ` prefix and a tag name.
    fn bits(arguments: &str) -> Vec<TagBit> {
        let mut offset = 10;
        arguments
            .split(' ')
            .map(|text| {
                let bit = TagBit::new(
                    text.to_string(),
                    Span::saturating_from_parts_usize(offset, text.len()),
                );
                offset += text.len() + 1;
                bit
            })
            .collect()
    }

    fn texts(grammar: TagExpressionGrammar, arguments: &str) -> Vec<String> {
        grammar
            .operands(&bits(arguments))
            .into_iter()
            .map(|operand| operand.text)
            .collect()
    }

    #[test]
    fn condition_skips_operators() {
        assert_eq!(
            texts(
                TagExpressionGrammar::Condition,
                "not a is not b and c not in d"
            ),
            ["a", "b", "c", "d"]
        );
    }

    #[test]
    fn cycle_reference_and_as_forms() {
        assert!(texts(TagExpressionGrammar::Cycle, "rows").is_empty());
        assert_eq!(
            texts(TagExpressionGrammar::Cycle, "a as rows"),
            ["a", "as", "rows"]
        );
        assert_eq!(
            texts(TagExpressionGrammar::Cycle, "a b as rows"),
            ["a", "b"]
        );
        assert_eq!(
            texts(TagExpressionGrammar::Cycle, "a b as rows silent"),
            ["a", "b"]
        );
    }

    #[test]
    fn lorem_count_follows_method_and_random() {
        assert_eq!(texts(TagExpressionGrammar::Lorem, "n w random"), ["n"]);
        assert!(texts(TagExpressionGrammar::Lorem, "random").is_empty());
    }

    #[test]
    fn cache_reads_backend_and_skips_fragment_name() {
        assert_eq!(
            texts(
                TagExpressionGrammar::Cache,
                "500 sidebar user using=backend"
            ),
            ["backend", "500", "user"]
        );
    }

    #[test]
    fn for_sequence_follows_in() {
        assert_eq!(
            texts(TagExpressionGrammar::For, "x in items reversed"),
            ["items"]
        );
        assert!(texts(TagExpressionGrammar::For, "x items").is_empty());
    }

    #[test]
    fn keyword_values_map_back_to_source() {
        let operands = TagExpressionGrammar::Url.operands(&bits("'home' page=p|x"));
        assert_eq!(operands[1].text, "p|x");
        assert_eq!(operands[1].span, Span::new(22, 3));
        assert_eq!(operands[1].source_span(2..3), Some(Span::new(24, 1)));
    }

    #[test]
    fn regroup_maps_only_the_grouper_to_source() {
        let operands =
            TagExpressionGrammar::Regroup.operands(&bits("people by gender|lower as groups"));
        assert_eq!(operands[1].text, "groups.gender|lower");
        assert_eq!(operands[1].source_span(14..19), Some(Span::new(27, 5)));
        assert_eq!(operands[1].source_span(0..6), None);
    }

    #[test]
    fn expression_filters_use_source_spans() {
        let filters =
            tag_expression_filters(TagExpressionGrammar::Filter, &bits("lower|default:x"));
        let names = filters
            .iter()
            .map(|filter| (filter.name.as_str(), filter.span, filter.arg.is_some()))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                ("lower", Span::new(10, 5), false),
                ("default", Span::new(16, 9), true)
            ]
        );
    }
}
