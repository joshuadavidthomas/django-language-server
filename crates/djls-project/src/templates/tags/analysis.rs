pub(crate) mod calls;
pub(crate) mod constants;
pub(crate) mod constraints;
pub(crate) mod exceptions;
pub(crate) mod expressions;
pub(crate) mod guards;
pub(crate) mod mutations;
pub(crate) mod native;
pub(crate) mod state;
pub(crate) mod statements;

use ruff_python_ast::BoolOp;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprSlice;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFunctionDef;

pub(crate) use self::calls::AbstractValueKey;
pub(crate) use self::state::AbstractValue;
pub(crate) use self::state::AssignmentCall;
pub(crate) use self::state::Env;
pub(crate) use self::statements::process_statements;
use crate::ast::ExprExt;
use crate::db::Db as ProjectDb;
use crate::python::PythonFunctionDefinition;
use crate::python::PythonSourceLookup;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::guards::ExtractedRuleFragment;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::ArgumentFormCoverage;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::ChoiceAt;
use crate::templates::tags::types::ExtractedDiagnosticMessage;
use crate::templates::tags::types::KnownOptions;
use crate::templates::tags::types::ParameterRequirement;
use crate::templates::tags::types::RemainderPolicy;
use crate::templates::tags::types::RequiredKeyword;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagArgument;
use crate::templates::tags::types::TagArgumentForm;
use crate::templates::tags::types::TagArgumentKind;
use crate::templates::tags::types::TagArgumentPattern;
use crate::templates::tags::types::TagArgumentPatternKind;
use crate::templates::tags::types::TagArgumentSyntax;
use crate::templates::tags::types::TagRule;
use crate::templates::tags::types::UniqueKeyCardinality;

/// Call-resolution context for the analysis.
///
/// Carries the immutable context needed to resolve helper function calls
/// (module functions list and Salsa database/file references). Does not
/// accumulate analysis results — those are returned via `AnalysisResult`.
///
/// When `db` and `file` are set (running under Salsa), `resolve_call`
/// delegates to `analyze_helper` — a Salsa tracked function with cycle
/// recovery and automatic memoization. When `None` (standalone extraction),
/// helper calls return `Unknown`.
pub(crate) struct TagSourceContext<'db> {
    pub lookup: PythonSourceLookup<'db>,
    pub function: PythonFunctionDefinition,
}

impl<'db> TagSourceContext<'db> {
    pub(crate) fn new(db: &'db dyn ProjectDb, function: PythonFunctionDefinition) -> Self {
        let lookup = PythonSourceLookup::for_definition(db, db.project(), &function);
        Self { lookup, function }
    }
}

pub(crate) struct CallContext<'ctx, 'db> {
    pub source: Option<&'ctx mut TagSourceContext<'db>>,
}

/// Results accumulated during statement processing.
///
/// Returned from `statements::process_statements` instead of being stored in a context.
/// This separates the accumulation of analysis results from the call-resolution
/// context that is threaded through the analysis. Constraints stay separate from
/// diagnostic messages because constraints come from guard conditions, while
/// messages come from the exception raised by a guard body.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AnalysisResult {
    pub constraints: ExtractedTagConstraints,
    pub diagnostic_messages: Vec<ExtractedDiagnosticMessage>,
    pub known_options: Option<KnownOptions>,
    pub argument_syntax: Option<TagArgumentSyntax>,
    pub assignment_call: Option<AssignmentCall>,
}

impl AnalysisResult {
    /// Merge another result into this one.
    ///
    /// Constraints are combined additively. For `known_options`, the other
    /// result's value wins if present (last write wins — matches the sequential
    /// processing order of statements).
    fn extend(&mut self, other: AnalysisResult) {
        self.constraints.extend(other.constraints);
        for message in other.diagnostic_messages {
            if !self.diagnostic_messages.contains(&message) {
                self.diagnostic_messages.push(message);
            }
        }
        if other.known_options.is_some() {
            self.known_options = other.known_options;
        }
        match (&mut self.argument_syntax, other.argument_syntax) {
            (None, syntax) => {
                self.argument_syntax = syntax;
                self.assignment_call = other.assignment_call;
            }
            (
                Some(TagArgumentSyntax::Assignments { operand: current }),
                Some(TagArgumentSyntax::Assignments { operand: next }),
            ) if self.assignment_call.is_some()
                && self.assignment_call == other.assignment_call
                && current.mode == next.mode =>
            {
                if matches!(
                    (current.cardinality, next.cardinality),
                    (
                        UniqueKeyCardinality::Any,
                        UniqueKeyCardinality::AtLeastOne | UniqueKeyCardinality::ExactlyOne
                    ) | (
                        UniqueKeyCardinality::AtLeastOne,
                        UniqueKeyCardinality::ExactlyOne
                    )
                ) {
                    if current.cardinality == UniqueKeyCardinality::Any {
                        current.empty_message = next.empty_message;
                    }
                    current.cardinality = next.cardinality;
                    current.multiple_message = next.multiple_message;
                }
                if current.remainder == RemainderPolicy::Continue
                    && next.remainder == RemainderPolicy::Reject
                {
                    current.remainder = next.remainder;
                    current.remainder_message = next.remainder_message;
                }
            }
            (Some(_), Some(_)) => {
                self.argument_syntax = Some(TagArgumentSyntax::Unknown);
                self.assignment_call = None;
            }
            (Some(_), None) => {}
        }
    }
}

impl From<ExtractedRuleFragment> for AnalysisResult {
    fn from(rule: ExtractedRuleFragment) -> Self {
        Self {
            constraints: rule.constraints,
            diagnostic_messages: rule.diagnostic_messages,
            known_options: None,
            argument_syntax: None,
            assignment_call: None,
        }
    }
}

/// Validated representation of a Django template tag compile function.
///
/// Ensures the function has at least two positional parameters (parser and token)
/// before analysis begins. Use `from_ast` to construct from a `StmtFunctionDef`.
struct CompileFunction<'a> {
    parser_param: &'a str,
    token_param: &'a str,
    body: &'a [Stmt],
}

impl<'a> CompileFunction<'a> {
    /// Extract a `CompileFunction` from an AST function definition.
    ///
    /// Returns `None` if the function has fewer than 2 positional parameters,
    /// since a valid Django compile function requires at least `parser` and `token`.
    fn from_ast(func: &'a StmtFunctionDef) -> Option<Self> {
        let params = &func.parameters;
        let parser_param = params.args.first()?.parameter.name.as_str();
        let token_param = params.args.get(1)?.parameter.name.as_str();
        Some(CompileFunction {
            parser_param,
            token_param,
            body: &func.body,
        })
    }
}

/// Analyze a compile function to extract argument constraints.
///
/// This is the main entry point for the analyzer. It tracks `token`
/// and `parser` parameters through the function body, extracting constraints
/// from `if condition: raise ...` guard patterns.
///
/// Helper function calls are resolved only when analysis runs with a Salsa
/// database and file context (see [`CallContext`]). In standalone mode
/// (no database), helper calls evaluate to `Unknown`. Bare builtin names also
/// remain unproven without module bindings.
#[must_use]
pub(crate) fn analyze_compile_function(func: &StmtFunctionDef) -> TagRule {
    analyze_compile_function_with_context(func, None, None)
}

/// Analyze a compile function with name-resolution facts from its parsed module.
///
/// This pure seam is for source-backed callers that do not have a Salsa file.
/// A detached function cannot prove whether its bare names still refer to
/// Python builtins and must use [`analyze_compile_function`] instead.
#[cfg(test)]
pub(crate) fn analyze_compile_function_in_module(
    module: &[Stmt],
    func: &StmtFunctionDef,
) -> TagRule {
    let bindings = constants::StaticBindings::from_module(module);
    analyze_compile_function_with_context(func, None, Some(&bindings))
}

pub(crate) fn analyze_compile_function_in_source(
    source: &mut TagSourceContext<'_>,
    func: &StmtFunctionDef,
) -> TagRule {
    let db = source.lookup.db();
    let bindings = constants::module_static_bindings(db, source.function.file());
    analyze_compile_function_with_context(func, Some(source), Some(bindings))
}

fn analyze_compile_function_with_context(
    func: &StmtFunctionDef,
    source: Option<&mut TagSourceContext<'_>>,
    static_bindings: Option<&constants::StaticBindings>,
) -> TagRule {
    let Some(compile_fn) = CompileFunction::from_ast(func) else {
        return TagRule::default();
    };

    let mut env = state::Env::for_compile_function(compile_fn.parser_param, compile_fn.token_param);
    if let Some(static_bindings) = static_bindings {
        constants::seed_static_bindings(static_bindings, func, &mut env);
    }
    let mut ctx = CallContext { source };

    let (result, _) = statements::process_statements(compile_fn.body, &mut env, &mut ctx);

    let argument_syntax = match result.argument_syntax {
        None | Some(TagArgumentSyntax::Unknown) => {
            if let Some(forms) = derive_finite_backward_keyword_forms(
                &env,
                &result.constraints.required_keywords,
                &result.constraints.arg_constraints,
            ) {
                forms
            } else {
                let arguments = extract_arg_names(
                    &env,
                    &result.constraints.required_keywords,
                    &[],
                    &result.constraints.arg_constraints,
                );
                if arguments.is_empty() {
                    TagArgumentSyntax::Unknown
                } else {
                    TagArgumentSyntax::Parameters(arguments)
                }
            }
        }
        Some(syntax) => syntax,
    };

    let as_var = if !matches!(&argument_syntax, TagArgumentSyntax::Assignments { .. })
        && supports_manual_as_var_strip(compile_fn.body)
    {
        AsVar::Strip
    } else {
        AsVar::Keep
    };
    TagRule {
        arg_constraints: result.constraints.arg_constraints,
        required_keywords: result.constraints.required_keywords,
        choice_at_constraints: result.constraints.choice_at_constraints,
        known_options: result.known_options,
        diagnostic_messages: if result.diagnostic_messages.is_empty() {
            None
        } else {
            Some(result.diagnostic_messages)
        },
        argument_syntax,
        as_var,
    }
}

fn supports_manual_as_var_strip(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::If(stmt_if) = stmt else {
            return false;
        };
        let Some(name) = body_strips_trailing_as_var(&stmt_if.body) else {
            return false;
        };

        condition_checks_manual_as_var(stmt_if.test.as_ref(), &name)
    })
}

fn body_strips_trailing_as_var(stmts: &[Stmt]) -> Option<String> {
    stmts.iter().find_map(|stmt| {
        let Stmt::Assign(StmtAssign { targets, value, .. }) = stmt else {
            return None;
        };
        let [target] = targets.as_slice() else {
            return None;
        };
        let target_name = target.name_target()?;

        let Expr::Subscript(ExprSubscript { value, slice, .. }) = value.as_ref() else {
            return None;
        };
        let source_name = value.name_target()?;
        if target_name != source_name {
            return None;
        }

        let Expr::Slice(ExprSlice {
            lower: None,
            upper: Some(upper),
            step: None,
            ..
        }) = slice.as_ref()
        else {
            return None;
        };
        (upper.negative_integer() == Some(2)).then(|| target_name.to_string())
    })
}

fn condition_checks_manual_as_var(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::And,
            values,
            ..
        }) => values
            .iter()
            .any(|value| condition_checks_manual_as_var(value, name)),
        Expr::Compare(compare) => comparison_is_as_keyword_check(compare, name),
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => false,
    }
}

fn comparison_is_as_keyword_check(compare: &ExprCompare, name: &str) -> bool {
    let [CmpOp::Eq] = &*compare.ops else {
        return false;
    };
    let [right] = &*compare.comparators else {
        return false;
    };
    let left = compare.left.as_ref();

    (subscript_is_negative_index(left, name, 2) && right.string_literal() == Some("as"))
        || (left.string_literal() == Some("as") && subscript_is_negative_index(right, name, 2))
}

fn subscript_is_negative_index(expr: &Expr, name: &str, index: usize) -> bool {
    let Expr::Subscript(ExprSubscript { value, slice, .. }) = expr else {
        return false;
    };
    value.name_target() == Some(name) && slice.negative_integer() == Some(index)
}

// Keep fallback grammars small enough to scan in snapshots and completion lists.
const MAX_DERIVED_ARGUMENT_FORMS: usize = 8;

fn derive_finite_backward_keyword_forms(
    env: &state::Env,
    required_keywords: &[RequiredKeyword],
    arg_constraints: &[ArgumentCountConstraint],
) -> Option<TagArgumentSyntax> {
    if !required_keywords
        .iter()
        .any(|keyword| matches!(keyword.position, SplitPosition::Backward(_)))
    {
        return None;
    }

    let total_lengths = finite_accepted_total_lengths(arg_constraints)?;
    let mut forms = Vec::with_capacity(total_lengths.len());
    let mut mapped_backward_keyword = false;

    for total_length in total_lengths {
        let argument_count = total_length.checked_sub(1)?;
        let mut literals = vec![None; argument_count];
        for keyword in required_keywords {
            let Some(argument_index) = keyword.position.to_bits_index(argument_count) else {
                continue;
            };
            mapped_backward_keyword |= matches!(keyword.position, SplitPosition::Backward(_));
            let value = keyword.value.as_str();
            if literals[argument_index].is_some_and(|existing| existing != value) {
                return None;
            }
            literals[argument_index] = Some(value);
        }

        let pattern = literals
            .into_iter()
            .enumerate()
            .map(|(argument_index, literal)| {
                let (name, kind) = literal.map_or_else(
                    || {
                        (
                            derived_argument_name(env, argument_index, argument_count),
                            TagArgumentPatternKind::Variable,
                        )
                    },
                    |literal| {
                        (
                            literal.to_string(),
                            TagArgumentPatternKind::Literal(literal.to_string()),
                        )
                    },
                );
                TagArgumentPattern {
                    name,
                    kind,
                    mismatch_message: None,
                }
            })
            .collect();

        let Ok(form) = TagArgumentForm::new(pattern) else {
            return None;
        };
        forms.push(form);
    }

    if !mapped_backward_keyword {
        return None;
    }

    // These forms are not terminal-path evidence. They are complete because the
    // finite count constraints exhaust every accepted length and each such length
    // has one fixed form. The corpus false-positive sweep checks the narrower
    // hypothesis that a backward keyword applies whenever its index resolves.
    Some(TagArgumentSyntax::Forms {
        forms,
        coverage: ArgumentFormCoverage::Complete,
        length_mismatch_message: None,
    })
}

fn derived_argument_name(env: &state::Env, argument_index: usize, argument_count: usize) -> String {
    env.iter()
        .filter_map(|(name, value)| match value {
            state::AbstractValue::SplitElement { index }
                if index.to_bits_index(argument_count) == Some(argument_index) =>
            {
                Some(name)
            }
            state::AbstractValue::Unknown
            | state::AbstractValue::Token
            | state::AbstractValue::Parser
            | state::AbstractValue::SplitResult(_)
            | state::AbstractValue::SplitElement { .. }
            | state::AbstractValue::SplitLength(_)
            | state::AbstractValue::Int(_)
            | state::AbstractValue::Str(_)
            | state::AbstractValue::SplitPredicate(_)
            | state::AbstractValue::AssignmentMap(_)
            | state::AbstractValue::AssignmentRemainder(_)
            | state::AbstractValue::Tuple(_) => None,
        })
        .min()
        .map_or_else(|| format!("arg{}", argument_index + 1), str::to_string)
}

fn finite_accepted_total_lengths(constraints: &[ArgumentCountConstraint]) -> Option<Vec<usize>> {
    let mut explicit_lengths: Option<Vec<usize>> = None;
    for constraint in constraints {
        let values = match constraint {
            ArgumentCountConstraint::Exact(length) => Some(vec![*length]),
            ArgumentCountConstraint::OneOf(lengths) => Some(lengths.clone()),
            ArgumentCountConstraint::Min(_) | ArgumentCountConstraint::Max(_) => None,
        };
        if let Some(values) = values {
            explicit_lengths = Some(match explicit_lengths {
                Some(mut lengths) => {
                    lengths.retain(|length| values.contains(length));
                    lengths
                }
                None => values,
            });
        }
    }

    let mut lengths =
        if let Some(lengths) = explicit_lengths {
            lengths
        } else {
            let lower = constraints
                .iter()
                .filter_map(|constraint| match constraint {
                    ArgumentCountConstraint::Exact(length)
                    | ArgumentCountConstraint::Min(length) => Some(*length),
                    ArgumentCountConstraint::OneOf(lengths) => lengths.iter().min().copied(),
                    ArgumentCountConstraint::Max(_) => None,
                })
                .max()?;
            let upper = constraints
                .iter()
                .filter_map(|constraint| match constraint {
                    ArgumentCountConstraint::Exact(length)
                    | ArgumentCountConstraint::Max(length) => Some(*length),
                    ArgumentCountConstraint::OneOf(lengths) => lengths.iter().max().copied(),
                    ArgumentCountConstraint::Min(_) => None,
                })
                .min()?;
            let count = upper.checked_sub(lower)?.checked_add(1)?;
            if count > MAX_DERIVED_ARGUMENT_FORMS {
                return None;
            }
            (lower..=upper).collect()
        };

    lengths.sort_unstable();
    lengths.dedup();
    lengths.retain(|length| {
        *length > 0
            && constraints.iter().all(|constraint| match constraint {
                ArgumentCountConstraint::Exact(expected) => length == expected,
                ArgumentCountConstraint::Min(minimum) => length >= minimum,
                ArgumentCountConstraint::Max(maximum) => length <= maximum,
                ArgumentCountConstraint::OneOf(expected) => expected.contains(length),
            })
    });

    (!lengths.is_empty() && lengths.len() <= MAX_DERIVED_ARGUMENT_FORMS).then_some(lengths)
}

/// Extract argument names from the environment after analysis.
///
/// Scans env bindings for `SplitElement` values to reconstruct positional
/// argument names. Combines with `RequiredKeyword` positions for literal args.
/// Falls back to generic `arg1`/`arg2` names.
///
/// This assumes all `SplitElement` values in the env represent genuine
/// positional tag arguments. The assumption holds because Django template
/// tag compilation functions use top-level tuple unpacking or indexed
/// access for argument extraction — not loop-based pop patterns. The one
/// exception (option loops like `while remaining: option = remaining.pop(0)`)
/// is handled by skipping body processing in the While arm of
/// `process_statement`, so the loop variable never enters the env.
pub(super) fn extract_arg_names(
    env: &state::Env,
    required_keywords: &[RequiredKeyword],
    choice_at_constraints: &[ChoiceAt],
    arg_constraints: &[ArgumentCountConstraint],
) -> Vec<TagArgument> {
    // Collect named positions from env: variable name → split_contents position
    let mut named_positions: Vec<(usize, String)> = Vec::new();

    for (name, value) in env.iter() {
        if let state::AbstractValue::SplitElement {
            index: crate::templates::tags::types::SplitPosition::Forward(pos),
        } = value
        {
            // Position 0 is the tag name, not a user argument.
            if *pos > 0 {
                named_positions.push((*pos, name.to_string()));
            }
        }
    }

    // Sort by (position, name) for deterministic output even when
    // multiple variables map to the same split_contents position
    named_positions.sort_by(|(pos_a, name_a), (pos_b, name_b)| {
        pos_a.cmp(pos_b).then_with(|| name_a.cmp(name_b))
    });
    // Deduplicate: if multiple vars at same position, keep the first (alphabetically)
    named_positions.dedup_by_key(|(pos, _)| *pos);

    // Determine how many arg positions to generate
    let max_from_env = named_positions.iter().map(|(p, _)| *p).max().unwrap_or(0);
    let max_from_keywords = required_keywords
        .iter()
        .filter_map(|rk| match rk.position {
            SplitPosition::Forward(n) if n > 0 => Some(n),
            SplitPosition::Forward(_) | SplitPosition::Backward(_) => None,
        })
        .max()
        .unwrap_or(0);
    let max_from_constraints = infer_max_position(arg_constraints);

    let min_pos = infer_min_position(arg_constraints);
    let max_pos = max_from_env
        .max(max_from_keywords)
        .max(max_from_constraints);

    if max_pos == 0 {
        return Vec::new();
    }

    let mut args = Vec::new();
    for pos in 1..=max_pos {
        let pos_split = SplitPosition::Forward(pos);
        let inferred_requirement = if min_pos.is_some_and(|min_pos| pos > min_pos) {
            ParameterRequirement::Optional
        } else {
            ParameterRequirement::Required
        };

        // Check if there's a required keyword or choice at this position.
        if let Some(rk) = required_keywords.iter().find(|rk| rk.position == pos_split) {
            args.push(TagArgument {
                name: rk.value.clone(),
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Literal(rk.value.clone()),
            });
            continue;
        }
        if let Some(choice) = choice_at_constraints
            .iter()
            .find(|choice| choice.position == pos_split)
        {
            args.push(TagArgument {
                name: format!("arg{pos}"),
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Choice(choice.values.clone()),
            });
            continue;
        }

        // Check if env has a named variable at this position
        if let Some((_, name)) = named_positions.iter().find(|(p, _)| *p == pos) {
            args.push(TagArgument {
                name: name.clone(),
                requirement: inferred_requirement,
                kind: TagArgumentKind::Variable,
            });
            continue;
        }

        // Fallback: generic name
        args.push(TagArgument {
            name: format!("arg{pos}"),
            requirement: inferred_requirement,
            kind: TagArgumentKind::Variable,
        });
    }

    args
}

/// Infer the maximum argument position from constraints.
///
/// Returns the highest position (in `split_contents` coordinates, including tag name).
fn infer_max_position(constraints: &[ArgumentCountConstraint]) -> usize {
    let mut max = 0;
    for c in constraints {
        let candidate = match c {
            ArgumentCountConstraint::Exact(n)
            | ArgumentCountConstraint::Min(n)
            | ArgumentCountConstraint::Max(n) => n.saturating_sub(1),
            ArgumentCountConstraint::OneOf(vals) => {
                vals.iter().copied().max().unwrap_or(0).saturating_sub(1)
            }
        };
        max = max.max(candidate);
    }
    max
}

/// Infer the minimum argument position from constraints.
///
/// Returns the known number of guaranteed argument positions (in `split_contents`
/// coordinates, excluding the tag name), or `None` when no constraint has a lower bound.
fn infer_min_position(constraints: &[ArgumentCountConstraint]) -> Option<usize> {
    constraints
        .iter()
        .filter_map(|constraint| match constraint {
            ArgumentCountConstraint::Exact(n) | ArgumentCountConstraint::Min(n) => {
                Some(n.saturating_sub(1))
            }
            ArgumentCountConstraint::Max(_) => None,
            ArgumentCountConstraint::OneOf(vals) => {
                vals.iter().copied().min().map(|n| n.saturating_sub(1))
            }
        })
        .max()
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::types::ArgumentFormCoverage;
    use crate::templates::tags::types::ExtractedMessageArg;
    use crate::templates::tags::types::ExtractedMessageTemplate;
    use crate::templates::tags::types::TagArgumentPatternKind;

    fn parameters(rule: &TagRule) -> &[TagArgument] {
        rule.argument_syntax
            .parameters()
            .expect("expected parameter syntax")
    }

    fn analyze_source(source: &str) -> TagRule {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .iter()
            .find_map(|statement| {
                if let Stmt::FunctionDef(function) = statement {
                    Some(function)
                } else {
                    None
                }
            })
            .expect("no function found");
        analyze_compile_function_in_module(&module.body, func)
    }

    #[test]
    fn detached_compile_function_does_not_assume_builtin_identity() {
        let parsed = parse_module(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 2:
        raise TemplateSyntaxError("bad")
"#,
        )
        .expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .iter()
            .find_map(|statement| {
                if let Stmt::FunctionDef(function) = statement {
                    Some(function)
                } else {
                    None
                }
            })
            .expect("no function found");

        let rule = analyze_compile_function(func);

        assert!(rule.arg_constraints.is_empty());
        assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown);
    }

    #[test]
    fn manual_as_var_suffix_pattern_strips_before_count_validation() {
        let rule = analyze_source(
            r#"
def now(parser, token):
    bits = token.split_contents()
    asvar = None
    if len(bits) == 4 and bits[-2] == "as":
        asvar = bits[-1]
        bits = bits[:-2]
    if len(bits) != 2:
        raise TemplateSyntaxError("'now' statement takes one argument")
    format_string = bits[1][1:-1]
"#,
        );

        assert_eq!(rule.as_var, AsVar::Strip);
        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn arg_names_from_tuple_unpack() {
        let rule = analyze_source(
            r"
def do_tag(parser, token):
    tag_name, item, connector, varname = token.split_contents()
    if len(tag_name) != 4:
        raise TemplateSyntaxError('err')
",
        );
        assert_eq!(parameters(&rule).len(), 3);
        assert_eq!(parameters(&rule)[0].name, "item");
        assert_eq!(parameters(&rule)[1].name, "connector");
        assert_eq!(parameters(&rule)[2].name, "varname");
    }

    #[test]
    fn arg_names_from_indexed_access() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise TemplateSyntaxError("err")
    format_string = bits[1]
    target = bits[3]
"#,
        );
        assert_eq!(parameters(&rule).len(), 3);
        assert_eq!(parameters(&rule)[0].name, "format_string");
        // Position 2 (split index 2) has no named var — should get generic name
        assert_eq!(parameters(&rule)[1].name, "arg2");
        assert_eq!(parameters(&rule)[2].name, "target");
    }

    #[test]
    fn arg_names_with_required_keyword() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise TemplateSyntaxError("err")
    if bits[2] != "as":
        raise TemplateSyntaxError("err")
    value = bits[1]
    varname = bits[3]
"#,
        );
        assert_eq!(parameters(&rule).len(), 3);
        assert_eq!(parameters(&rule)[0].name, "value");
        assert_eq!(parameters(&rule)[0].kind, TagArgumentKind::Variable);
        assert_eq!(parameters(&rule)[1].name, "as");
        assert_eq!(
            parameters(&rule)[1].kind,
            TagArgumentKind::Literal("as".to_string())
        );
        assert_eq!(parameters(&rule)[2].name, "varname");
        assert_eq!(parameters(&rule)[2].kind, TagArgumentKind::Variable);
    }

    #[test]
    fn arg_names_fallback_generic() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(parameters(&rule).len(), 3);
        assert_eq!(parameters(&rule)[0].name, "arg1");
        assert_eq!(parameters(&rule)[1].name, "arg2");
        assert_eq!(parameters(&rule)[2].name, "arg3");
    }

    #[test]
    fn min_and_max_make_arguments_after_the_minimum_optional() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 3 or len(bits) > 6:
        raise TemplateSyntaxError("err")
    first = bits[1]
    second = bits[2]
"#,
        );

        assert_eq!(
            parameters(&rule)
                .iter()
                .map(|argument| argument.requirement)
                .collect::<Vec<_>>(),
            vec![
                ParameterRequirement::Required,
                ParameterRequirement::Required,
                ParameterRequirement::Optional,
                ParameterRequirement::Optional,
                ParameterRequirement::Optional,
            ]
        );
    }

    #[test]
    fn named_position_after_the_minimum_is_optional() {
        let mut env = state::Env::default();
        env.set(
            "third".to_string(),
            state::AbstractValue::SplitElement {
                index: SplitPosition::Forward(3),
            },
        );

        let arguments = extract_arg_names(
            &env,
            &[],
            &[],
            &[
                ArgumentCountConstraint::Min(3),
                ArgumentCountConstraint::Max(4),
            ],
        );

        assert_eq!(arguments[2].name, "third");
        assert_eq!(arguments[2].requirement, ParameterRequirement::Optional);
    }

    #[test]
    fn exact_count_requires_every_synthesized_argument() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise TemplateSyntaxError("err")
"#,
        );

        assert_eq!(
            parameters(&rule)
                .iter()
                .map(|argument| argument.requirement)
                .collect::<Vec<_>>(),
            vec![
                ParameterRequirement::Required,
                ParameterRequirement::Required,
                ParameterRequirement::Required,
            ]
        );
    }

    #[test]
    fn one_of_makes_arguments_after_smallest_count_optional() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) not in (3, 5):
        raise TemplateSyntaxError("err")
"#,
        );

        assert_eq!(
            parameters(&rule)
                .iter()
                .map(|argument| argument.requirement)
                .collect::<Vec<_>>(),
            vec![
                ParameterRequirement::Required,
                ParameterRequirement::Required,
                ParameterRequirement::Optional,
                ParameterRequirement::Optional,
            ]
        );
    }

    #[test]
    fn max_only_keeps_every_synthesized_argument_required() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 3:
        raise TemplateSyntaxError("err")
"#,
        );

        assert_eq!(
            parameters(&rule)
                .iter()
                .map(|argument| argument.requirement)
                .collect::<Vec<_>>(),
            vec![
                ParameterRequirement::Required,
                ParameterRequirement::Required,
            ]
        );
    }

    #[test]
    fn minimum_position_uses_the_strongest_conjunctive_lower_bound() {
        assert_eq!(infer_min_position(&[]), None);
        assert_eq!(infer_min_position(&[ArgumentCountConstraint::Max(6)]), None);
        assert_eq!(
            infer_min_position(&[ArgumentCountConstraint::OneOf(Vec::new())]),
            None
        );
        assert_eq!(
            infer_min_position(&[
                ArgumentCountConstraint::Min(3),
                ArgumentCountConstraint::Max(6),
            ]),
            Some(2)
        );
        assert_eq!(
            infer_min_position(&[ArgumentCountConstraint::Exact(4)]),
            Some(3)
        );
        assert_eq!(
            infer_min_position(&[ArgumentCountConstraint::OneOf(vec![3, 5])]),
            Some(2)
        );
        assert_eq!(
            infer_min_position(&[
                ArgumentCountConstraint::OneOf(Vec::new()),
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Exact(4),
            ]),
            Some(3)
        );
    }

    #[test]
    fn finite_backward_keywords_derive_complete_fixed_forms() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 3 or len(bits) > 6:
        raise TemplateSyntaxError("bad count")
    context_name = bits[-1]
    if bits[-2] != "as":
        raise TemplateSyntaxError("expected as")
    if len(bits) >= 5:
        if bits[-4] != "for":
            raise TemplateSyntaxError("expected for")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("finite backward keywords should produce forms");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        let rendered = forms
            .iter()
            .map(|form| {
                form.pattern()
                    .iter()
                    .map(|argument| match &argument.kind {
                        TagArgumentPatternKind::Variable => format!("<{}>", argument.name),
                        TagArgumentPatternKind::Literal(literal) => literal.clone(),
                        TagArgumentPatternKind::Choice(_)
                        | TagArgumentPatternKind::VariableWidth { .. }
                        | TagArgumentPatternKind::VariableExcept(_) => {
                            panic!("derived form should contain only literals and variables")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                "as <context_name>",
                "<arg1> as <context_name>",
                "for <arg2> as <context_name>",
                "<arg1> for <arg3> as <context_name>",
            ]
        );
    }

    #[test]
    fn unbounded_backward_keyword_uses_parameters() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 3:
        raise TemplateSyntaxError("bad count")
    context_name = bits[-1]
    if bits[-2] != "as":
        raise TemplateSyntaxError("expected as")
"#,
        );

        assert!(matches!(
            rule.argument_syntax,
            TagArgumentSyntax::Parameters(_)
        ));
    }

    #[test]
    fn accepted_length_sets_are_finite_and_capped() {
        assert_eq!(
            finite_accepted_total_lengths(&[
                ArgumentCountConstraint::Min(3),
                ArgumentCountConstraint::Max(6),
            ]),
            Some(vec![3, 4, 5, 6])
        );
        assert_eq!(
            finite_accepted_total_lengths(&[ArgumentCountConstraint::Exact(4)]),
            Some(vec![4])
        );
        assert_eq!(
            finite_accepted_total_lengths(&[ArgumentCountConstraint::OneOf(vec![5, 3, 5])]),
            Some(vec![3, 5])
        );
        assert_eq!(
            finite_accepted_total_lengths(&[ArgumentCountConstraint::Min(3)]),
            None
        );
        assert_eq!(
            finite_accepted_total_lengths(&[ArgumentCountConstraint::Max(6)]),
            None
        );
        assert_eq!(finite_accepted_total_lengths(&[]), None);
        assert_eq!(
            finite_accepted_total_lengths(&[
                ArgumentCountConstraint::Min(1),
                ArgumentCountConstraint::Max(9),
            ]),
            None
        );
    }

    #[test]
    fn exhaustive_length_dispatch_retains_correlated_forms() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, value, mode = bits
    elif len(bits) == 5:
        tag, value, as_, target, mode = bits
        if as_ != "as":
            raise TemplateSyntaxError("expected as")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected correlated argument forms");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 2);
        assert_eq!(forms[0].pattern().len(), 2);
        assert_eq!(forms[1].pattern().len(), 4);
        assert_eq!(
            forms[1].pattern()[1].kind,
            TagArgumentPatternKind::Literal("as".to_string())
        );
        assert_eq!(forms[1].pattern()[2].name, "target");
    }

    #[test]
    fn sequential_syntax_dispatches_intersect_on_the_same_paths() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, mode, value = bits
    else:
        raise TemplateSyntaxError("bad count")

    if len(bits) == 3:
        if mode != "safe":
            raise TemplateSyntaxError("bad mode")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("both dispatches constrain the same accepted path");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert!(forms[0].match_full(&["safe", "value"]).is_ok());
        assert!(forms[0].match_full(&["unsafe", "value"]).is_err());
    }

    #[test]
    fn duplicate_length_elif_is_shadowed_by_the_rejecting_branch() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        raise TemplateSyntaxError("rejected first")
    elif len(bits) == 3:
        tag, mode, value = bits
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown);
    }

    #[test]
    fn nested_count_constraints_remove_contradictory_forms() {
        let guards = [
            "len(bits) != 4",
            "len(bits) < 4",
            "len(bits) > 2",
            "len(bits) not in (2, 4)",
        ];

        for guard in guards {
            let rule = analyze_source(&format!(
                r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        if {guard}:
            raise TemplateSyntaxError("contradictory count")
    else:
        raise TemplateSyntaxError("bad count")
"#
            ));

            assert_eq!(
                rule.argument_syntax,
                TagArgumentSyntax::Unknown,
                "guard should reject the enclosing width: {guard}"
            );
        }
    }

    #[test]
    fn nested_count_constraints_use_original_split_coordinates() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, first, second, third = bits
        bits.pop(0)
        if len(bits) != 3:
            raise TemplateSyntaxError("bad remaining count")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the original width satisfies the adjusted count");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern().len(), 3);
    }

    #[test]
    fn unknown_successful_length_branch_makes_known_forms_partial() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, first, second = bits
    elif len(bits) == 5:
        for bit in bits:
            consume(bit)
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the supported branch should remain available");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        assert_eq!(forms.len(), 2);
        assert_eq!(
            forms
                .iter()
                .map(|form| form.pattern().len())
                .collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn same_length_literal_dispatch_keeps_constraints_correlated() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, kind, value, mode = bits
        if kind == "first":
            if mode != "left":
                raise TemplateSyntaxError("expected left")
        elif kind == "second":
            if mode != "right":
                raise TemplateSyntaxError("expected right")
        else:
            raise TemplateSyntaxError("bad kind")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected same-length alternatives");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 2);
        assert_eq!(
            forms[0]
                .pattern()
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentPatternKind::Literal("first".to_string()),
                &TagArgumentPatternKind::Variable,
                &TagArgumentPatternKind::Literal("left".to_string()),
            ]
        );
        assert_eq!(
            forms[1]
                .pattern()
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentPatternKind::Literal("second".to_string()),
                &TagArgumentPatternKind::Variable,
                &TagArgumentPatternKind::Literal("right".to_string()),
            ]
        );
    }

    #[test]
    fn same_shape_branches_keep_atom_messages_on_their_execution_paths() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, kind, value, ending = bits
        if kind == "first":
            if ending != "done":
                raise TemplateSyntaxError("first branch: %s" % token.contents)
        elif kind == "second":
            if ending != "done":
                raise TemplateSyntaxError("second branch: %s" % token.contents)
        else:
            raise TemplateSyntaxError("bad kind")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected branch-specific forms");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert!(rule.diagnostic_messages.is_none());
        for (branch, template) in [
            ("first", "first branch: %s"),
            ("second", "second branch: %s"),
        ] {
            let form = forms
                .iter()
                .find(|form| {
                    form.pattern()[0].kind == TagArgumentPatternKind::Literal(branch.to_string())
                })
                .expect("branch form should be retained");
            assert_eq!(
                form.pattern()[2].mismatch_message,
                Some(ExtractedMessageTemplate::PercentFormat {
                    template: template.to_string(),
                    args: vec![ExtractedMessageArg::TokenContents],
                })
            );
        }
    }

    #[test]
    fn nested_literal_dispatch_processes_later_guards() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, mode, value, ending = bits
        if mode == "first":
            pass
        else:
            raise TemplateSyntaxError("bad mode")
        if ending != "last":
            raise TemplateSyntaxError("bad ending")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected correlated argument forms");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0]
                .pattern()
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentPatternKind::Literal("first".to_string()),
                &TagArgumentPatternKind::Variable,
                &TagArgumentPatternKind::Literal("last".to_string()),
            ]
        );
    }

    #[test]
    fn duplicate_nested_literal_elif_is_shadowed_by_the_rejecting_branch() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, mode, value = bits
        if mode == "blocked":
            raise TemplateSyntaxError("rejected first")
        elif mode == "blocked":
            pass
        else:
            raise TemplateSyntaxError("bad mode")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown);
    }

    #[test]
    fn cross_position_literal_elif_projects_negative_discriminator() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, mode, value = bits
        if mode == "blocked":
            raise TemplateSyntaxError("bad mode")
        elif value == "allowed":
            pass
        else:
            raise TemplateSyntaxError("bad value")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the negative and positive facts form one ordered pattern");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0].pattern()[0].kind,
            TagArgumentPatternKind::VariableExcept("blocked".to_string())
        );
        assert_eq!(
            forms[0].pattern()[1].kind,
            TagArgumentPatternKind::Literal("allowed".to_string())
        );
        assert!(forms[0].match_full(&["other", "allowed"]).is_ok());
        assert!(forms[0].match_full(&["blocked", "allowed"]).is_err());
        assert!(forms[0].match_full(&["other", "wrong"]).is_err());
    }

    #[test]
    fn conflicting_nested_literals_discard_the_impossible_branch() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, mode, value = bits
        if mode == "first":
            if mode == "second":
                pass
            else:
                raise TemplateSyntaxError("conflict")
        elif mode == "second":
            pass
        else:
            raise TemplateSyntaxError("bad mode")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected the feasible literal form");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0].pattern()[0].kind,
            TagArgumentPatternKind::Literal("second".to_string())
        );
    }

    #[test]
    fn returned_form_does_not_receive_tail_constraints() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, mode, value = bits
        if mode == "first":
            return Node()
        else:
            raise TemplateSyntaxError("bad mode")
        if value != "last":
            raise TemplateSyntaxError("bad value")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the returned path should produce a form");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0]
                .pattern()
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentPatternKind::Literal("first".to_string()),
                &TagArgumentPatternKind::Variable,
            ]
        );
    }

    #[test]
    fn unsupported_boolean_operand_does_not_invent_a_complete_form() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, value, mode = bits
        if mode != "left" or runtime_check(value):
            raise TemplateSyntaxError("bad mode")
    elif len(bits) == 4:
        tag, first, second, third = bits
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the supported branch should remain available");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern().len(), 3);
    }

    #[test]
    fn method_call_guard_does_not_invent_a_literal_form() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, value, mode = bits
        if mode.lower() != "left":
            raise TemplateSyntaxError("bad mode")
    elif len(bits) == 4:
        tag, first, second, third = bits
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the supported branch should remain available");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern().len(), 3);
    }

    #[test]
    fn divergent_dispatch_mutations_do_not_taint_downstream_guards() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 3:
        tag, first, second = bits
        bits.pop(0)
    elif len(bits) == 5:
        tag, first, second, third, fourth = bits
        bits = bits[2:]
    else:
        raise TemplateSyntaxError("bad count")
    if len(bits) < 2:
        raise TemplateSyntaxError("bad remaining count")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("expected correlated argument forms");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 2);
        assert!(
            rule.arg_constraints.is_empty(),
            "a downstream guard cannot use a branch-specific split mutation: {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn nested_with_return_does_not_receive_tail_literal() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 2:
        with manager():
            return Node()
    else:
        raise TemplateSyntaxError("bad count")
    if bits[1] != "tail":
        raise TemplateSyntaxError("bad tail")
"#,
        );
        let (forms, coverage) = rule.argument_syntax.forms().expect("returned form");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern()[0].kind, TagArgumentPatternKind::Variable);
        assert!(forms[0].match_full(&["other"]).is_ok());
    }

    #[test]
    fn nested_loop_and_match_returns_remain_separate_from_tail_guards() {
        for suite in [
            "for value in runtime_values:\n            return Node()",
            "while runtime_check():\n            return Node()",
            "match runtime_value:\n            case 1:\n                return Node()\n            case _:\n                pass",
        ] {
            let source = format!(
                "def do_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) == 2:\n        {suite}\n    else:\n        raise TemplateSyntaxError('bad count')\n    if bits[1] != 'tail':\n        raise TemplateSyntaxError('bad tail')\n"
            );
            let rule = analyze_source(&source);
            let (forms, _) = rule.argument_syntax.forms().expect("known accepted forms");
            assert!(
                forms.iter().any(|form| form.match_full(&["other"]).is_ok()),
                "a return inside the nested suite must bypass the tail: {source}"
            );
            assert!(
                forms.iter().any(|form| form.match_full(&["tail"]).is_ok()),
                "the non-returning nested path must still satisfy the tail: {source}"
            );
        }
    }

    #[test]
    fn exact_width_prunes_out_of_range_and_aliased_literal_paths() {
        for body in [
            "if bits[3] == 'x':\n            pass\n        else:\n            raise TemplateSyntaxError('bad')",
            "if bits[1] == 'x':\n            if bits[-2] == 'y':\n                pass\n            else:\n                raise TemplateSyntaxError('bad alias')\n        else:\n            raise TemplateSyntaxError('bad first')",
        ] {
            let source = format!(
                "def do_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) == 2:\n        {body}\n    else:\n        raise TemplateSyntaxError('bad count')\n"
            );
            let rule = analyze_source(&source);
            assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown, "{source}");
        }
    }

    #[test]
    fn compiled_filter_object_is_not_treated_as_its_source_bit() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 2:
        value = parser.compile_filter(bits[1])
        if value == "raw":
            return Node()
        else:
            raise TemplateSyntaxError("bad value")
    else:
        raise TemplateSyntaxError("bad count")
"#,
        );
        let (forms, _) = rule.argument_syntax.forms().expect("known count form");
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern()[0].kind, TagArgumentPatternKind::Variable);
    }

    #[test]
    fn form_expansion_is_bounded_and_marked_partial() {
        let mut dispatches = String::new();
        for position in 1..=7 {
            write!(
                dispatches,
                "if bits[{position}] == 'left{position}':\n    pass\nelse:\n    pass\n"
            )
            .expect("writing to a String should succeed");
        }
        let source = format!(
            "def do_tag(parser, token):\n    bits = token.split_contents()\n    if len(bits) == 8:\n        tag, a, b, c, d, e, f, g = bits\n        {}\n    else:\n        raise TemplateSyntaxError('bad count')\n",
            dispatches.replace('\n', "\n        ").trim_end()
        );
        let rule = analyze_source(&source);

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("bounded known forms should remain available");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        // One slot retains the rejecting destination summary; the total path
        // budget remains 64 rather than granting 64 states per destination.
        assert_eq!(forms.len(), 63);
    }

    #[test]
    fn fixed_tuple_for_loop_executes_every_iteration() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    for _ in (1, 2):
        bits.pop(0)
    if len(bits) == 2:
        return Node()
    raise TemplateSyntaxError("bad")
"#,
        );

        let (forms, coverage) = rule
            .argument_syntax
            .forms()
            .expect("the two iterations establish one accepted width");
        assert_eq!(coverage, ArgumentFormCoverage::Complete);
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].pattern().len(), 3);
    }

    #[test]
    fn unknown_loop_remainder_keeps_later_form_tracking_partial() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    while runtime_check():
        bits.pop(0)
    if len(bits) == 2:
        return Node()
    raise TemplateSyntaxError("bad")
"#,
        );

        let (_, coverage) = rule
            .argument_syntax
            .forms()
            .expect("bounded loop paths still retain useful width evidence");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
    }

    #[test]
    fn accepting_match_wildcard_prevents_literal_constraint() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case ["tag", "special"]:
            return Node()
        case _:
            return Node()
"#,
        );

        assert!(rule.arg_constraints.is_empty());
        assert!(rule.required_keywords.is_empty());
    }

    #[test]
    fn supported_match_guard_retains_its_static_constraints() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case ["tag", mode] if mode == "safe":
            return Node()
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );

        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
        let (forms, coverage) = rule.argument_syntax.forms().expect("known guarded form");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0].pattern()[0].kind,
            TagArgumentPatternKind::Literal("safe".to_string())
        );
    }

    #[test]
    fn unsupported_split_writes_and_escapes_discard_stale_width() {
        for operation in [
            "bits[:] = ['tag', 'forced']",
            "[first, *bits] = ['tag', 'forced']",
            "replacement = bits = ['tag', 'forced']",
            "replace_contents(bits)",
            "replace_contents((bits,))",
            "replace_contents([bits])",
            "alias, = (bits,)\n    replace_contents(alias)",
        ] {
            let rule = analyze_source(&format!(
                "def do_tag(parser, token):\n    bits = token.split_contents()\n    {operation}\n    if len(bits) != 2:\n        raise TemplateSyntaxError('bad')\n    return Node()\n"
            ));

            assert_eq!(
                rule.argument_syntax,
                TagArgumentSyntax::Unknown,
                "operation retained stale split facts: {operation}"
            );
            assert!(rule.arg_constraints.is_empty(), "operation: {operation}");
        }
    }

    #[test]
    fn scalar_reads_and_string_join_do_not_escape_the_split_list() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    value = bits[1]
    consume_scalar(value)
    joined = " ".join(bits)
    if len(bits) != 2:
        raise TemplateSyntaxError("bad")
    return Node(joined)
"#,
        );

        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn pass_only_try_does_not_make_handler_return_reachable() {
        let rule = analyze_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    try:
        pass
    except ValueError:
        return Node()
    if len(bits) != 2:
        raise TemplateSyntaxError("bad")
    return Node()
"#,
        );

        assert_eq!(
            rule.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn wagtail_include_block_keeps_argument_evidence_across_option_branch() {
        let rule = analyze_source(
            r#"
def include_block(parser, token):
    tokens = token.split_contents()
    try:
        tag_name = tokens.pop(0)
        block_var_token = tokens.pop(0)
    except IndexError:
        raise TemplateSyntaxError("requires one argument")
    block_var = parser.compile_filter(block_var_token)
    if tokens and tokens[0] == "with":
        tokens.pop(0)
        extra_context = token_kwargs(tokens, parser)
    else:
        extra_context = None
    return Node(block_var, extra_context)
"#,
        );
        let TagArgumentSyntax::Parameters(parameters) = &rule.argument_syntax else {
            panic!("{rule:#?}");
        };
        assert!(
            parameters
                .iter()
                .any(|parameter| parameter.name == "block_var_token"),
            "{rule:#?}"
        );
    }

    #[test]
    fn arg_names_empty_when_no_constraints() {
        let rule = analyze_source(
            r"
def do_tag(parser, token):
    pass
",
        );
        assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown);
    }
}
