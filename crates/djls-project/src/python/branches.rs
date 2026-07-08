use ruff_python_ast as ast;

use super::semantic_model::PythonSemanticModelBuilder;
use super::semantic_model::TouchedNames;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    pub(crate) const fn from_bool(value: bool) -> Self {
        if value {
            Self::AlwaysTrue
        } else {
            Self::AlwaysFalse
        }
    }

    pub(crate) const fn negate(self) -> Self {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Ambiguous => Self::Ambiguous,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BranchPath<'a> {
    One(&'a [ast::Stmt]),
    Two(&'a [ast::Stmt], &'a [ast::Stmt]),
}

impl<'a> BranchPath<'a> {
    fn segments(self) -> impl Iterator<Item = &'a [ast::Stmt]> {
        match self {
            Self::One(body) => [Some(body), None],
            Self::Two(first, second) => [Some(first), Some(second)],
        }
        .into_iter()
        .flatten()
    }
}

pub(crate) fn walk_if(builder: &mut PythonSemanticModelBuilder<'_>, stmt_if: &ast::StmtIf) {
    match builder.evaluate_test(&stmt_if.test) {
        Truthiness::AlwaysTrue => builder.walk_body(&stmt_if.body),
        Truthiness::AlwaysFalse => walk_false_if_clauses(builder, &stmt_if.elif_else_clauses),
        Truthiness::Ambiguous => {
            let mut arms = Vec::with_capacity(stmt_if.elif_else_clauses.len() + 2);
            arms.push(stmt_if.body.as_slice());
            arms.extend(
                stmt_if
                    .elif_else_clauses
                    .iter()
                    .map(|clause| clause.body.as_slice()),
            );
            if !stmt_if
                .elif_else_clauses
                .iter()
                .any(|clause| clause.test.is_none())
            {
                arms.push(&[]);
            }
            walk_ambiguous_arms(builder, &arms);
        }
    }
}

fn walk_false_if_clauses(
    builder: &mut PythonSemanticModelBuilder<'_>,
    clauses: &[ast::ElifElseClause],
) {
    for (index, clause) in clauses.iter().enumerate() {
        let Some(test) = &clause.test else {
            builder.walk_body(&clause.body);
            return;
        };

        match builder.evaluate_test(test) {
            Truthiness::AlwaysTrue => {
                builder.walk_body(&clause.body);
                return;
            }
            Truthiness::AlwaysFalse => {}
            Truthiness::Ambiguous => {
                let ambiguous_clauses = &clauses[index..];
                let mut arms: Vec<&[ast::Stmt]> = ambiguous_clauses
                    .iter()
                    .map(|clause| clause.body.as_slice())
                    .collect();
                if !ambiguous_clauses.iter().any(|clause| clause.test.is_none()) {
                    arms.push(&[]);
                }
                walk_ambiguous_arms(builder, &arms);
                return;
            }
        }
    }
}

pub(crate) fn walk_try(builder: &mut PythonSemanticModelBuilder<'_>, stmt_try: &ast::StmtTry) {
    if stmt_try.handlers.is_empty() {
        builder.walk_body(&stmt_try.body);
        builder.walk_body(&stmt_try.orelse);
        builder.walk_body(&stmt_try.finalbody);
        return;
    }

    let mut paths = Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
    paths.push(BranchPath::Two(
        stmt_try.body.as_slice(),
        stmt_try.orelse.as_slice(),
    ));
    for handler in &stmt_try.handlers {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        for prefix_len in 0..stmt_try.body.len().max(1) {
            paths.push(BranchPath::Two(
                &stmt_try.body[..prefix_len],
                handler.body.as_slice(),
            ));
        }
    }
    walk_ambiguous_paths(builder, &paths);
    builder.walk_body(&stmt_try.finalbody);
}

pub(crate) fn walk_match(
    builder: &mut PythonSemanticModelBuilder<'_>,
    stmt_match: &ast::StmtMatch,
) {
    if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
        bind_pattern_names(builder, &stmt_match.cases[0].pattern);
        builder.walk_body(&stmt_match.cases[0].body);
        return;
    }

    let mut writes = TouchedNames::default();
    for case in &stmt_match.cases {
        record_pattern_writes(&case.pattern, &mut writes);
        let body_writes = PythonSemanticModelBuilder::collect_writes(&case.body);
        writes.merge(body_writes);
    }

    let base = builder.take_state();
    let mut branches = Vec::with_capacity(stmt_match.cases.len() + 1);
    for case in &stmt_match.cases {
        builder.set_state(base.clone());
        bind_pattern_names(builder, &case.pattern);
        builder.walk_body(&case.body);
        branches.push(builder.take_state());
    }
    if !stmt_match.cases.iter().any(is_irrefutable_match_case) {
        branches.push(base.clone());
    }
    let joined = PythonSemanticModelBuilder::join_branches(base, &branches, &writes);
    builder.set_state(joined);
}

pub(crate) fn degrade_loop_bodies(
    builder: &mut PythonSemanticModelBuilder<'_>,
    bodies: &[&[ast::Stmt]],
) {
    let mut writes = TouchedNames::default();
    for body in bodies {
        let body_writes = PythonSemanticModelBuilder::collect_writes(body);
        writes.merge(body_writes);
    }
    builder.degrade_writes(writes);
}

fn walk_ambiguous_arms(builder: &mut PythonSemanticModelBuilder<'_>, arms: &[&[ast::Stmt]]) {
    let paths: Vec<BranchPath<'_>> = arms.iter().map(|arm| BranchPath::One(arm)).collect();
    walk_ambiguous_paths(builder, &paths);
}

fn walk_ambiguous_paths(builder: &mut PythonSemanticModelBuilder<'_>, paths: &[BranchPath<'_>]) {
    let mut writes = TouchedNames::default();
    for path in paths {
        for segment in path.segments() {
            let segment_writes = PythonSemanticModelBuilder::collect_writes(segment);
            writes.merge(segment_writes);
        }
    }

    let base = builder.take_state();
    let mut branches = Vec::with_capacity(paths.len());
    for path in paths {
        builder.set_state(base.clone());
        for segment in path.segments() {
            builder.walk_body(segment);
        }
        branches.push(builder.take_state());
    }
    let joined = PythonSemanticModelBuilder::join_branches(base, &branches, &writes);
    builder.set_state(joined);
}

fn bind_pattern_names(builder: &mut PythonSemanticModelBuilder<'_>, pattern: &ast::Pattern) {
    for name in pattern_bound_names(pattern) {
        builder.bind_pattern_name(name);
    }
}

fn record_pattern_writes(pattern: &ast::Pattern, writes: &mut TouchedNames) {
    for name in pattern_bound_names(pattern) {
        writes.record(name);
    }
}

pub(crate) fn pattern_bound_names(pattern: &ast::Pattern) -> Vec<&str> {
    let mut names = Vec::new();
    collect_pattern_bound_names(pattern, &mut names);
    names
}

fn collect_pattern_bound_names<'a>(pattern: &'a ast::Pattern, names: &mut Vec<&'a str>) {
    match pattern {
        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => {}
        ast::Pattern::MatchSequence(sequence) => {
            for pattern in &sequence.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
        ast::Pattern::MatchMapping(mapping) => {
            for pattern in &mapping.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(rest) = &mapping.rest {
                names.push(rest.as_str());
            }
        }
        ast::Pattern::MatchClass(class) => {
            for pattern in &class.arguments.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            for keyword in &class.arguments.keywords {
                collect_pattern_bound_names(&keyword.pattern, names);
            }
        }
        ast::Pattern::MatchStar(star) => {
            if let Some(name) = &star.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchAs(match_as) => {
            if let Some(pattern) = &match_as.pattern {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(name) = &match_as.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchOr(match_or) => {
            for pattern in &match_or.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
    }
}

fn is_irrefutable_match_case(case: &ast::MatchCase) -> bool {
    case.guard.is_none() && is_irrefutable_pattern(&case.pattern)
}

fn is_irrefutable_pattern(pattern: &ast::Pattern) -> bool {
    match pattern {
        ast::Pattern::MatchValue(_)
        | ast::Pattern::MatchSingleton(_)
        | ast::Pattern::MatchSequence(_)
        | ast::Pattern::MatchMapping(_)
        | ast::Pattern::MatchClass(_)
        | ast::Pattern::MatchStar(_) => false,
        ast::Pattern::MatchAs(match_as) => match_as
            .pattern
            .as_deref()
            .is_none_or(is_irrefutable_pattern),
        ast::Pattern::MatchOr(match_or) => match_or.patterns.iter().any(is_irrefutable_pattern),
    }
}
