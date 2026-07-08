use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use ruff_python_ast as ast;

/// A Python `from X import ...` statement that another extractor may follow.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ModuleImport {
    pub(crate) level: u32,
    pub(crate) module: Option<String>,
}

/// Source text plus the file identity that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleSource {
    source: String,
    file: File,
    path: Utf8PathBuf,
}

impl ModuleSource {
    pub(crate) fn new(file: File, path: Utf8PathBuf, source: String) -> Self {
        Self { source, file, path }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }

    pub(crate) fn path(&self) -> &Utf8Path {
        &self.path
    }
}

/// Import-following seam used by module-level Python fact extractors.
pub(crate) trait ModuleImportResolver {
    fn resolve_star_import(
        &mut self,
        import: &ModuleImport,
        importer: &Utf8Path,
    ) -> Option<ModuleSource>;

    fn resolve_named_import(
        &mut self,
        import: &ModuleImport,
        importer: &Utf8Path,
    ) -> Option<ModuleSource>;
}

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

pub(crate) trait ModuleFactsBuilder {
    type State: Clone;
    type Writes: Default;

    fn walk_body(&mut self, body: &[ast::Stmt]);
    fn evaluate_test(&self, expr: &ast::Expr) -> Truthiness;
    fn collect_writes(&self, body: &[ast::Stmt]) -> Self::Writes;
    fn merge_writes(&self, writes: &mut Self::Writes, other: Self::Writes);
    fn degrade_writes(&mut self, writes: Self::Writes);
    fn take_state(&mut self) -> Self::State;
    fn set_state(&mut self, state: Self::State);
    fn join_branches(
        &self,
        base: Self::State,
        branches: &[Self::State],
        writes: &Self::Writes,
    ) -> Self::State;
    fn bind_pattern_name(&mut self, name: &str);
    fn record_name_write(&self, name: &str, writes: &mut Self::Writes);
}

#[derive(Debug, Clone, Copy)]
enum ControlFlowPath<'a> {
    One(&'a [ast::Stmt]),
    Two(&'a [ast::Stmt], &'a [ast::Stmt]),
}

impl<'a> ControlFlowPath<'a> {
    fn segments(self) -> impl Iterator<Item = &'a [ast::Stmt]> {
        match self {
            Self::One(body) => [Some(body), None],
            Self::Two(first, second) => [Some(first), Some(second)],
        }
        .into_iter()
        .flatten()
    }
}

pub(crate) fn walk_if<B: ModuleFactsBuilder>(builder: &mut B, stmt_if: &ast::StmtIf) {
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

fn walk_false_if_clauses<B: ModuleFactsBuilder>(builder: &mut B, clauses: &[ast::ElifElseClause]) {
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

pub(crate) fn walk_try<B: ModuleFactsBuilder>(builder: &mut B, stmt_try: &ast::StmtTry) {
    if stmt_try.handlers.is_empty() {
        builder.walk_body(&stmt_try.body);
        builder.walk_body(&stmt_try.orelse);
        builder.walk_body(&stmt_try.finalbody);
        return;
    }

    let mut paths = Vec::with_capacity(1 + stmt_try.handlers.len() * stmt_try.body.len().max(1));
    paths.push(ControlFlowPath::Two(
        stmt_try.body.as_slice(),
        stmt_try.orelse.as_slice(),
    ));
    for handler in &stmt_try.handlers {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        for prefix_len in 0..stmt_try.body.len().max(1) {
            paths.push(ControlFlowPath::Two(
                &stmt_try.body[..prefix_len],
                handler.body.as_slice(),
            ));
        }
    }
    walk_ambiguous_paths(builder, &paths);
    builder.walk_body(&stmt_try.finalbody);
}

pub(crate) fn walk_match<B: ModuleFactsBuilder>(builder: &mut B, stmt_match: &ast::StmtMatch) {
    if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
        bind_pattern_names(builder, &stmt_match.cases[0].pattern);
        builder.walk_body(&stmt_match.cases[0].body);
        return;
    }

    let mut writes = B::Writes::default();
    for case in &stmt_match.cases {
        record_pattern_writes(builder, &case.pattern, &mut writes);
        let body_writes = builder.collect_writes(&case.body);
        builder.merge_writes(&mut writes, body_writes);
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
    let joined = builder.join_branches(base, &branches, &writes);
    builder.set_state(joined);
}

pub(crate) fn degrade_touched_bodies<B: ModuleFactsBuilder>(
    builder: &mut B,
    bodies: &[&[ast::Stmt]],
) {
    let mut writes = B::Writes::default();
    for body in bodies {
        let body_writes = builder.collect_writes(body);
        builder.merge_writes(&mut writes, body_writes);
    }
    builder.degrade_writes(writes);
}

fn walk_ambiguous_arms<B: ModuleFactsBuilder>(builder: &mut B, arms: &[&[ast::Stmt]]) {
    let paths: Vec<ControlFlowPath<'_>> =
        arms.iter().map(|arm| ControlFlowPath::One(arm)).collect();
    walk_ambiguous_paths(builder, &paths);
}

fn walk_ambiguous_paths<B: ModuleFactsBuilder>(builder: &mut B, paths: &[ControlFlowPath<'_>]) {
    let mut writes = B::Writes::default();
    for path in paths {
        for segment in path.segments() {
            let segment_writes = builder.collect_writes(segment);
            builder.merge_writes(&mut writes, segment_writes);
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
    let joined = builder.join_branches(base, &branches, &writes);
    builder.set_state(joined);
}

fn bind_pattern_names<B: ModuleFactsBuilder>(builder: &mut B, pattern: &ast::Pattern) {
    for name in pattern_bound_names(pattern) {
        builder.bind_pattern_name(name);
    }
}

fn record_pattern_writes<B: ModuleFactsBuilder>(
    builder: &B,
    pattern: &ast::Pattern,
    writes: &mut B::Writes,
) {
    for name in pattern_bound_names(pattern) {
        builder.record_name_write(name, writes);
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
