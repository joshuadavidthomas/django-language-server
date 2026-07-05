use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::MatchCase;
use ruff_python_ast::Pattern;
use ruff_python_ast::PatternMatchAs;
use ruff_python_ast::PatternMatchSequence;
use ruff_python_ast::PatternMatchValue;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtMatch;

use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::constraints::ExtractedTagConstraints;
use crate::templates::tags::analysis::exceptions::direct_raise_exception;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::ArgumentCountConstraint;
use crate::templates::tags::types::RequiredKeyword;
use crate::templates::tags::types::SplitPosition;

/// Extract argument constraints from a match statement whose subject is a `SplitResult`.
///
/// Analyzes `match token.split_contents(): case ...:` patterns from Django 6.0+.
/// Collects valid case shapes (cases whose body does NOT raise an exception)
/// and derives argument count constraints and required keywords from them.
pub(super) fn extract_match_constraints(
    match_stmt: &StmtMatch,
    env: &mut Env,
) -> Option<ExtractedTagConstraints> {
    let subject = eval_expr(&match_stmt.subject, env);
    if !matches!(subject, AbstractValue::SplitResult(_)) {
        return None;
    }

    let mut valid_lengths: Vec<usize> = Vec::new();
    let mut has_variable_length = false;
    let mut min_variable_length: Option<usize> = None;

    for case in &match_stmt.cases {
        match classify_case_body(&case.body) {
            CaseBody::UnconditionalRaise => continue,
            CaseBody::MixedRaise => return None,
            CaseBody::NeverRaises => {}
        }

        match analyze_case_pattern(&case.pattern) {
            PatternShape::Fixed(len) => {
                if !valid_lengths.contains(&len) {
                    valid_lengths.push(len);
                }
            }
            PatternShape::Variable { min_len } => {
                has_variable_length = true;
                match min_variable_length {
                    Some(current) if min_len < current => min_variable_length = Some(min_len),
                    None => min_variable_length = Some(min_len),
                    _ => {}
                }
            }
            PatternShape::Wildcard => {
                // Wildcard/irrefutable pattern — matches anything including zero-length,
                // so unconditionally override any prior minimum to 0
                has_variable_length = true;
                min_variable_length = Some(0);
            }
            PatternShape::Unknown => {}
        }
    }

    if valid_lengths.is_empty() && !has_variable_length {
        return None;
    }

    let mut arg_constraints = Vec::new();

    if has_variable_length {
        // Variable-length patterns: only Min constraint from the shortest
        if let Some(min) = min_variable_length {
            let fixed_min = valid_lengths.iter().copied().min();
            let overall_min = match fixed_min {
                Some(fm) if fm < min => fm,
                _ => min,
            };
            if overall_min > 0 {
                arg_constraints.push(ArgumentCountConstraint::Min(overall_min));
            }
        }
    } else {
        // Only fixed-length patterns
        valid_lengths.sort_unstable();
        if valid_lengths.len() == 1 {
            arg_constraints.push(ArgumentCountConstraint::Exact(valid_lengths[0]));
        } else {
            // Check if contiguous range → Min + Max
            let min = valid_lengths[0];
            let max = valid_lengths[valid_lengths.len() - 1];
            let is_contiguous = max - min + 1 == valid_lengths.len();
            if is_contiguous && valid_lengths.len() > 2 {
                arg_constraints.push(ArgumentCountConstraint::Min(min));
                arg_constraints.push(ArgumentCountConstraint::Max(max));
            } else {
                arg_constraints.push(ArgumentCountConstraint::OneOf(valid_lengths));
            }
        }
    }

    // Extract required keywords from valid cases
    let required_keywords = extract_keywords_from_valid_cases(&match_stmt.cases);

    Some(ExtractedTagConstraints {
        arg_constraints,
        required_keywords,
        choice_at_constraints: Vec::new(),
    })
}

/// Shape determined from analyzing a match case pattern.
enum PatternShape {
    /// Fixed number of elements (from `PatternMatchSequence` without star)
    Fixed(usize),
    /// Variable number of elements (from `PatternMatchSequence` with star)
    Variable { min_len: usize },
    /// Wildcard/irrefutable pattern (`case _:` or `case x:`)
    Wildcard,
    /// Unrecognized pattern
    Unknown,
}

/// Analyze a case pattern to determine its shape.
fn analyze_case_pattern(pattern: &Pattern) -> PatternShape {
    match pattern {
        Pattern::MatchSequence(PatternMatchSequence { patterns, .. }) => {
            let has_star = patterns.iter().any(|p| matches!(p, Pattern::MatchStar(_)));
            if has_star {
                // Count non-star elements for minimum length
                let fixed_count = patterns
                    .iter()
                    .filter(|p| !matches!(p, Pattern::MatchStar(_)))
                    .count();
                PatternShape::Variable {
                    min_len: fixed_count,
                }
            } else {
                PatternShape::Fixed(patterns.len())
            }
        }
        // `case _:` or `case x:` — wildcard/capture, matches anything
        Pattern::MatchAs(PatternMatchAs { pattern: None, .. }) => PatternShape::Wildcard,
        // `case pattern as x:` — delegate to inner pattern
        Pattern::MatchAs(PatternMatchAs {
            pattern: Some(inner),
            ..
        }) => analyze_case_pattern(inner),
        _ => PatternShape::Unknown,
    }
}

/// Extract required keyword literals from valid (non-error) match cases.
///
/// When ALL valid cases of the same length agree on a literal at a specific position,
/// that position has a required keyword.
fn extract_keywords_from_valid_cases(cases: &[MatchCase]) -> Vec<RequiredKeyword> {
    // Collect fixed-length valid cases grouped by length
    let mut by_length: std::collections::HashMap<usize, Vec<Vec<Option<String>>>> =
        std::collections::HashMap::new();

    for case in cases {
        if !matches!(classify_case_body(&case.body), CaseBody::NeverRaises) {
            continue;
        }
        if let Pattern::MatchSequence(PatternMatchSequence { patterns, .. }) = &case.pattern {
            if patterns.iter().any(|p| matches!(p, Pattern::MatchStar(_))) {
                continue; // Skip variable-length patterns for keyword extraction
            }
            let literals: Vec<Option<String>> = patterns.iter().map(pattern_literal).collect();
            by_length.entry(patterns.len()).or_default().push(literals);
        }
    }

    let mut keywords = Vec::new();
    for cases_at_len in by_length.values() {
        if cases_at_len.is_empty() {
            continue;
        }
        let num_positions = cases_at_len[0].len();
        for pos in 0..num_positions {
            // Check if ALL cases agree on the same literal at this position
            let first_literal = &cases_at_len[0][pos];
            if let Some(lit) = first_literal
                && cases_at_len
                    .iter()
                    .all(|c| c.get(pos).and_then(|v| v.as_ref()) == Some(lit))
            {
                // Skip position 0 — that's the tag name, not a user argument
                if pos > 0 {
                    let kw = RequiredKeyword {
                        position: SplitPosition::Forward(pos),
                        value: lit.clone(),
                    };
                    if !keywords.contains(&kw) {
                        keywords.push(kw);
                    }
                }
            }
        }
    }

    keywords
}

/// Extract a string literal from a pattern element, if it is one.
fn pattern_literal(pattern: &Pattern) -> Option<String> {
    match pattern {
        Pattern::MatchValue(PatternMatchValue { value, .. }) => {
            if let Expr::StringLiteral(ExprStringLiteral { value: s, .. }) = value.as_ref() {
                Some(s.to_str().to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

enum CaseBody {
    UnconditionalRaise,
    MixedRaise,
    NeverRaises,
}

fn classify_case_body(body: &[Stmt]) -> CaseBody {
    if direct_raise_exception(body).is_some() {
        return CaseBody::UnconditionalRaise;
    }

    if any_path_raises_exception(body) {
        CaseBody::MixedRaise
    } else {
        CaseBody::NeverRaises
    }
}

/// Check if any code path in a body contains a `raise` with an exception.
///
/// Unlike `exceptions::direct_raise_exception` (which only checks direct raises),
/// this recurses into control flow branches. Used for match case classification
/// where a nested raise means only some paths can error. Any exception type
/// counts — `TemplateSyntaxError`, `ValueError`, etc.
fn any_path_raises_exception(body: &[Stmt]) -> bool {
    let mut found = false;
    // Recurse into control flow to check all possible execution paths.
    // NOTE: Recursing into Stmt::Try means a raise caught by an except
    // handler is still treated as "this case can error." This is a known
    // false positive — no corpus projects exhibit this pattern, but it's
    // possible in the wild.
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if matches!(
            stmt,
            Stmt::Raise(ruff_python_ast::StmtRaise { exc: Some(_), .. })
        ) {
            found = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    found
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    fn parse_match(source: &str) -> StmtMatch {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        for stmt in module.body {
            if let Stmt::FunctionDef(func) = stmt {
                for stmt in func.body {
                    if let Stmt::Match(match_stmt) = stmt {
                        return match_stmt;
                    }
                }
            }
        }
        panic!("no match statement found in source");
    }

    #[test]
    fn unconditional_raise_arms_are_skipped() {
        let match_stmt = parse_match(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag":
            raise TemplateSyntaxError("bad")
        case "tag", name:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );
        let constraints = extract_match_constraints(
            &match_stmt,
            &mut Env::for_compile_function("parser", "token"),
        )
        .expect("valid arm should produce constraints");

        assert_eq!(
            constraints.arg_constraints,
            vec![ArgumentCountConstraint::Exact(2)]
        );
    }

    #[test]
    fn mixed_raise_arm_bails_out_of_match_extraction() {
        let match_stmt = parse_match(
            r#"
def do_tag(parser, token):
    match token.split_contents():
        case "tag", name:
            if name == "bad":
                raise TemplateSyntaxError("bad")
        case "tag", name, value:
            pass
        case _:
            raise TemplateSyntaxError("bad")
"#,
        );

        assert!(
            extract_match_constraints(
                &match_stmt,
                &mut Env::for_compile_function("parser", "token")
            )
            .is_none()
        );
    }
}
