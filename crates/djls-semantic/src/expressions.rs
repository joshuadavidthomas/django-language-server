//! Tag arguments that a compile function consumes as template expressions.
//!
//! Extraction records each tag's Argument Consumption: which Tag Bits Django passes to
//! `parser.compile_filter()`, `FilterExpression`, or `kwarg_re`. This module maps that fact onto
//! one tag occurrence. It knows no tag names; a tag whose consumption extraction could not prove
//! is not checked.

mod filter_expression;

use djls_project::ArgumentConsumption;
use djls_project::AssignmentMode;
use djls_project::TagArgumentSyntax;
use djls_project::TagRule;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::FilterArgument;
use djls_templates::TagBit;
pub(crate) use filter_expression::parse_filter_expression;

use crate::scoping::ScopedTagFacts;
use crate::structure::ActiveTemplateTag;
use crate::tags::effective_tag_bits;
use crate::tags::is_python_word_key;

/// The arguments of one tag occurrence that Django compiles, grouped by how it compiles them.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ConsumedArguments<'a> {
    /// Text that Django passes to `parser.compile_filter()`.
    pub(crate) expressions: Vec<ExpressionOperand>,
    /// Runs of bits that Django parses as one `{% if %}`-style condition.
    pub(crate) conditions: Vec<&'a [TagBit]>,
}

impl ConsumedArguments<'_> {
    /// Filter uses inside the syntactically valid expressions.
    pub(crate) fn filters(&self) -> Vec<Filter> {
        let mut filters = Vec::new();
        for operand in &self.expressions {
            let Ok(parsed) = parse_filter_expression(&operand.text) else {
                continue;
            };
            for filter in parsed {
                let arg = filter.argument.map(|range| {
                    FilterArgument::new(
                        operand.text[range.clone()].to_string(),
                        operand.source_span(range),
                    )
                });
                filters.push(Filter {
                    name: filter.name,
                    arg,
                    span: operand.source_span(filter.range),
                });
            }
        }
        filters
    }
}

/// The consumed arguments of a tag occurrence, from its effective extracted rules.
///
/// A tag that an unresolved load could shadow has no known consumption: the definition that
/// compiles it may not be the one DJLS resolved.
pub(crate) fn consumed_arguments<'a>(
    facts: &ScopedTagFacts,
    tag: ActiveTemplateTag<'a>,
) -> ConsumedArguments<'a> {
    let Some(fact) = facts.for_tag(tag) else {
        return ConsumedArguments::default();
    };
    if fact.unknown_load_can_shadow {
        return ConsumedArguments::default();
    }
    let Some(rules) = fact.spec.as_ref().and_then(|spec| spec.extracted_rules()) else {
        return ConsumedArguments::default();
    };
    rule_consumed_arguments(rules, tag.bits)
}

fn rule_consumed_arguments<'a>(rules: &TagRule, bits: &'a [TagBit]) -> ConsumedArguments<'a> {
    let bits = effective_tag_bits(bits, rules.as_var.strips_suffix());
    let consumption = bit_consumption(rules, bits);

    let mut consumed = ConsumedArguments::default();
    let mut condition_start = None;
    for (index, (bit, consumption)) in bits.iter().zip(&consumption).enumerate() {
        match consumption {
            ArgumentConsumption::Condition => {
                condition_start.get_or_insert(index);
                continue;
            }
            ArgumentConsumption::Expression => {
                consumed.expressions.push(ExpressionOperand::suffix(bit, 0));
            }
            ArgumentConsumption::OptionalKeywordExpression => {
                consumed.expressions.push(ExpressionOperand::suffix(
                    bit,
                    kwarg_value_start(bit.as_str()),
                ));
            }
            ArgumentConsumption::Unknown
            | ArgumentConsumption::Binding
            | ArgumentConsumption::Raw => {}
        }
        if let Some(start) = condition_start.take() {
            consumed.conditions.push(&bits[start..index]);
        }
    }
    if let Some(start) = condition_start {
        consumed.conditions.push(&bits[start..]);
    }
    consumed
}

/// How the compile function consumes each argument of this occurrence.
///
/// Uses that hold on every accepting path combine with the uses implied by the argument syntax
/// and with the uses of the argument forms the occurrence matches. Several matching forms keep
/// only the uses they share, because Django may take either path.
fn bit_consumption(rules: &TagRule, bits: &[TagBit]) -> Vec<ArgumentConsumption> {
    let mut consumption = vec![ArgumentConsumption::Unknown; bits.len()];
    for consumed in &rules.argument_consumption {
        for index in consumed.bits.to_bits_range(bits.len()) {
            consumption[index] = consumption[index].meet(consumed.consumption);
        }
    }

    let implied = match &rules.argument_syntax {
        // `parse_bits()` compiles every argument through `token_kwargs()` or
        // `compile_filter()`, which is what `kwarg_re` describes.
        TagArgumentSyntax::Signature { .. } => {
            vec![ArgumentConsumption::OptionalKeywordExpression; bits.len()]
        }
        TagArgumentSyntax::Assignments { operand } => assignment_consumption(bits, operand.mode),
        TagArgumentSyntax::Forms { forms, .. } => {
            let mut matched = forms.iter().filter_map(|form| form.match_atoms(bits).ok());
            match matched.next() {
                Some(first) => {
                    let mut joined = atom_consumption(&first);
                    for atoms in matched {
                        for (joined, atom) in joined.iter_mut().zip(atom_consumption(&atoms)) {
                            *joined = joined.join(atom);
                        }
                    }
                    joined
                }
                None => Vec::new(),
            }
        }
        TagArgumentSyntax::Parameters(_) | TagArgumentSyntax::Unknown => Vec::new(),
    };
    for (consumption, implied) in consumption.iter_mut().zip(implied) {
        *consumption = consumption.meet(implied);
    }
    consumption
}

fn atom_consumption(atoms: &[&djls_project::TagArgumentPattern]) -> Vec<ArgumentConsumption> {
    atoms.iter().map(|atom| atom.consumption).collect()
}

/// The values Django's `token_kwargs()` compiles from the leading assignments.
fn assignment_consumption(bits: &[TagBit], mode: AssignmentMode) -> Vec<ArgumentConsumption> {
    let mut consumption = vec![ArgumentConsumption::Unknown; bits.len()];
    let is_modern = |bit: &TagBit| kwarg_value_start(bit.as_str()) > 0;
    if bits.first().is_some_and(is_modern) {
        for (bit, consumption) in bits.iter().zip(&mut consumption) {
            if !is_modern(bit) {
                break;
            }
            *consumption = ArgumentConsumption::OptionalKeywordExpression;
        }
    } else if mode == AssignmentMode::ModernOrLegacy {
        let mut index = 0;
        while bits.get(index + 1).is_some_and(|bit| bit.as_str() == "as")
            && bits.get(index + 2).is_some()
        {
            consumption[index] = ArgumentConsumption::Expression;
            index += 3;
            if bits.get(index).is_some_and(|bit| bit.as_str() == "and") {
                index += 1;
            } else {
                break;
            }
        }
    }
    consumption
}

/// Byte offset of the expression in a bit matched by Django's `kwarg_re`,
/// `(?:(\w+)=)?(.+)`: after a `name=` prefix when one is followed by a value.
fn kwarg_value_start(bit: &str) -> usize {
    match bit.split_once('=') {
        Some((name, value)) if !value.is_empty() && is_python_word_key(name) => name.len() + 1,
        Some(_) | None => 0,
    }
}

/// Text that Django passes to `parser.compile_filter()`: a bit, or the part after its keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpressionOperand {
    pub(crate) text: String,
    pub(crate) span: Span,
}

impl ExpressionOperand {
    fn suffix(bit: &TagBit, start: usize) -> Self {
        let text = bit.as_str()[start..].to_string();
        let span = Span::saturating_from_parts_usize(bit.span.start_usize() + start, text.len());
        Self { text, span }
    }

    /// Source span of a byte range of `text`.
    fn source_span(&self, range: std::ops::Range<usize>) -> Span {
        Span::saturating_from_parts_usize(self.span.start_usize() + range.start, range.len())
    }
}

#[cfg(test)]
mod tests {
    use djls_project::ArgumentFormCoverage;
    use djls_project::AsVar;
    use djls_project::AssignmentOperand;
    use djls_project::ConsumedTagBits;
    use djls_project::RemainderPolicy;
    use djls_project::SplitPosition;
    use djls_project::TagArgumentForm;
    use djls_project::TagArgumentPattern;
    use djls_project::TagArgumentPatternKind;
    use djls_project::TagBitSpan;
    use djls_project::UniqueKeyCardinality;

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

    fn consumed(
        first: SplitPosition,
        last: SplitPosition,
        consumption: ArgumentConsumption,
    ) -> ConsumedTagBits {
        ConsumedTagBits {
            bits: TagBitSpan { first, last },
            consumption,
        }
    }

    fn texts(rules: &TagRule, arguments: &str) -> Vec<String> {
        rule_consumed_arguments(rules, &bits(arguments))
            .expressions
            .into_iter()
            .map(|operand| operand.text)
            .collect()
    }

    #[test]
    fn spans_resolve_after_the_stripped_as_target() {
        // `{% url %}`: the view name, then optional `name=` arguments.
        let rules = TagRule {
            argument_consumption: vec![
                consumed(
                    SplitPosition::Forward(1),
                    SplitPosition::Forward(1),
                    ArgumentConsumption::Expression,
                ),
                consumed(
                    SplitPosition::Forward(2),
                    SplitPosition::Backward(1),
                    ArgumentConsumption::OptionalKeywordExpression,
                ),
            ],
            as_var: AsVar::Strip,
            ..TagRule::default()
        };

        assert_eq!(
            texts(&rules, "'home' 1 page=p|x a==b as link"),
            ["'home'", "1", "p|x", "=b"]
        );
        assert_eq!(texts(&rules, "'home' page="), ["'home'", "page="]);
    }

    #[test]
    fn keyword_values_map_back_to_source() {
        let rules = TagRule {
            argument_syntax: TagArgumentSyntax::Signature {
                parameters: Vec::new(),
                positional_only: 0,
                variadic_keyword: Some("kwargs".to_string()),
            },
            ..TagRule::default()
        };
        let bits = bits("page=p|default:x");
        let consumed = rule_consumed_arguments(&rules, &bits);

        assert_eq!(consumed.expressions[0].span, Span::new(15, 11));
        assert_eq!(
            consumed
                .filters()
                .iter()
                .map(|filter| (filter.name.as_str(), filter.span))
                .collect::<Vec<_>>(),
            [("default", Span::new(17, 9))]
        );
    }

    #[test]
    fn several_matching_forms_keep_only_shared_consumption() {
        let atom = |name: &str, kind, consumption| TagArgumentPattern {
            name: name.to_string(),
            kind,
            mismatch_message: None,
            consumption,
        };
        let form = |second| {
            TagArgumentForm::new(vec![
                atom(
                    "value",
                    TagArgumentPatternKind::Variable,
                    ArgumentConsumption::Expression,
                ),
                atom("name", TagArgumentPatternKind::Variable, second),
            ])
            .expect("fixed-width form is valid")
        };
        let rules = TagRule {
            argument_syntax: TagArgumentSyntax::Forms {
                forms: vec![
                    form(ArgumentConsumption::Expression),
                    form(ArgumentConsumption::Unknown),
                ],
                coverage: ArgumentFormCoverage::Partial,
                length_mismatch_message: None,
            },
            ..TagRule::default()
        };

        assert_eq!(texts(&rules, "a _b"), ["a"]);
    }

    #[test]
    fn assignments_compile_modern_and_legacy_values() {
        let rules = |mode| TagRule {
            argument_syntax: TagArgumentSyntax::Assignments {
                operand: AssignmentOperand {
                    mode,
                    cardinality: UniqueKeyCardinality::Any,
                    remainder: RemainderPolicy::Reject,
                    empty_message: None,
                    multiple_message: None,
                    remainder_message: None,
                },
            },
            ..TagRule::default()
        };

        assert_eq!(
            texts(&rules(AssignmentMode::ModernOrLegacy), "a=x|lower b=y _c"),
            ["x|lower", "y"]
        );
        assert_eq!(
            texts(&rules(AssignmentMode::ModernOrLegacy), "x as a and _y as b"),
            ["x", "_y"]
        );
        assert!(texts(&rules(AssignmentMode::Modern), "x as a").is_empty());
    }

    #[test]
    fn condition_runs_dispatch_together() {
        let rules = TagRule {
            argument_consumption: vec![
                consumed(
                    SplitPosition::Forward(1),
                    SplitPosition::Backward(3),
                    ArgumentConsumption::Condition,
                ),
                consumed(
                    SplitPosition::Backward(1),
                    SplitPosition::Backward(1),
                    ArgumentConsumption::Expression,
                ),
            ],
            ..TagRule::default()
        };
        let bits = bits("a and b then value");
        let consumed = rule_consumed_arguments(&rules, &bits);

        assert_eq!(
            consumed
                .conditions
                .iter()
                .map(|run| run.iter().map(TagBit::as_str).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            [["a", "and", "b"]]
        );
        assert_eq!(consumed.expressions[0].text, "value");
    }
}
