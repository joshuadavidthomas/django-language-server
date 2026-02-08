//! Expression evaluation and statement processing for the dataflow analyzer.

mod effects;
mod expressions;
mod match_arms;
mod statements;

pub use expressions::eval_expr;
pub use statements::process_statements;

use crate::dataflow::calls::HelperCache;

/// Context for the dataflow analysis, threading through shared state.
pub struct AnalysisContext<'a> {
    pub module_funcs: &'a [&'a StmtFunctionDef],
    pub caller_name: &'a str,
    pub call_depth: usize,
    pub cache: &'a mut HelperCache,
    pub known_options: Option<crate::types::KnownOptions>,
    pub constraints: super::constraints::Constraints,
}

use ruff_python_ast::StmtFunctionDef;

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::dataflow::calls::HelperCache;
    use crate::dataflow::domain::AbstractValue;
    use crate::dataflow::domain::Env;
    use crate::dataflow::domain::Index;

    fn parse_function(source: &str) -> StmtFunctionDef {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        for stmt in module.body {
            if let Stmt::FunctionDef(func_def) = stmt {
                return func_def;
            }
        }
        panic!("no function definition found in source");
    }

    fn eval_body(source: &str) -> Env {
        let func = parse_function(source);
        let parser_param = func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());
        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut cache = HelperCache::new();
        let mut ctx = AnalysisContext {
            module_funcs: &[],
            caller_name: "test",
            call_depth: 0,
            cache: &mut cache,
            known_options: None,
            constraints: crate::dataflow::constraints::Constraints::default(),
        };
        process_statements(&func.body, &mut env, &mut ctx);
        env
    }

    #[test]
    fn env_initialization() {
        let source = "def do_tag(parser, token): pass";
        let func = parse_function(source);
        let env = Env::for_compile_function(
            func.parameters.args[0].parameter.name.as_str(),
            func.parameters.args[1].parameter.name.as_str(),
        );
        assert_eq!(env.get("parser"), &AbstractValue::Parser);
        assert_eq!(env.get("token"), &AbstractValue::Token);
        assert_eq!(env.get("nonexistent"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_contents_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn contents_split_binding() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    args = token.contents.split()
",
        );
        assert_eq!(
            env.get("args"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn parser_token_split_contents() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = parser.token.split_contents()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn subscript_forward() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits[0]
    item = bits[2]
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: Index::Forward(2)
            }
        );
    }

    #[test]
    fn subscript_negative() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits[-1]
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
            }
        );
    }

    #[test]
    fn slice_from_start() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn slice_with_existing_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[2:]
    more = rest[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult {
                base_offset: 2,
                pops_from_end: 0
            }
        );
        assert_eq!(
            env.get("more"),
            &AbstractValue::SplitResult {
                base_offset: 3,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn len_of_split_result() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn list_wrapping() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = list(bits)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn star_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, *rest = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn tuple_unpack() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    a, b, c = (1, 'x', None)
",
        );
        assert_eq!(env.get("a"), &AbstractValue::Int(1));
        assert_eq!(env.get("b"), &AbstractValue::Str("x".to_string()));
        assert_eq!(env.get("c"), &AbstractValue::Unknown);
    }

    #[test]
    fn contents_split_none_1() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, rest = token.contents.split(None, 1)
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(env.get("rest"), &AbstractValue::Unknown);
    }

    #[test]
    fn unknown_variable() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    x = some_function()
",
        );
        assert_eq!(env.get("x"), &AbstractValue::Unknown);
    }

    #[test]
    fn split_result_tuple_unpack_no_star() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    tag_name, item, connector, varname = token.split_contents()
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("item"),
            &AbstractValue::SplitElement {
                index: Index::Forward(1)
            }
        );
        assert_eq!(
            env.get("connector"),
            &AbstractValue::SplitElement {
                index: Index::Forward(2)
            }
        );
        assert_eq!(
            env.get("varname"),
            &AbstractValue::SplitElement {
                index: Index::Forward(3)
            }
        );
    }

    #[test]
    fn subscript_with_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    rest = bits[1:]
    second = rest[0]
",
        );
        assert_eq!(
            env.get("second"),
            &AbstractValue::SplitElement {
                index: Index::Forward(1)
            }
        );
    }

    #[test]
    fn if_branch_updates_env() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    if True:
        rest = bits[1:]
",
        );
        assert_eq!(
            env.get("rest"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn integer_literal() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 42
",
        );
        assert_eq!(env.get("n"), &AbstractValue::Int(42));
    }

    #[test]
    fn string_literal() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    s = "hello"
"#,
        );
        assert_eq!(env.get("s"), &AbstractValue::Str("hello".to_string()));
    }

    #[test]
    fn slice_truncation_preserves_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = bits[1:]
    truncated = bits[:3]
",
        );
        assert_eq!(
            env.get("truncated"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn star_unpack_with_trailing() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    first, *middle, last = token.split_contents()
",
        );
        assert_eq!(
            env.get("first"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        // middle = original[1:-1], so base_offset=1 and pops_from_end=1
        // (the trailing `last` element is accounted for in pops_from_end)
        assert_eq!(
            env.get("middle"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 1
            }
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
            }
        );
    }

    #[test]
    fn pop_0_offset() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn pop_0_with_assignment() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits.pop(0)
",
        );
        assert_eq!(
            env.get("tag_name"),
            &AbstractValue::SplitElement {
                index: Index::Forward(0)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn pop_from_end() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 1
            }
        );
    }

    #[test]
    fn pop_from_end_with_assignment() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    last = bits.pop()
",
        );
        assert_eq!(
            env.get("last"),
            &AbstractValue::SplitElement {
                index: Index::Backward(1)
            }
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 1
            }
        );
    }

    #[test]
    fn multiple_pops() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    bits.pop()
    bits.pop()
",
        );
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 2
            }
        );
    }

    #[test]
    fn len_after_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn len_after_end_pop() {
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
    bits.pop()
    n = len(bits)
",
        );
        assert_eq!(
            env.get("n"),
            &AbstractValue::SplitLength {
                base_offset: 0,
                pops_from_end: 2
            }
        );
    }

    fn analyze(source: &str) -> crate::types::TagRule {
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
        crate::dataflow::analyze_compile_function(&func, &[])
    }

    #[test]
    fn option_loop_basic() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(opts.allow_duplicates);
    }

    #[test]
    fn option_loop_with_duplicate_check() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining_bits = bits[2:]
    seen = set()
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option in seen:
            raise TemplateSyntaxError("duplicate option")
        elif option == "silent":
            pass
        elif option == "cache":
            pass
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["silent".to_string(), "cache".to_string()]);
        assert!(opts.rejects_unknown);
        assert!(!opts.allow_duplicates);
    }

    #[test]
    fn option_loop_allows_unknown() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while remaining:
        option = remaining.pop(0)
        if option == "noescape":
            pass
        elif option == "trimmed":
            pass
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(
            opts.values,
            vec!["noescape".to_string(), "trimmed".to_string()]
        );
        assert!(!opts.rejects_unknown);
        assert!(opts.allow_duplicates);
    }

    #[test]
    fn option_loop_include_pattern() {
        let rule = analyze(
            r#"
def do_include(parser, token):
    bits = token.split_contents()
    options = {}
    remaining_bits = bits[2:]
    while remaining_bits:
        option = remaining_bits.pop(0)
        if option == "with":
            value = remaining_bits.pop(0)
        elif option == "only":
            options["only"] = True
        else:
            raise TemplateSyntaxError("unknown option")
"#,
        );
        let opts = rule.known_options.expect("should have known_options");
        assert_eq!(opts.values, vec!["with".to_string(), "only".to_string()]);
        assert!(opts.rejects_unknown);
    }

    #[test]
    fn no_option_loop_returns_none() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(rule.known_options.is_none());
    }

    #[test]
    fn match_partialdef_pattern() {
        let rule = analyze(
            r#"
def partialdef_func(parser, token):
    match token.split_contents():
        case "partialdef", partial_name, "inline":
            inline = True
        case "partialdef", partial_name, _:
            raise TemplateSyntaxError("bad")
        case "partialdef", partial_name:
            inline = False
        case ["partialdef"]:
            raise TemplateSyntaxError("bad")
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::OneOf(vec![2, 3])),
            "expected OneOf([2, 3]), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_partial_exact() {
        let rule = analyze(
            r#"
def partial_func(parser, token):
    match token.split_contents():
        case "partial", partial_name:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::Exact(2)),
            "expected Exact(2), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_non_split_result_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    x = something()
    match x:
        case "a":
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "non-SplitResult match should produce no constraints"
        );
    }

    #[test]
    fn match_star_pattern_variable_length() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", *rest:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::Min(1)),
            "expected Min(1), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_multiple_valid_lengths() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a:
            pass
        case "tag", a, b, c:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints
                .contains(&crate::types::ArgumentCountConstraint::OneOf(vec![2, 4])),
            "expected OneOf([2, 4]), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_all_error_cases_no_constraints() {
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag":
            raise TemplateSyntaxError("bad")
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        assert!(
            rule.arg_constraints.is_empty(),
            "all-error match should produce no constraints, got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_wildcard_overrides_variable_min_to_zero() {
        // When a Variable arm (min_len=2) appears before a non-error Wildcard,
        // the wildcard should unconditionally set the minimum to 0 since it
        // matches anything including zero-length inputs.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, *rest:
            pass
        case _:
            pass
"#,
        );
        // Wildcard `case _:` is a valid (non-error) arm that matches anything,
        // so there should be no Min constraint at all (min is effectively 0).
        assert!(
            !rule
                .arg_constraints
                .iter()
                .any(|c| matches!(c, crate::types::ArgumentCountConstraint::Min(_))),
            "wildcard should override variable min to 0 (no Min constraint), got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_wildcard_after_fixed_produces_no_min() {
        // A non-error wildcard means any length is valid, so even fixed-length
        // arms shouldn't produce exact/range constraints when a wildcard is present.
        let rule = analyze(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", a, b:
            pass
        case _:
            pass
"#,
        );
        // The wildcard is non-error, so it acts as a variable-length catch-all.
        // With min=0, no Min constraint should be emitted.
        assert!(
            !rule
                .arg_constraints
                .iter()
                .any(|c| matches!(c, crate::types::ArgumentCountConstraint::Min(m) if *m > 0)),
            "non-error wildcard should prevent Min constraint > 0, got {:?}",
            rule.arg_constraints
        );
    }

    #[test]
    fn match_env_updates_propagate() {
        let env = eval_body(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", name:
            result = name
"#,
        );
        // The match body should have processed assignments
        assert_eq!(env.get("result"), &AbstractValue::Unknown);
    }

    #[test]
    fn while_body_assignments_propagate() {
        // Non-option while loop: body should be processed for env updates
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[1:]
    while some_condition:
        val = remaining.pop(0)
",
        );
        // The pop(0) assignment inside the while body should be processed
        assert_eq!(
            env.get("val"),
            &AbstractValue::SplitElement {
                index: Index::Forward(1)
            }
        );
        // The pop(0) side effect should also mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult {
                base_offset: 2,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn while_body_pop_side_effects() {
        // Non-option while loop: pop side effects should be tracked
        let env = eval_body(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while some_condition:
        remaining.pop(0)
",
        );
        // The pop(0) inside the while body should mutate `remaining`
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult {
                base_offset: 3,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn contents_split_none_2_is_not_tuple() {
        // split(None, 2) should NOT be modeled as a 2-tuple;
        // only split(None, 1) has the special 2-tuple treatment.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 2)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn contents_split_none_0_is_not_tuple() {
        // split(None, 0) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    result = token.contents.split(None, 0)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn contents_split_none_variable_is_not_tuple() {
        // split(None, some_var) should NOT be modeled as a 2-tuple.
        let env = eval_body(
            r"
def do_tag(parser, token):
    n = 1
    result = token.contents.split(None, n)
",
        );
        assert_eq!(
            env.get("result"),
            &AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn while_option_loop_skips_body_processing() {
        // Option loop pattern: body should NOT be processed to avoid
        // the loop variable appearing as a false positional argument
        let env = eval_body(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    remaining = bits[2:]
    while remaining:
        option = remaining.pop(0)
        if option == "with":
            pass
        elif option == "only":
            pass
        else:
            raise TemplateSyntaxError("unknown")
"#,
        );
        // `option` should NOT have a SplitElement value since the
        // option loop body is not processed (to avoid false positives)
        assert_eq!(env.get("option"), &AbstractValue::Unknown);
        // `remaining` should keep its pre-loop value
        assert_eq!(
            env.get("remaining"),
            &AbstractValue::SplitResult {
                base_offset: 2,
                pops_from_end: 0
            }
        );
    }
}
