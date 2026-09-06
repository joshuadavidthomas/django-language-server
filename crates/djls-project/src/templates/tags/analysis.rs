pub(crate) mod calls;
pub(crate) mod constants;
pub(crate) mod constraints;
pub(crate) mod exceptions;
pub(crate) mod expressions;
pub(crate) mod forms;
pub(crate) mod guards;
pub(crate) mod match_arms;
pub(crate) mod mutations;
pub(crate) mod state;
pub(crate) mod statements;

use djls_source::File;
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
pub(crate) use self::state::Env;
pub(crate) use self::statements::process_statements;
use crate::ast::ExprExt;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::guards::ExtractedRuleFragment;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::AsVar;
use crate::templates::tags::types::ChoiceAt;
use crate::templates::tags::types::ExtractedDiagnosticMessage;
use crate::templates::tags::types::KnownOptions;
use crate::templates::tags::types::ParameterRequirement;
use crate::templates::tags::types::RequiredKeyword;
use crate::templates::tags::types::SplitPosition;
use crate::templates::tags::types::TagArgument;
use crate::templates::tags::types::TagArgumentKind;
use crate::templates::tags::types::TagArgumentSyntax;
use crate::templates::tags::types::TagRule;

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
pub(crate) struct CallContext<'a> {
    /// Salsa database, populated when running under tracked extraction.
    /// Used by `resolve_call` to call `analyze_helper` via Salsa.
    pub db: Option<&'a dyn djls_source::Db>,
    /// Source file being analyzed, used to construct `HelperCall` interned keys.
    pub file: Option<File>,
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
        match (&self.argument_syntax, other.argument_syntax) {
            (None, syntax) => self.argument_syntax = syntax,
            (Some(_), Some(_)) => {
                // Sequential syntax dispatches constrain the same call. Without
                // intersecting their forms, retaining either contract would
                // claim success paths that the other dispatch rejects.
                self.argument_syntax = Some(TagArgumentSyntax::Unknown);
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
/// (no database), helper calls evaluate to `Unknown`.
#[must_use]
pub(crate) fn analyze_compile_function(func: &StmtFunctionDef) -> TagRule {
    analyze_compile_function_with_context(func, None, None, None)
}

#[cfg(test)]
pub(crate) fn analyze_compile_function_in_module(
    module: &[Stmt],
    func: &StmtFunctionDef,
) -> TagRule {
    let bindings = constants::StaticBindings::from_module(module);
    analyze_compile_function_with_context(func, None, None, Some(&bindings))
}

pub(crate) fn analyze_compile_function_in_file(
    db: &dyn djls_source::Db,
    file: File,
    func: &StmtFunctionDef,
) -> TagRule {
    analyze_compile_function_with_context(
        func,
        Some(db),
        Some(file),
        Some(constants::module_static_bindings(db, file)),
    )
}

fn analyze_compile_function_with_context(
    func: &StmtFunctionDef,
    db: Option<&dyn djls_source::Db>,
    file: Option<File>,
    static_bindings: Option<&constants::StaticBindings>,
) -> TagRule {
    let Some(compile_fn) = CompileFunction::from_ast(func) else {
        return TagRule::default();
    };

    let mut env = state::Env::for_compile_function(compile_fn.parser_param, compile_fn.token_param);
    if let Some(static_bindings) = static_bindings {
        constants::seed_static_bindings(static_bindings, func, &mut env);
    }
    let mut ctx = CallContext { db, file };

    let (result, _) = statements::process_statements(compile_fn.body, &mut env, &mut ctx);

    let argument_syntax = result.argument_syntax.unwrap_or_else(|| {
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
    });

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
        as_var: if supports_manual_as_var_strip(compile_fn.body) {
            AsVar::Strip
        } else {
            AsVar::Keep
        },
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

    let max_pos = max_from_env
        .max(max_from_keywords)
        .max(max_from_constraints);

    if max_pos == 0 {
        return Vec::new();
    }

    let mut args = Vec::new();
    for pos in 1..=max_pos {
        let pos_split = SplitPosition::Forward(pos);

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
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Variable,
            });
            continue;
        }

        // Fallback: generic name
        args.push(TagArgument {
            name: format!("arg{pos}"),
            requirement: ParameterRequirement::Required,
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

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::types::ArgumentFormCoverage;

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
        assert_eq!(forms[0].arguments.len(), 2);
        assert_eq!(forms[1].arguments.len(), 4);
        assert_eq!(
            forms[1].arguments[1].kind,
            TagArgumentKind::Literal("as".to_string())
        );
        assert_eq!(forms[1].arguments[2].name, "target");
    }

    #[test]
    fn sequential_syntax_dispatches_become_unknown() {
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

        assert_eq!(rule.argument_syntax, TagArgumentSyntax::Unknown);
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
        assert_eq!(forms[0].arguments.len(), 3);
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
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].arguments.len(), 2);
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
                .arguments
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentKind::Literal("first".to_string()),
                &TagArgumentKind::Variable,
                &TagArgumentKind::Literal("left".to_string()),
            ]
        );
        assert_eq!(
            forms[1]
                .arguments
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentKind::Literal("second".to_string()),
                &TagArgumentKind::Variable,
                &TagArgumentKind::Literal("right".to_string()),
            ]
        );
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
                .arguments
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentKind::Literal("first".to_string()),
                &TagArgumentKind::Variable,
                &TagArgumentKind::Literal("last".to_string()),
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
    fn cross_position_literal_elif_is_partial() {
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
            .expect("the representable part of the elif should remain available");
        assert_eq!(coverage, ArgumentFormCoverage::Partial);
        assert_eq!(forms.len(), 1);
        assert_eq!(
            forms[0].arguments[1].kind,
            TagArgumentKind::Literal("allowed".to_string())
        );
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
            forms[0].arguments[0].kind,
            TagArgumentKind::Literal("second".to_string())
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
                .arguments
                .iter()
                .map(|argument| &argument.kind)
                .collect::<Vec<_>>(),
            vec![
                &TagArgumentKind::Literal("first".to_string()),
                &TagArgumentKind::Variable,
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
        assert_eq!(forms[0].arguments.len(), 3);
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
        assert_eq!(forms[0].arguments.len(), 3);
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
    if len(bits) < 4:
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
    fn form_expansion_is_bounded_and_marked_partial() {
        let mut dispatches = String::new();
        for position in 1..=7 {
            write!(
                dispatches,
                "if bits[{position}] == 'left{position}':\n    pass\nelif bits[{position}] == 'right{position}':\n    pass\nelse:\n    raise TemplateSyntaxError('bad')\n"
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
        assert_eq!(forms.len(), 64);
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
