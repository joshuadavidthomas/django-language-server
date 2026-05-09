pub(crate) mod calls;
pub(crate) mod expressions;
pub(crate) mod match_arms;
pub(crate) mod mutations;
pub(crate) mod rules;
pub(crate) mod state;
pub(crate) mod statements;

pub(crate) use calls::extract_return_value;
pub use calls::AbstractValueKey;
use djls_source::File;
use ruff_python_ast::StmtFunctionDef;

use crate::analysis::rules::ConstraintSet;
use crate::types::ArgumentCountConstraint;
use crate::types::AsVar;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::KnownOptions;
use crate::types::RequiredKeyword;
use crate::types::SplitPosition;
use crate::types::TagRule;

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
pub struct CallContext<'a> {
    /// Salsa database, populated when running under `extract_module`.
    /// Used by `resolve_call` to call `analyze_helper` via Salsa.
    pub db: Option<&'a dyn djls_source::Db>,
    /// Source file being analyzed, used to construct `HelperCall` interned keys.
    pub file: Option<File>,
}

/// Results accumulated during statement processing.
///
/// Returned from `statements::process_statements` instead of being stored in a context.
/// This separates the accumulation of analysis results from the call-resolution
/// context that is threaded through the analysis.
#[derive(Default)]
pub struct AnalysisResult {
    pub constraints: ConstraintSet,
    pub known_options: Option<KnownOptions>,
}

impl AnalysisResult {
    /// Merge another result into this one.
    ///
    /// Constraints are combined additively. For `known_options`, the other
    /// result's value wins if present (last write wins — matches the sequential
    /// processing order of statements).
    pub fn extend(&mut self, other: AnalysisResult) {
        self.constraints.extend(other.constraints);
        if other.known_options.is_some() {
            self.known_options = other.known_options;
        }
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
    let Some(parser_param) = func.parameters.args.first() else {
        return TagRule::default();
    };
    let Some(token_param) = func.parameters.args.get(1) else {
        return TagRule::default();
    };

    let mut env = state::Env::for_compile_function(
        parser_param.parameter.name.as_str(),
        token_param.parameter.name.as_str(),
    );
    let mut ctx = CallContext {
        db: None,
        file: None,
    };

    let result = statements::process_statements(&func.body, &mut env, &mut ctx);

    let extracted_args = extract_arg_names(
        &env,
        &result.constraints.required_keywords,
        &result.constraints.arg_constraints,
        &[
            parser_param.parameter.name.to_string(),
            token_param.parameter.name.to_string(),
            "tag_name".to_string(),
        ],
    );

    TagRule {
        arg_constraints: result.constraints.arg_constraints,
        required_keywords: result.constraints.required_keywords,
        choice_at_constraints: result.constraints.choice_at_constraints,
        known_options: result.known_options,
        extracted_args,
        as_var: AsVar::Keep,
    }
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
fn extract_arg_names(
    env: &state::Env,
    required_keywords: &[RequiredKeyword],
    arg_constraints: &[ArgumentCountConstraint],
    ignored_names: &[String],
) -> Vec<ExtractedArg> {
    // Collect named positions from env: variable name → split_contents position
    let mut named_positions: Vec<(usize, String)> = Vec::new();

    for (name, value) in env.iter() {
        if let state::AbstractValue::SplitElement {
            index: crate::types::SplitPosition::Forward(pos),
        } = value
        {
            // Skip position 0 (tag name) and skip parser/token params
            if *pos > 0 && !ignored_names.iter().any(|ignored| ignored == name) {
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
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let max_from_constraints = arg_constraints
        .iter()
        .map(|constraint| match constraint {
            ArgumentCountConstraint::Exact(n)
            | ArgumentCountConstraint::Min(n)
            | ArgumentCountConstraint::Max(n) => n.saturating_sub(1),
            ArgumentCountConstraint::OneOf(vals) => {
                vals.iter().copied().max().unwrap_or(0).saturating_sub(1)
            }
        })
        .max()
        .unwrap_or(0);

    let max_pos = max_from_env
        .max(max_from_keywords)
        .max(max_from_constraints);

    if max_pos == 0 {
        return Vec::new();
    }

    let mut args = Vec::new();
    for pos in 1..=max_pos {
        let pos_split = SplitPosition::Forward(pos);
        let arg_index = pos_split
            .arg_index()
            .expect("Forward(pos) with pos >= 1 always has an arg_index");

        // Check if there's a required keyword at this position
        if let Some(rk) = required_keywords.iter().find(|rk| rk.position == pos_split) {
            args.push(ExtractedArg {
                name: rk.value.clone(),
                required: true,
                kind: ExtractedArgKind::Literal(rk.value.clone()),
                position: arg_index,
            });
            continue;
        }

        // Check if env has a named variable at this position
        if let Some((_, name)) = named_positions.iter().find(|(p, _)| *p == pos) {
            args.push(ExtractedArg {
                name: name.clone(),
                required: true,
                kind: ExtractedArgKind::Variable,
                position: arg_index,
            });
            continue;
        }

        // Fallback: generic name
        args.push(ExtractedArg {
            name: format!("arg{pos}"),
            required: true,
            kind: ExtractedArgKind::Variable,
            position: arg_index,
        });
    }

    args
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;

    fn analyze_source(source: &str) -> TagRule {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .into_iter()
            .find_map(|s| {
                if let Stmt::FunctionDef(f) = s {
                    Some(f)
                } else {
                    None
                }
            })
            .expect("no function found");
        analyze_compile_function(&func)
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
        assert_eq!(rule.extracted_args.len(), 3);
        assert_eq!(rule.extracted_args[0].name, "item");
        assert_eq!(rule.extracted_args[0].position, 0);
        assert_eq!(rule.extracted_args[1].name, "connector");
        assert_eq!(rule.extracted_args[1].position, 1);
        assert_eq!(rule.extracted_args[2].name, "varname");
        assert_eq!(rule.extracted_args[2].position, 2);
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
        assert_eq!(rule.extracted_args.len(), 3);
        assert_eq!(rule.extracted_args[0].name, "format_string");
        assert_eq!(rule.extracted_args[0].position, 0);
        // Position 2 (split index 2) has no named var — should get generic name
        assert_eq!(rule.extracted_args[1].name, "arg2");
        assert_eq!(rule.extracted_args[1].position, 1);
        assert_eq!(rule.extracted_args[2].name, "target");
        assert_eq!(rule.extracted_args[2].position, 2);
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
        assert_eq!(rule.extracted_args.len(), 3);
        assert_eq!(rule.extracted_args[0].name, "value");
        assert_eq!(rule.extracted_args[0].kind, ExtractedArgKind::Variable);
        assert_eq!(rule.extracted_args[1].name, "as");
        assert_eq!(
            rule.extracted_args[1].kind,
            ExtractedArgKind::Literal("as".to_string())
        );
        assert_eq!(rule.extracted_args[2].name, "varname");
        assert_eq!(rule.extracted_args[2].kind, ExtractedArgKind::Variable);
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
        assert_eq!(rule.extracted_args.len(), 3);
        assert_eq!(rule.extracted_args[0].name, "arg1");
        assert_eq!(rule.extracted_args[1].name, "arg2");
        assert_eq!(rule.extracted_args[2].name, "arg3");
    }

    #[test]
    fn arg_names_empty_when_no_constraints() {
        let rule = analyze_source(
            r"
def do_tag(parser, token):
    pass
",
        );
        assert!(rule.extracted_args.is_empty());
    }
}
