use ruff_python_ast::Expr;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::MatchCase;
use ruff_python_ast::Pattern;
use ruff_python_ast::PatternMatchAs;
use ruff_python_ast::PatternMatchSequence;
use ruff_python_ast::PatternMatchValue;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtMatch;

use super::expressions::eval_expr;
use crate::dataflow::constraints::ConstraintSet;
use crate::dataflow::domain::AbstractValue;
use crate::dataflow::domain::Env;
use crate::types::ArgumentCountConstraint;
use crate::types::RequiredKeyword;
use crate::types::SplitPosition;

/// Extract argument constraints from a match statement whose subject is a `SplitResult`.
///
/// Analyzes `match token.split_contents(): case ...:` patterns from Django 6.0+.
/// Collects valid case shapes (cases whose body does NOT raise `TemplateSyntaxError`)
/// and derives argument count constraints and required keywords from them.
pub(super) fn extract_match_constraints(
    match_stmt: &StmtMatch,
    env: &Env,
) -> Option<ConstraintSet> {
    let subject = eval_expr(&match_stmt.subject, env);
    if !matches!(subject, AbstractValue::SplitResult(_)) {
        return None;
    }

    let mut valid_lengths: Vec<usize> = Vec::new();
    let mut has_variable_length = false;
    let mut min_variable_length: Option<usize> = None;

    for case in &match_stmt.cases {
        if any_path_raises_template_syntax_error(&case.body) {
            continue;
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

    Some(ConstraintSet {
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
        if any_path_raises_template_syntax_error(&case.body) {
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
            if let Some(lit) = first_literal {
                if cases_at_len
                    .iter()
                    .all(|c| c.get(pos).and_then(|v| v.as_ref()) == Some(lit))
                {
                    // Skip position 0 — that's the tag name, not a user argument
                    if pos > 0 {
                        keywords.push(RequiredKeyword {
                            position: SplitPosition::Forward(pos),
                            value: lit.clone(),
                        });
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

/// Check if any code path in a body contains `raise TemplateSyntaxError(...)`.
///
/// Unlike `constraints::body_raises_template_syntax_error` (which only checks
/// direct raises), this recurses into if/elif/else branches. Used for match
/// case classification where any raise in any branch means the case can error.
fn any_path_raises_template_syntax_error(body: &[Stmt]) -> bool {
    use ruff_python_ast::StmtRaise;

    for stmt in body {
        match stmt {
            Stmt::Raise(StmtRaise { exc: Some(exc), .. }) => {
                if crate::dataflow::constraints::is_template_syntax_error_call(exc) {
                    return true;
                }
            }
            Stmt::If(if_stmt) => {
                if any_path_raises_template_syntax_error(&if_stmt.body) {
                    return true;
                }
                for clause in &if_stmt.elif_else_clauses {
                    if any_path_raises_template_syntax_error(&clause.body) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}
