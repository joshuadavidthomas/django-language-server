use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;
use rustc_hash::FxHashSet;

use super::PythonSemanticModelAnalysis;
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
use super::state::PythonSemanticState;
use super::touched_names::TouchedNames;
use super::touched_names::collect_touched_names;
use super::touched_names::expr_read_names;
use super::touched_names::first_import_segment;
use super::touched_names::target_write_names;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::ImportSourceResolution;
use crate::python::PythonImport;
use crate::python::PythonImportResolver;
use crate::python::PythonSource;
use crate::python::evaluate_path;

mod control_flow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    pub(super) const fn from_bool(value: bool) -> Self {
        if value {
            Self::AlwaysTrue
        } else {
            Self::AlwaysFalse
        }
    }

    pub(super) const fn negate(self) -> Self {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Ambiguous => Self::Ambiguous,
        }
    }
}

pub(super) struct PythonSemanticEvaluator<'a> {
    source: &'a PythonSource,
    analysis: &'a mut PythonSemanticModelAnalysis,
    resolver: &'a mut dyn PythonImportResolver,
}

impl<'a> PythonSemanticEvaluator<'a> {
    pub(super) fn new(
        source: &'a PythonSource,
        analysis: &'a mut PythonSemanticModelAnalysis,
        resolver: &'a mut dyn PythonImportResolver,
    ) -> Self {
        Self {
            source,
            analysis,
            resolver,
        }
    }

    fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.source.file(), ranged.span())
    }

    pub(super) fn walk_body(
        &mut self,
        mut state: PythonSemanticState,
        body: &[ast::Stmt],
    ) -> PythonSemanticState {
        for stmt in body {
            state = self.walk_stmt(state, stmt);
        }
        state
    }

    fn walk_stmt(
        &mut self,
        mut state: PythonSemanticState,
        stmt: &ast::Stmt,
    ) -> PythonSemanticState {
        match stmt {
            ast::Stmt::Assign(assign) => self.walk_assign(&mut state, assign),
            ast::Stmt::AnnAssign(assign) => self.walk_ann_assign(&mut state, assign),
            ast::Stmt::AugAssign(assign) => self.walk_aug_assign(&mut state, assign),
            ast::Stmt::Expr(expr) => self.walk_expr(&mut state, &expr.value),
            ast::Stmt::Import(import) => self.walk_import(&mut state, &import.names),
            ast::Stmt::ImportFrom(import) => self.walk_import_from(&mut state, import),
            ast::Stmt::If(stmt_if) => {
                return self.walk_if(state, stmt_if);
            }
            ast::Stmt::For(stmt_for) => {
                self.bind_unknown_targets(&mut state, &stmt_for.target);
                return self.degrade_loop_bodies(state, &[&stmt_for.body, &stmt_for.orelse]);
            }
            ast::Stmt::While(stmt_while) => {
                return match Self::evaluate_test(&state, &stmt_while.test) {
                    Truthiness::AlwaysFalse => self.walk_body(state, &stmt_while.orelse),
                    Truthiness::AlwaysTrue | Truthiness::Ambiguous => {
                        self.degrade_loop_bodies(state, &[&stmt_while.body, &stmt_while.orelse])
                    }
                };
            }
            ast::Stmt::With(stmt_with) => {
                for item in &stmt_with.items {
                    if let Some(optional_vars) = &item.optional_vars {
                        self.bind_unknown_targets(&mut state, optional_vars);
                    }
                }
                return self.walk_body(state, &stmt_with.body);
            }
            ast::Stmt::Try(stmt_try) => {
                return self.walk_try(state, stmt_try);
            }
            ast::Stmt::FunctionDef(function) => {
                Self::bind_unknown_name(&mut state, function.name.as_str(), self.origin(function));
            }
            ast::Stmt::ClassDef(class) => {
                Self::bind_unknown_name(&mut state, class.name.as_str(), self.origin(class));
            }
            ast::Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.bind_unknown_targets(&mut state, target);
                }
            }
            ast::Stmt::TypeAlias(type_alias) => {
                self.bind_unknown_targets(&mut state, &type_alias.name);
            }
            ast::Stmt::Match(stmt_match) => {
                return self.walk_match(state, stmt_match);
            }
            ast::Stmt::Return(_)
            | ast::Stmt::Raise(_)
            | ast::Stmt::Assert(_)
            | ast::Stmt::Global(_)
            | ast::Stmt::Nonlocal(_)
            | ast::Stmt::Pass(_)
            | ast::Stmt::Break(_)
            | ast::Stmt::Continue(_)
            | ast::Stmt::IpyEscapeCommand(_) => {}
        }
        state
    }

    fn walk_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAssign) {
        if assign.targets.len() != 1 {
            for target in &assign.targets {
                self.bind_unknown_targets(state, target);
            }
            return;
        }

        self.assign_target(state, &assign.targets[0], &assign.value);
    }

    fn walk_ann_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAnnAssign) {
        let Some(value) = &assign.value else {
            self.bind_unknown_targets(state, &assign.target);
            return;
        };
        self.assign_target(state, &assign.target, value);
    }

    fn assign_target(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
    ) {
        if let Some(name) = target.name_target() {
            self.assign_name(state, name, value);
        } else {
            self.bind_unknown_targets(state, target);
        }
    }

    fn assign_name(&mut self, state: &mut PythonSemanticState, name: &str, value: &ast::Expr) {
        let binding_origin = self.origin(value);
        state.mutations.retain(|mutation| mutation.root != name);

        if let Some(source_name) = value.name_target()
            && let Some(binding) = state.bindings.get(source_name).cloned()
        {
            let mut imported_binding = binding;
            imported_binding.name = name.to_string();
            for bound in &mut imported_binding.values {
                bound.binding_origin = binding_origin;
            }
            state.bindings.bind(name, imported_binding);
            return;
        }

        let value = self.evaluate_value(state, value);
        state
            .bindings
            .bind(name, PythonBinding::full(name, value, binding_origin));
    }

    fn walk_aug_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAugAssign) {
        if assign.op != ast::Operator::Add {
            self.bind_unknown_targets(state, &assign.target);
            return;
        }

        if !self.apply_extend_mutation(state, &assign.target, &assign.value, self.origin(assign)) {
            self.bind_unknown_targets(state, &assign.target);
        }
    }

    fn walk_expr(&mut self, state: &mut PythonSemanticState, expr: &ast::Expr) {
        let ast::Expr::Call(call) = expr else {
            self.record_unsupported_touches(state, expr_read_names(expr));
            return;
        };
        let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
            self.record_unsupported_touches(state, expr_read_names(expr));
            return;
        };

        Self::record_mutation(state, &attribute.value, attribute.attr.as_str());

        match attribute.attr.as_str() {
            "append" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !self.apply_append_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "extend" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !self.apply_extend_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "insert" if call.arguments.args.len() == 2 && call.arguments.keywords.is_empty() => {
                if !self.apply_insert_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    &call.arguments.args[1],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "remove" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !Self::apply_remove_mutation(state, &attribute.value, &call.arguments.args[0]) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            _ => self.record_unsupported_touches(state, expr_read_names(expr)),
        }
    }

    fn walk_import(&mut self, state: &mut PythonSemanticState, aliases: &[ast::Alias]) {
        for alias in aliases {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                |asname| asname.as_str(),
            );
            Self::bind_unknown_name(state, bound_name, self.origin(alias));
        }
    }

    fn analyze_imported_model(
        &mut self,
        state: &mut PythonSemanticState,
        imported_source: &PythonSource,
    ) -> PythonSemanticModel {
        let imported_model = self.analysis.analyze_source(imported_source, self.resolver);
        state.mutations.extend(imported_model.mutations.clone());
        state.effects.add_imported_model(imported_model.clone());
        imported_model
    }

    fn walk_import_from(&mut self, state: &mut PythonSemanticState, import: &ast::StmtImportFrom) {
        let python_import = PythonImport {
            level: import.level,
            module: import
                .module
                .as_ref()
                .map(ruff_python_ast::Identifier::as_str),
            importer: self.source.path(),
        };
        let is_star_import = import.names.iter().any(|alias| alias.name.as_str() == "*");
        if is_star_import {
            match self.resolver.resolve_star_import(python_import) {
                ImportSourceResolution::Resolved(imported_source) => {
                    let imported_model = self.analyze_imported_model(state, &imported_source);
                    state.bindings.merge_star_import(&imported_model);
                }
                ImportSourceResolution::ReadFailed { file, path } => {
                    state.effects.add_read_failure(file, path);
                    state.bindings.mark_all_partial();
                }
                ImportSourceResolution::Unresolved | ImportSourceResolution::SkippedExternal => {
                    state.bindings.mark_all_partial();
                }
            }
            return;
        }

        match self.resolver.resolve_named_import(python_import) {
            ImportSourceResolution::Resolved(imported_source) => {
                let imported_model = self.analyze_imported_model(state, &imported_source);
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
                        Self::bind_unknown_name(state, bound_name, self.origin(alias));
                    }
                }
            }
            ImportSourceResolution::ReadFailed { file, path } => {
                state.effects.add_read_failure(file, path);
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    Self::bind_unknown_name(state, bound_name, self.origin(alias));
                }
            }
            ImportSourceResolution::Unresolved | ImportSourceResolution::SkippedExternal => {
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    Self::bind_unknown_name(state, bound_name, self.origin(alias));
                }
            }
        }
    }

    fn evaluate_test_expr(state: &PythonSemanticState, expr: &ast::Expr) -> Truthiness {
        if let Some(name) = expr.name_target() {
            return state
                .bindings
                .bool_value(name)
                .map_or(Truthiness::Ambiguous, Truthiness::from_bool);
        }

        match expr {
            ast::Expr::BooleanLiteral(literal) => Truthiness::from_bool(literal.value),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
                Self::evaluate_test_expr(state, &unary.operand).negate()
            }
            _ => Truthiness::Ambiguous,
        }
    }

    fn evaluate_value(&self, state: &PythonSemanticState, expr: &ast::Expr) -> PythonValue {
        let origin = self.origin(expr);
        let path = evaluate_path(expr, self.source.path(), &state.bindings.path_bindings());

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
            ast::Expr::List(list) => self.evaluate_list(state, &list.elts, origin),
            ast::Expr::Tuple(tuple) => self.evaluate_list(state, &tuple.elts, origin),
            ast::Expr::Dict(dict) => self.evaluate_dict(state, dict, origin),
            ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
                self.evaluate_addition(state, &bin_op.left, &bin_op.right, origin)
            }
            _ => PythonValue::unknown(origin),
        }
    }

    fn evaluate_list(
        &self,
        state: &PythonSemanticState,
        elements: &[ast::Expr],
        origin: Origin,
    ) -> PythonValue {
        let mut values = Vec::new();
        let mut completeness = PythonCompleteness::Full;
        for element in elements {
            if let ast::Expr::Starred(starred) = element {
                let starred = self.evaluate_value(state, &starred.value);
                match starred.kind() {
                    PythonValueKind::List(items) => values.extend(items.clone()),
                    _ => completeness = PythonCompleteness::Partial,
                }
                if !starred.is_complete() {
                    completeness = PythonCompleteness::Partial;
                }
                continue;
            }

            let value = self.evaluate_value(state, element);
            if !value.is_complete() {
                completeness = PythonCompleteness::Partial;
            }
            values.push(value);
        }
        PythonValue::new(PythonValueKind::List(values), origin, completeness)
    }

    fn evaluate_addition(
        &self,
        state: &PythonSemanticState,
        left: &ast::Expr,
        right: &ast::Expr,
        origin: Origin,
    ) -> PythonValue {
        let left = self.evaluate_value(state, left);
        let right = self.evaluate_value(state, right);
        let operands_complete = left.is_complete() && right.is_complete();
        match (left.kind, right.kind) {
            (PythonValueKind::List(mut left), PythonValueKind::List(right)) => {
                left.extend(right);
                let completeness = if operands_complete && left.iter().all(PythonValue::is_complete)
                {
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
        &self,
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

            let key = self.evaluate_value(state, key_expr);
            if let PythonValueKind::Str(key_text) = key.kind()
                && !seen_string_keys.insert(key_text.clone())
            {
                continue;
            }

            let mut value = self.evaluate_value(state, &item.value);
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

    fn bind_unknown_targets(&mut self, state: &mut PythonSemanticState, target: &ast::Expr) {
        for name in target_write_names(target) {
            Self::bind_unknown_name(state, name, self.origin(target));
        }
    }

    fn bind_unknown_name(state: &mut PythonSemanticState, name: &str, origin: Origin) {
        state
            .bindings
            .bind(name, PythonBinding::unknown(name, origin));
    }

    fn record_mutation(state: &mut PythonSemanticState, target: &ast::Expr, method: &str) {
        if let Some(target) = MutationTarget::from_expr(target) {
            state.mutations.push(PythonMutation {
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
        &mut self,
        state: &mut PythonSemanticState,
        names: impl IntoIterator<Item = String>,
    ) {
        for name in names {
            Self::bind_unknown_name(
                state,
                &name,
                Origin::new(self.source.file(), Span::new(0, 0)),
            );
        }
    }

    fn apply_append_mutation(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
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
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| {
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
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        index: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let Some(index) = index.non_negative_integer() else {
            return Self::mark_target_partial(state, target);
        };
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
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
            return Self::mark_target_partial(state, target);
        };
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
            PythonValueKind::List(values) => {
                if let Some(position) = values.iter().position(|item| {
                    matches!(item.kind(), PythonValueKind::Str(candidate) if candidate == value)
                }) {
                    values.remove(position);
                }
                true
            }
            _ => false,
        })
    }

    fn mark_target_partial(state: &mut PythonSemanticState, target: &ast::Expr) -> bool {
        Self::mutate_target(state, target, |value| {
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
}

impl PythonSemanticEvaluator<'_> {
    pub(super) fn evaluate_test(state: &PythonSemanticState, expr: &ast::Expr) -> Truthiness {
        Self::evaluate_test_expr(state, expr)
    }

    pub(super) fn collect_writes(body: &[ast::Stmt]) -> TouchedNames {
        collect_touched_names(body)
    }

    pub(super) fn degrade_writes(&self, state: &mut PythonSemanticState, writes: TouchedNames) {
        if writes.all {
            state.bindings.mark_all_partial();
        }
        state.degrade_names(
            writes.names,
            Origin::new(self.source.file(), Span::new(0, 0)),
        );
    }

    pub(super) fn bind_pattern_name(&self, state: &mut PythonSemanticState, name: &str) {
        state.bindings.bind(
            name,
            PythonBinding::unknown(name, Origin::new(self.source.file(), Span::new(0, 0))),
        );
    }
}
