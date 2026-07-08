use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::control_flow::BranchPath;
use super::control_flow::Truthiness;
use super::control_flow::evaluate_test_with;
use super::control_flow::is_irrefutable_match_case;
use super::model::PythonBinding;
use super::model::PythonCompleteness;
use super::model::PythonDict;
use super::model::PythonDictEntry;
use super::model::PythonMutation;
use super::model::PythonSemanticModel;
use super::model::PythonValue;
use super::model::PythonValueKind;
use super::mutation_target::MutationAccess;
use super::mutation_target::MutationTarget;
use super::source_graph::PythonImportEdge;
use super::source_graph::PythonSourceGraph;
use super::state::PythonSemanticState;
use super::statement_walk;
use super::statement_walk::StatementSemantics;
use super::touched_names::TouchedNames;
use super::touched_names::collect_touched_names;
use super::touched_names::expr_read_names;
use super::touched_names::first_import_segment;
use super::touched_names::pattern_bound_names;
use super::touched_names::target_write_names;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::PythonSource;
use crate::python::evaluate_path;

pub(super) struct EvaluationContext<'a> {
    source: &'a PythonSource,
    graph: &'a PythonSourceGraph,
    completed_models: &'a FxHashMap<File, PythonSemanticModel>,
    cycle_models: &'a FxHashSet<File>,
}

impl<'a> EvaluationContext<'a> {
    pub(super) const fn new(
        source: &'a PythonSource,
        graph: &'a PythonSourceGraph,
        completed_models: &'a FxHashMap<File, PythonSemanticModel>,
        cycle_models: &'a FxHashSet<File>,
    ) -> Self {
        Self {
            source,
            graph,
            completed_models,
            cycle_models,
        }
    }

    fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.source.file(), ranged.span())
    }

    fn resolved_model(&self, file: File) -> ResolvedImportModel<'_> {
        if let Some(model) = self.completed_models.get(&file) {
            ResolvedImportModel::Available(model)
        } else if self.cycle_models.contains(&file) {
            ResolvedImportModel::Cycle
        } else {
            panic!("resolved import edge must have an evaluated model or explicit cycle")
        }
    }
}

enum ResolvedImportModel<'a> {
    Available(&'a PythonSemanticModel),
    Cycle,
}

pub(super) fn evaluate_body(
    ctx: &EvaluationContext<'_>,
    state: PythonSemanticState,
    body: &[ast::Stmt],
) -> PythonSemanticState {
    let mut semantics = SemanticSemantics { ctx };
    statement_walk::walk_body(&mut semantics, state, body)
}

struct SemanticSemantics<'ctx, 'source> {
    ctx: &'ctx EvaluationContext<'source>,
}

impl StatementSemantics for SemanticSemantics<'_, '_> {
    type State = PythonSemanticState;

    fn walk_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAssign) {
        walk_assign(self.ctx, state, assign);
    }

    fn walk_ann_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAnnAssign) {
        walk_ann_assign(self.ctx, state, assign);
    }

    fn walk_aug_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAugAssign) {
        walk_aug_assign(self.ctx, state, assign);
    }

    fn walk_import(&mut self, state: &mut Self::State, import: &ast::StmtImport) {
        walk_import(self.ctx, state, &import.names);
    }

    fn walk_import_from(&mut self, state: &mut Self::State, import: &ast::StmtImportFrom) {
        walk_import_from(self.ctx, state, import);
    }

    fn walk_expr(&mut self, state: &mut Self::State, expr: &ast::StmtExpr) {
        walk_expr(self.ctx, state, &expr.value);
    }

    fn bind_for_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        bind_unknown_targets(self.ctx, state, target);
    }

    fn bind_with_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        bind_unknown_targets(self.ctx, state, target);
    }

    fn bind_function(&mut self, state: &mut Self::State, function: &ast::StmtFunctionDef) {
        bind_unknown_name(state, function.name.as_str(), self.ctx.origin(function));
    }

    fn bind_class(&mut self, state: &mut Self::State, class: &ast::StmtClassDef) {
        bind_unknown_name(state, class.name.as_str(), self.ctx.origin(class));
    }

    fn bind_delete_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        bind_unknown_targets(self.ctx, state, target);
    }

    fn bind_type_alias(&mut self, state: &mut Self::State, alias: &ast::StmtTypeAlias) {
        bind_unknown_targets(self.ctx, state, &alias.name);
    }

    fn bind_pattern_names(&mut self, state: &mut Self::State, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            bind_pattern_name(self.ctx, state, name);
        }
    }

    fn evaluate_test(&self, state: &Self::State, expr: &ast::Expr) -> Truthiness {
        evaluate_test(state, expr)
    }

    fn degrade_loop_bodies(
        &mut self,
        mut state: Self::State,
        bodies: &[&[ast::Stmt]],
    ) -> Self::State {
        let base = state.clone();
        let mut writes = TouchedNames::default();
        for body in bodies {
            let body_state = statement_walk::walk_body(self, base.clone(), body);
            writes.merge(PythonSemanticState::changed_writes_from(&base, &body_state));
        }
        degrade_writes(self.ctx, &mut state, writes);
        state
    }

    fn join_ambiguous_paths(
        &mut self,
        state: Self::State,
        paths: &[BranchPath<'_>],
    ) -> Self::State {
        let mut writes = TouchedNames::default();
        for path in paths {
            for segment in path.segments() {
                writes.merge(collect_writes(segment));
            }
        }

        let base = state;
        let mut branches = Vec::with_capacity(paths.len());
        for path in paths {
            let mut branch = base.clone();
            for segment in path.segments() {
                branch = statement_walk::walk_body(self, branch, segment);
            }
            branches.push(branch);
        }
        PythonSemanticState::join_branches(base, &branches, &writes)
    }

    fn join_match_cases(&mut self, state: Self::State, cases: &[ast::MatchCase]) -> Self::State {
        let mut writes = TouchedNames::default();
        for case in cases {
            record_pattern_writes(&case.pattern, &mut writes);
            writes.merge(collect_writes(&case.body));
        }

        let base = state;
        let mut branches = Vec::with_capacity(cases.len() + 1);
        for case in cases {
            let mut branch = base.clone();
            self.bind_pattern_names(&mut branch, &case.pattern);
            branches.push(statement_walk::walk_body(self, branch, &case.body));
        }
        if !cases.iter().any(is_irrefutable_match_case) {
            branches.push(base.clone());
        }
        PythonSemanticState::join_branches(base, &branches, &writes)
    }
}

fn walk_assign(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    assign: &ast::StmtAssign,
) {
    if assign.targets.len() != 1 {
        for target in &assign.targets {
            bind_unknown_targets(ctx, state, target);
        }
        return;
    }

    assign_target(ctx, state, &assign.targets[0], &assign.value);
}

fn walk_ann_assign(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    assign: &ast::StmtAnnAssign,
) {
    let Some(value) = &assign.value else {
        bind_unknown_targets(ctx, state, &assign.target);
        return;
    };
    assign_target(ctx, state, &assign.target, value);
}

fn assign_target(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    value: &ast::Expr,
) {
    if let Some(name) = target.name_target() {
        assign_name(ctx, state, name, value);
    } else {
        bind_unknown_targets(ctx, state, target);
    }
}

fn assign_name(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    name: &str,
    value: &ast::Expr,
) {
    let binding_origin = ctx.origin(value);
    if let Some(source_name) = value.name_target()
        && let Some(binding) = state.bindings.get(source_name).cloned()
    {
        let mut imported_binding = binding;
        imported_binding.name = name.to_string();
        for bound in &mut imported_binding.values {
            bound.binding_origin = binding_origin;
        }
        state.bindings.bind(name, imported_binding);
        state
            .mutations
            .replace_root_from_assignment(source_name, name);
        return;
    }

    state.mutations.remove_root(name);
    let value = evaluate_value(ctx, state, value);
    state
        .bindings
        .bind(name, PythonBinding::full(name, value, binding_origin));
}

fn walk_aug_assign(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    assign: &ast::StmtAugAssign,
) {
    if assign.op != ast::Operator::Add {
        bind_unknown_targets(ctx, state, &assign.target);
        return;
    }

    if !apply_extend_mutation(
        ctx,
        state,
        &assign.target,
        &assign.value,
        ctx.origin(assign),
    ) {
        bind_unknown_targets(ctx, state, &assign.target);
    }
}

fn walk_expr(ctx: &EvaluationContext<'_>, state: &mut PythonSemanticState, expr: &ast::Expr) {
    let ast::Expr::Call(call) = expr else {
        record_unsupported_touches(ctx, state, expr_read_names(expr));
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        record_unsupported_touches(ctx, state, expr_read_names(expr));
        return;
    };

    match attribute.attr.as_str() {
        "append" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
            record_mutation(state, &attribute.value, attribute.attr.as_str());
            if !apply_append_mutation(
                ctx,
                state,
                &attribute.value,
                &call.arguments.args[0],
                ctx.origin(call),
            ) {
                record_unsupported_touches(ctx, state, expr_read_names(expr));
            }
        }
        "extend" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
            record_mutation(state, &attribute.value, attribute.attr.as_str());
            if !apply_extend_mutation(
                ctx,
                state,
                &attribute.value,
                &call.arguments.args[0],
                ctx.origin(call),
            ) {
                record_unsupported_touches(ctx, state, expr_read_names(expr));
            }
        }
        "insert" if call.arguments.args.len() == 2 && call.arguments.keywords.is_empty() => {
            record_mutation(state, &attribute.value, attribute.attr.as_str());
            if !apply_insert_mutation(
                ctx,
                state,
                &attribute.value,
                &call.arguments.args[0],
                &call.arguments.args[1],
                ctx.origin(call),
            ) {
                record_unsupported_touches(ctx, state, expr_read_names(expr));
            }
        }
        "remove" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
            record_mutation(state, &attribute.value, attribute.attr.as_str());
            if !apply_remove_mutation(state, &attribute.value, &call.arguments.args[0]) {
                record_unsupported_touches(ctx, state, expr_read_names(expr));
            }
        }
        _ => record_unsupported_touches(ctx, state, expr_read_names(expr)),
    }
}

fn walk_import(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    aliases: &[ast::Alias],
) {
    for alias in aliases {
        let bound_name = alias.asname.as_ref().map_or_else(
            || first_import_segment(alias.name.as_str()),
            |asname| asname.as_str(),
        );
        bind_unknown_name(state, bound_name, ctx.origin(alias));
    }
}

fn walk_import_from(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    import: &ast::StmtImportFrom,
) {
    let is_star_import = import.names.iter().any(|alias| alias.name.as_str() == "*");
    let Some(edge) = ctx.graph.import_edge(ctx.source.file(), import) else {
        apply_missing_import(ctx, state, import, is_star_import);
        return;
    };

    if is_star_import {
        match edge {
            PythonImportEdge::Resolved { file, .. } => match ctx.resolved_model(*file) {
                ResolvedImportModel::Available(imported_model) => {
                    state.bindings.merge_star_import(imported_model);
                    state.mutations.extend_from(imported_model.mutation_set());
                }
                ResolvedImportModel::Cycle => state.bindings.mark_all_partial(),
            },
            PythonImportEdge::ReadFailed { .. }
            | PythonImportEdge::Unresolved { .. }
            | PythonImportEdge::SkippedExternal { .. } => state.bindings.mark_all_partial(),
        }
        return;
    }

    match edge {
        PythonImportEdge::Resolved { file, .. } => {
            let ResolvedImportModel::Available(imported_model) = ctx.resolved_model(*file) else {
                bind_imported_names_unknown(ctx, state, &import.names);
                return;
            };
            for (imported_name, bound_name) in edge.named_imports() {
                state.mutations.extend_renamed_root_from(
                    imported_model.mutation_set(),
                    imported_name,
                    bound_name,
                );
            }
            for alias in &import.names {
                let imported_name = alias.name.as_str();
                let bound_name = alias
                    .asname
                    .as_ref()
                    .map_or_else(|| imported_name, |asname| asname.as_str());
                if let Some(binding) = imported_model.binding(imported_name).cloned() {
                    state.bindings.bind(
                        bound_name,
                        PythonBinding {
                            name: bound_name.to_string(),
                            values: binding.values,
                            completeness: binding.completeness,
                        },
                    );
                } else {
                    bind_unknown_name(state, bound_name, ctx.origin(alias));
                }
            }
        }
        PythonImportEdge::ReadFailed { .. }
        | PythonImportEdge::Unresolved { .. }
        | PythonImportEdge::SkippedExternal { .. } => {
            bind_imported_names_unknown(ctx, state, &import.names);
        }
    }
}

fn bind_imported_names_unknown(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    aliases: &[ast::Alias],
) {
    for alias in aliases {
        let bound_name = alias
            .asname
            .as_ref()
            .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
        bind_unknown_name(state, bound_name, ctx.origin(alias));
    }
}

fn apply_missing_import(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    import: &ast::StmtImportFrom,
    is_star_import: bool,
) {
    if is_star_import {
        state.bindings.mark_all_partial();
        return;
    }
    bind_imported_names_unknown(ctx, state, &import.names);
}

pub(super) fn evaluate_test(state: &PythonSemanticState, expr: &ast::Expr) -> Truthiness {
    evaluate_test_with(expr, |name| state.bindings.bool_value(name))
}

fn evaluate_value(
    ctx: &EvaluationContext<'_>,
    state: &PythonSemanticState,
    expr: &ast::Expr,
) -> PythonValue {
    let origin = ctx.origin(expr);
    let path = evaluate_path(expr, ctx.source.path(), &state.bindings.path_bindings());

    if let Some(name) = expr.name_target()
        && let Some(binding) = state.bindings.get(name)
        && let [bound] = binding.values()
    {
        let mut value = bound.value().clone();
        if !binding.is_complete() || !bound.is_complete() {
            value.mark_partial();
        }
        return value;
    }

    if let Some(value) = expr.string_literal() {
        return PythonValue::full(PythonValueKind::Str(value.to_string()), origin);
    }
    if let Some(value) = expr.bool_literal() {
        return PythonValue::full(PythonValueKind::Bool(value), origin);
    }
    if let Some(path) = path {
        return PythonValue::full(PythonValueKind::Path(path), origin);
    }

    match expr {
        ast::Expr::List(list) => evaluate_list(ctx, state, &list.elts, origin),
        ast::Expr::Tuple(tuple) => evaluate_list(ctx, state, &tuple.elts, origin),
        ast::Expr::Dict(dict) => evaluate_dict(ctx, state, dict, origin),
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            evaluate_addition(ctx, state, &bin_op.left, &bin_op.right, origin)
        }
        _ => PythonValue::unknown(origin),
    }
}

fn evaluate_list(
    ctx: &EvaluationContext<'_>,
    state: &PythonSemanticState,
    elements: &[ast::Expr],
    origin: Origin,
) -> PythonValue {
    let mut values = Vec::new();
    let mut completeness = PythonCompleteness::Full;
    for element in elements {
        if let ast::Expr::Starred(starred) = element {
            let starred = evaluate_value(ctx, state, &starred.value);
            match starred.kind() {
                PythonValueKind::List(items) => values.extend(items.clone()),
                _ => completeness = PythonCompleteness::Partial,
            }
            if !starred.is_complete() {
                completeness = PythonCompleteness::Partial;
            }
            continue;
        }

        let value = evaluate_value(ctx, state, element);
        if !value.is_complete() {
            completeness = PythonCompleteness::Partial;
        }
        values.push(value);
    }
    PythonValue::new(PythonValueKind::List(values), origin, completeness)
}

fn evaluate_addition(
    ctx: &EvaluationContext<'_>,
    state: &PythonSemanticState,
    left: &ast::Expr,
    right: &ast::Expr,
    origin: Origin,
) -> PythonValue {
    let left = evaluate_value(ctx, state, left);
    let right = evaluate_value(ctx, state, right);
    let operands_complete = left.is_complete() && right.is_complete();
    match (left.kind, right.kind) {
        (PythonValueKind::List(mut left), PythonValueKind::List(right)) => {
            left.extend(right);
            let completeness = if operands_complete && left.iter().all(PythonValue::is_complete) {
                PythonCompleteness::Full
            } else {
                PythonCompleteness::Partial
            };
            PythonValue::new(PythonValueKind::List(left), origin, completeness)
        }
        _ => PythonValue::unknown(origin),
    }
}

fn evaluate_dict(
    ctx: &EvaluationContext<'_>,
    state: &PythonSemanticState,
    dict: &ast::ExprDict,
    origin: Origin,
) -> PythonValue {
    let mut entries: Vec<PythonDictEntry> = Vec::new();
    let mut seen_string_keys = FxHashSet::default();
    let mut completeness = PythonCompleteness::Full;
    let mut earlier_entries_uncertain = false;

    for item in dict.items.iter().rev() {
        let Some(key_expr) = &item.key else {
            completeness = PythonCompleteness::Partial;
            earlier_entries_uncertain = true;
            continue;
        };

        let key = evaluate_value(ctx, state, key_expr);
        if let PythonValueKind::Str(key_text) = key.kind()
            && !seen_string_keys.insert(key_text.clone())
        {
            continue;
        }

        let mut value = evaluate_value(ctx, state, &item.value);
        if earlier_entries_uncertain {
            value.mark_partial();
        }
        if !key.is_complete() || !value.is_complete() {
            completeness = PythonCompleteness::Partial;
        }

        entries.push(PythonDictEntry { key, value });
    }

    entries.reverse();
    PythonValue::new(
        PythonValueKind::Dict(PythonDict { entries }),
        origin,
        completeness,
    )
}

fn bind_unknown_targets(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    target: &ast::Expr,
) {
    for name in target_write_names(target) {
        bind_unknown_name(state, name, ctx.origin(target));
    }
}

fn bind_unknown_name(state: &mut PythonSemanticState, name: &str, origin: Origin) {
    state
        .bindings
        .bind(name, PythonBinding::unknown(name, origin));
}

fn record_mutation(state: &mut PythonSemanticState, target: &ast::Expr, method: &str) {
    if let Some(target) = MutationTarget::from_expr(target) {
        state.mutations.insert(PythonMutation {
            root: target.root.to_string(),
            access: target
                .access
                .iter()
                .map(MutationAccess::to_public)
                .collect(),
            method: method.to_string(),
        });
    }
}

fn record_unsupported_touches(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    names: impl IntoIterator<Item = String>,
) {
    for name in names {
        bind_unknown_name(
            state,
            &name,
            Origin::new(ctx.source.file(), Span::new(0, 0)),
        );
    }
}

fn apply_append_mutation(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    value: &ast::Expr,
    _origin: Origin,
) -> bool {
    let value = evaluate_value(ctx, state, value);
    let value_complete = value.is_complete();
    mutate_target(state, target, |target_value| match &mut target_value.kind {
        PythonValueKind::List(values) => {
            values.push(value);
            if !value_complete {
                target_value.mark_partial();
            }
            true
        }
        _ => false,
    })
}

fn apply_extend_mutation(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    value: &ast::Expr,
    _origin: Origin,
) -> bool {
    let value = evaluate_value(ctx, state, value);
    let value_complete = value.is_complete();
    mutate_target(state, target, |target_value| {
        match (&mut target_value.kind, value.kind) {
            (PythonValueKind::List(values), PythonValueKind::List(mut extension)) => {
                values.append(&mut extension);
                if !value_complete {
                    target_value.mark_partial();
                }
                true
            }
            (PythonValueKind::List(_), _) => {
                target_value.mark_partial();
                true
            }
            _ => false,
        }
    })
}

fn apply_insert_mutation(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    index: &ast::Expr,
    value: &ast::Expr,
    _origin: Origin,
) -> bool {
    let Some(index) = index.non_negative_integer() else {
        return mark_target_partial(state, target);
    };
    let value = evaluate_value(ctx, state, value);
    let value_complete = value.is_complete();
    mutate_target(state, target, |target_value| match &mut target_value.kind {
        PythonValueKind::List(values) => {
            let index = index.min(values.len());
            values.insert(index, value);
            if !value_complete {
                target_value.mark_partial();
            }
            true
        }
        _ => false,
    })
}

fn apply_remove_mutation(
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    value: &ast::Expr,
) -> bool {
    let Some(value) = value.string_literal() else {
        return mark_target_partial(state, target);
    };
    mutate_target(state, target, |target_value| match &mut target_value.kind {
        PythonValueKind::List(values) => {
            if let Some(position) = values.iter().position(
                |item| matches!(item.kind(), PythonValueKind::Str(candidate) if candidate == value),
            ) {
                values.remove(position);
            }
            true
        }
        _ => false,
    })
}

fn mark_target_partial(state: &mut PythonSemanticState, target: &ast::Expr) -> bool {
    mutate_target(state, target, |value| {
        value.mark_partial();
        true
    })
}

fn mutate_target(
    state: &mut PythonSemanticState,
    target: &ast::Expr,
    mutate: impl FnOnce(&mut PythonValue) -> bool,
) -> bool {
    let Some(target) = MutationTarget::from_expr(target) else {
        return false;
    };
    let Some(binding) = state.bindings.by_name.get_mut(target.root) else {
        return false;
    };
    let [bound] = binding.values.as_mut_slice() else {
        binding.mark_partial();
        return true;
    };
    let Some(value) = target.resolve_mut(&mut bound.value) else {
        binding.mark_partial();
        return true;
    };
    let mutated = mutate(value);
    if mutated && !value.is_complete() {
        binding.mark_partial();
    }
    mutated
}

pub(super) fn collect_writes(body: &[ast::Stmt]) -> TouchedNames {
    collect_touched_names(body)
}

fn record_pattern_writes(pattern: &ast::Pattern, writes: &mut TouchedNames) {
    for name in pattern_bound_names(pattern) {
        writes.record(name);
    }
}

pub(super) fn degrade_writes(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    writes: TouchedNames,
) {
    if writes.all {
        state.bindings.mark_all_partial();
    }
    state.degrade_names(
        writes.names,
        Origin::new(ctx.source.file(), Span::new(0, 0)),
    );
}

pub(super) fn bind_pattern_name(
    ctx: &EvaluationContext<'_>,
    state: &mut PythonSemanticState,
    name: &str,
) {
    state.bindings.bind(
        name,
        PythonBinding::unknown(name, Origin::new(ctx.source.file(), Span::new(0, 0))),
    );
}
