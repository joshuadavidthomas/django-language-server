pub(crate) mod calls;
pub(crate) mod constraints;
pub(crate) mod domain;
pub(crate) mod eval;

pub use calls::HelperCache;
use ruff_python_ast::StmtFunctionDef;

use crate::types::ArgumentCountConstraint;
use crate::types::ExtractedArg;
use crate::types::ExtractedArgKind;
use crate::types::RequiredKeyword;
use crate::types::TagRule;

/// Analyze a compile function using dataflow analysis to extract argument constraints.
///
/// This is the main entry point for the dataflow analyzer. It tracks `token`
/// and `parser` parameters through the function body, extracting constraints
/// from `raise TemplateSyntaxError(...)` guards.
///
/// `module_funcs` provides all function definitions in the same module, used
/// for bounded-depth inlining of helper function calls.
#[must_use]
pub fn analyze_compile_function(
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
) -> TagRule {
    let mut cache = HelperCache::new();
    analyze_compile_function_with_cache(func, module_funcs, &mut cache)
}

/// Analyze a compile function with an existing helper cache.
///
/// When analyzing multiple compile functions in the same module, passing
/// a shared cache avoids re-analyzing helpers called by multiple functions.
#[must_use]
pub fn analyze_compile_function_with_cache(
    func: &StmtFunctionDef,
    module_funcs: &[&StmtFunctionDef],
    cache: &mut HelperCache,
) -> TagRule {
    let params = &func.parameters;
    let parser_param = params
        .args
        .first()
        .map_or("parser", |p| p.parameter.name.as_str());
    let token_param = params
        .args
        .get(1)
        .map_or("token", |p| p.parameter.name.as_str());

    let mut env = domain::Env::for_compile_function(parser_param, token_param);
    let mut ctx = eval::AnalysisContext {
        module_funcs,
        caller_name: func.name.as_str(),
        call_depth: 0,
        cache,
        known_options: None,
        constraints: constraints::Constraints::default(),
    };

    eval::process_statements(&func.body, &mut env, &mut ctx);

    let extracted_args = extract_arg_names(
        &env,
        &ctx.constraints.required_keywords,
        &ctx.constraints.arg_constraints,
    );

    TagRule {
        arg_constraints: ctx.constraints.arg_constraints,
        required_keywords: ctx.constraints.required_keywords,
        choice_at_constraints: ctx.constraints.choice_at_constraints,
        known_options: ctx.known_options,
        extracted_args,
        supports_as_var: false,
    }
}

/// Extract argument names from the environment after dataflow analysis.
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
    env: &domain::Env,
    required_keywords: &[RequiredKeyword],
    arg_constraints: &[ArgumentCountConstraint],
) -> Vec<ExtractedArg> {
    // Collect named positions from env: variable name → split_contents position
    let mut named_positions: Vec<(usize, String)> = Vec::new();

    for (name, value) in env.iter() {
        if let domain::AbstractValue::SplitElement {
            index: domain::Index::Forward(pos),
        } = value
        {
            // Skip position 0 (tag name) and skip parser/token params
            if *pos > 0 && name != "parser" && name != "token" && name != "tag_name" {
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
        .filter_map(|rk| {
            if rk.position > 0 {
                usize::try_from(rk.position).ok()
            } else {
                None
            }
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
        let pos_i64 = i64::try_from(pos).unwrap_or(0);

        // Check if there's a required keyword at this position
        if let Some(rk) = required_keywords.iter().find(|rk| rk.position == pos_i64) {
            args.push(ExtractedArg {
                name: rk.value.clone(),
                required: true,
                kind: ExtractedArgKind::Literal(rk.value.clone()),
                position: pos - 1,
            });
            continue;
        }

        // Check if env has a named variable at this position
        if let Some((_, name)) = named_positions.iter().find(|(p, _)| *p == pos) {
            args.push(ExtractedArg {
                name: name.clone(),
                required: true,
                kind: ExtractedArgKind::Variable,
                position: pos - 1,
            });
            continue;
        }

        // Fallback: generic name
        args.push(ExtractedArg {
            name: format!("arg{pos}"),
            required: true,
            kind: ExtractedArgKind::Variable,
            position: pos - 1,
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
        analyze_compile_function(&func, &[])
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
