use std::ops::ControlFlow;

use camino::Utf8Path;
use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::settings::extraction::INSTALLED_APPS;
use crate::settings::extraction::SettingsExtraction;
use crate::settings::extraction::TEMPLATES;
use crate::settings::extraction::bindings::SettingsBindings;
use crate::settings::extraction::bindings::TouchedBindings;
use crate::settings::extraction::env::EvalEnv;
use crate::settings::extraction::installed_apps;
use crate::settings::extraction::templates;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalListBinding;
use crate::settings::types::SettingsImport;
use crate::settings::types::SettingsSourceResolver;
use crate::settings::types::TemplateDirPath;
use crate::settings::types::TemplateSettings;

pub(super) struct SettingsBindingsCollector<'a> {
    bindings: SettingsBindings,
    module_path: &'a Utf8Path,
    resolver: &'a mut dyn SettingsSourceResolver,
    extraction: &'a mut SettingsExtraction,
}

impl<'a> SettingsBindingsCollector<'a> {
    pub(super) fn new(
        module_path: &'a Utf8Path,
        resolver: &'a mut dyn SettingsSourceResolver,
        extraction: &'a mut SettingsExtraction,
    ) -> Self {
        Self {
            bindings: SettingsBindings::default(),
            module_path,
            resolver,
            extraction,
        }
    }

    pub(super) fn into_bindings(self) -> SettingsBindings {
        self.bindings
    }

    pub(super) fn mark_syntax_error(&mut self) {
        self.bindings.mark_installed_apps_partial();
        self.bindings.mark_templates_partial();
    }

    pub(super) fn walk_body(&mut self, body: &[ast::Stmt]) {
        for stmt in body {
            self.walk_stmt(stmt);
        }
    }

    fn walk_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Assign(assign) => self.walk_assign(assign),
            ast::Stmt::AnnAssign(assign) => self.walk_ann_assign(assign),
            ast::Stmt::AugAssign(assign) => self.walk_aug_assign(assign),
            ast::Stmt::Expr(expr) => self.walk_expr(&expr.value),
            ast::Stmt::Import(import) => self.walk_import(&import.names),
            ast::Stmt::ImportFrom(import) => self.walk_import_from(import),
            ast::Stmt::If(stmt_if) => self.walk_if(stmt_if),
            ast::Stmt::For(stmt_for) => {
                self.mark_unknown_targets(&stmt_for.target);
                self.walk_body(&stmt_for.body);
                self.walk_body(&stmt_for.orelse);
            }
            ast::Stmt::While(stmt_while) => {
                self.walk_body(&stmt_while.body);
                self.walk_body(&stmt_while.orelse);
            }
            ast::Stmt::With(stmt_with) => {
                for item in &stmt_with.items {
                    if let Some(optional_vars) = &item.optional_vars {
                        self.mark_unknown_targets(optional_vars);
                    }
                }
                self.walk_body(&stmt_with.body);
            }
            ast::Stmt::Try(stmt_try) => {
                self.walk_body(&stmt_try.body);
                for handler in &stmt_try.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    self.walk_body(&handler.body);
                }
                self.walk_body(&stmt_try.orelse);
                self.walk_body(&stmt_try.finalbody);
            }
            ast::Stmt::FunctionDef(function) => self.mark_definition_name(function.name.as_str()),
            ast::Stmt::ClassDef(class) => self.mark_definition_name(class.name.as_str()),
            ast::Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.mark_unknown_targets(target);
                }
            }
            ast::Stmt::TypeAlias(type_alias) => self.mark_unknown_targets(&type_alias.name),
            ast::Stmt::Return(_)
            | ast::Stmt::Raise(_)
            | ast::Stmt::Assert(_)
            | ast::Stmt::Global(_)
            | ast::Stmt::Nonlocal(_)
            | ast::Stmt::Match(_)
            | ast::Stmt::Pass(_)
            | ast::Stmt::Break(_)
            | ast::Stmt::Continue(_)
            | ast::Stmt::IpyEscapeCommand(_) => {}
        }
    }

    fn walk_assign(&mut self, assign: &ast::StmtAssign) {
        if assign.targets.len() != 1 {
            for target in &assign.targets {
                self.mark_unknown_targets(target);
            }
            return;
        }

        let target = &assign.targets[0];
        if let Some(name) = target.name_target() {
            self.assign_name(name, &assign.value);
        } else {
            self.mark_unknown_targets(target);
        }
    }

    fn walk_ann_assign(&mut self, assign: &ast::StmtAnnAssign) {
        let Some(value) = &assign.value else {
            self.mark_unknown_targets(&assign.target);
            return;
        };

        if let Some(name) = assign.target.name_target() {
            self.assign_name(name, value);
        } else {
            self.mark_unknown_targets(&assign.target);
        }
    }

    fn walk_aug_assign(&mut self, assign: &ast::StmtAugAssign) {
        if assign.op != ast::Operator::Add {
            self.mark_unknown_targets(&assign.target);
            return;
        }

        if assign.target.name_target() == Some(INSTALLED_APPS) {
            self.extend_installed_apps(&assign.value);
        } else if let Some(index) = templates_dirs_target(&assign.target) {
            self.extend_template_dirs(index, &assign.value);
        } else {
            self.mark_unknown_targets(&assign.target);
        }
    }

    fn walk_expr(&mut self, expr: &ast::Expr) {
        let ast::Expr::Call(call) = expr else {
            return;
        };
        let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
            return;
        };

        if attribute.value.name_target() == Some(INSTALLED_APPS) {
            self.apply_installed_apps_call(attribute.attr.as_str(), &call.arguments);
        } else if let Some(name) = attribute.value.name_target()
            && self.bindings.locals.list_binding(name).is_some()
        {
            self.bindings.locals.clear_name(name);
        } else if let Some(index) = templates_dirs_target(&attribute.value) {
            match attribute.attr.as_str() {
                "append"
                    if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() =>
                {
                    let env = EvalEnv::new(self.module_path, &self.bindings);
                    let path = env.evaluate_template_dir_path(&call.arguments.args[0]);
                    self.push_template_dir(index, path);
                }
                "extend"
                    if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() =>
                {
                    self.extend_template_dirs(index, &call.arguments.args[0]);
                }
                _ => self.bindings.mark_templates_unsupported(),
            }
        } else if expr_touches_name(expr, INSTALLED_APPS) {
            self.bindings.mark_installed_apps_unsupported();
        } else if expr_touches_name(expr, TEMPLATES) {
            self.bindings.mark_templates_unsupported();
        }
    }

    fn walk_import(&mut self, aliases: &[ast::Alias]) {
        for alias in aliases {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                |asname| asname.as_str(),
            );
            self.mark_definition_name(bound_name);
        }
    }

    fn walk_import_from(&mut self, import: &ast::StmtImportFrom) {
        let source_import = SettingsImport {
            level: import.level,
            module: import.module.as_ref().map(ToString::to_string),
        };

        let is_star_import = import.names.iter().any(|alias| alias.name.as_str() == "*");
        if is_star_import {
            let imported_bindings = self
                .resolver
                .resolve_star_import(&source_import, self.module_path)
                .and_then(|resolved| {
                    self.extraction
                        .extract_import_source(&resolved, self.resolver)
                });
            if let Some(bindings) = imported_bindings {
                self.bindings.merge_star_import(&bindings);
            } else {
                self.bindings.mark_installed_apps_partial();
                self.bindings.mark_templates_partial();
            }
            return;
        }

        let imported_bindings = self
            .resolver
            .resolve_named_import(&source_import, self.module_path)
            .and_then(|resolved| {
                self.extraction
                    .extract_import_source(&resolved, self.resolver)
            });
        if let Some(imported_bindings) = imported_bindings {
            for alias in &import.names {
                let imported_name = alias.name.as_str();
                let bound_name = alias
                    .asname
                    .as_ref()
                    .map_or_else(|| imported_name, |asname| asname.as_str());
                self.bind_imported_name(imported_name, bound_name, &imported_bindings);
            }
        } else {
            for alias in &import.names {
                let bound_name = alias
                    .asname
                    .as_ref()
                    .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                self.mark_definition_name(bound_name);
            }
        }
    }

    fn walk_if(&mut self, stmt_if: &ast::StmtIf) {
        match self.evaluate_test_expr(&stmt_if.test) {
            Truthiness::AlwaysTrue => self.walk_body(&stmt_if.body),
            Truthiness::AlwaysFalse => self.walk_false_if_clauses(&stmt_if.elif_else_clauses),
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
                self.walk_ambiguous_arms(&arms);
            }
        }
    }

    fn walk_false_if_clauses(&mut self, clauses: &[ast::ElifElseClause]) {
        for (index, clause) in clauses.iter().enumerate() {
            let Some(test) = &clause.test else {
                self.walk_body(&clause.body);
                return;
            };

            match self.evaluate_test_expr(test) {
                Truthiness::AlwaysTrue => {
                    self.walk_body(&clause.body);
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
                    self.walk_ambiguous_arms(&arms);
                    return;
                }
            }
        }
    }

    fn walk_ambiguous_arms(&mut self, arms: &[&[ast::Stmt]]) {
        let mut writes = TouchedBindings::default();
        for arm in arms {
            writes.merge(&collect_watched_writes(arm));
        }

        let base = std::mem::take(&mut self.bindings);
        let mut branch_bindings = Vec::with_capacity(arms.len());
        for arm in arms {
            self.bindings = base.clone();
            self.walk_body(arm);
            branch_bindings.push(std::mem::take(&mut self.bindings));
        }
        self.bindings = base.join_ambiguous(&branch_bindings, &writes);
    }

    fn evaluate_test_expr(&self, expr: &ast::Expr) -> Truthiness {
        if let Some(name) = expr.name_target() {
            return self
                .bindings
                .locals
                .bool_value(name)
                .map_or(Truthiness::Ambiguous, Truthiness::from_bool);
        }

        match expr {
            ast::Expr::BooleanLiteral(literal) => Truthiness::from_bool(literal.value),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
                self.evaluate_test_expr(&unary.operand).negate()
            }
            _ => Truthiness::Ambiguous,
        }
    }

    fn assign_name(&mut self, name: &str, value: &ast::Expr) {
        match name {
            INSTALLED_APPS => self.assign_installed_apps(value),
            TEMPLATES => self.assign_templates(value),
            _ => self.assign_aux(name, value),
        }
    }

    fn bind_imported_name(
        &mut self,
        imported_name: &str,
        bound_name: &str,
        imported_bindings: &SettingsBindings,
    ) {
        match imported_name {
            INSTALLED_APPS => {
                let Some(setting) = &imported_bindings.installed_apps else {
                    self.mark_definition_name(bound_name);
                    return;
                };
                if bound_name == INSTALLED_APPS {
                    self.bindings.installed_apps = Some(setting.clone());
                } else {
                    let list = if setting.is_fully_extracted() {
                        LocalListBinding::full(setting.values.clone())
                    } else {
                        LocalListBinding::partial(setting.values.clone())
                    };
                    self.bindings.locals.set_list(bound_name, list);
                }
            }
            TEMPLATES => {
                if bound_name == TEMPLATES {
                    if let Some(templates) = &imported_bindings.templates {
                        self.bindings.templates = Some(templates.clone());
                    } else {
                        self.mark_definition_name(bound_name);
                    }
                } else {
                    self.mark_definition_name(bound_name);
                }
            }
            _ => {
                if !self.bindings.locals.bind_imported_local(
                    &imported_bindings.locals,
                    imported_name,
                    bound_name,
                ) {
                    self.mark_definition_name(bound_name);
                }
            }
        }
    }

    fn assign_aux(&mut self, name: &str, value: &ast::Expr) {
        let env = EvalEnv::new(self.module_path, &self.bindings);
        match installed_apps::evaluate_local_list_assignment(value, &env) {
            Some(extracted) => self.bindings.locals.set_list(name, extracted.into()),
            None => self.bindings.locals.remove_list(name),
        }

        match value.bool_literal() {
            Some(value) => self.bindings.locals.set_bool(name, value),
            None => self.bindings.locals.remove_bool(name),
        }

        let env = EvalEnv::new(self.module_path, &self.bindings);
        match env.evaluate_template_dir_path(value) {
            TemplateDirPath::Resolved(path) => self.bindings.locals.set_path(name, path),
            TemplateDirPath::Unknown => self.bindings.locals.remove_path(name),
        }
    }

    fn assign_installed_apps(&mut self, value: &ast::Expr) {
        let env = EvalEnv::new(self.module_path, &self.bindings);
        match installed_apps::evaluate_assignment(value, &env) {
            installed_apps::AssignmentEffect::Assign(extracted) => {
                let status = extracted.status;
                self.bindings.installed_apps = Some(InstalledAppsSetting::full(extracted.values));
                if !status.is_complete() {
                    self.bindings.mark_installed_apps_partial();
                }
            }
            installed_apps::AssignmentEffect::Unsupported => {
                self.bindings.mark_installed_apps_unsupported();
            }
        }
    }

    fn extend_installed_apps(&mut self, value: &ast::Expr) {
        if !self.bindings.can_mutate_installed_apps() {
            self.bindings.mark_installed_apps_unsupported();
            return;
        }

        let env = EvalEnv::new(self.module_path, &self.bindings);
        let extracted = installed_apps::evaluate_list_operand(value, &env);
        let setting = self
            .bindings
            .installed_apps
            .as_mut()
            .expect("can_mutate_installed_apps requires an installed apps value");
        setting.values.extend(extracted.values);
        if !extracted.status.is_complete() {
            self.bindings.mark_installed_apps_partial();
        }
    }

    fn apply_installed_apps_call(&mut self, method: &str, arguments: &ast::Arguments) {
        if !self.bindings.can_mutate_installed_apps() {
            self.bindings.mark_installed_apps_unsupported();
            return;
        }

        match method {
            "append" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                if let Some(value) = arguments.args[0].string_literal() {
                    let setting = self
                        .bindings
                        .installed_apps
                        .as_mut()
                        .expect("can_mutate_installed_apps requires an installed apps value");
                    setting.values.push(value.to_string());
                } else {
                    self.bindings.mark_installed_apps_partial();
                }
            }
            "extend" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                self.extend_installed_apps(&arguments.args[0]);
            }
            "insert" if arguments.args.len() == 2 && arguments.keywords.is_empty() => {
                let index = arguments.args[0].non_negative_integer();
                let value = arguments.args[1].string_literal();
                match (index, value) {
                    (Some(index), Some(value)) => {
                        let setting =
                            self.bindings.installed_apps.as_mut().expect(
                                "can_mutate_installed_apps requires an installed apps value",
                            );
                        let index = index.min(setting.values.len());
                        setting.values.insert(index, value.to_string());
                    }
                    _ => self.bindings.mark_installed_apps_partial(),
                }
            }
            "remove" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                if let Some(value) = arguments.args[0].string_literal() {
                    let setting = self
                        .bindings
                        .installed_apps
                        .as_mut()
                        .expect("can_mutate_installed_apps requires an installed apps value");
                    if let Some(position) = setting.values.iter().position(|item| item == value) {
                        setting.values.remove(position);
                    }
                } else {
                    self.bindings.mark_installed_apps_partial();
                }
            }
            _ => self.bindings.mark_installed_apps_unsupported(),
        }
    }

    fn assign_templates(&mut self, value: &ast::Expr) {
        let env = EvalEnv::new(self.module_path, &self.bindings);
        match templates::evaluate_assignment(value, &env) {
            templates::AssignmentEffect::Assign(backends, completeness) => {
                self.bindings.templates = Some(TemplateSettings::full(backends));
                if completeness == templates::AssignmentCompleteness::Partial {
                    self.bindings.mark_templates_partial();
                }
            }
            templates::AssignmentEffect::Unsupported => self.bindings.mark_templates_unsupported(),
        }
    }

    fn extend_template_dirs(&mut self, index: usize, value: &ast::Expr) {
        let env = EvalEnv::new(self.module_path, &self.bindings);
        match templates::evaluate_dirs_extension(value, &env) {
            templates::DirsExtensionEffect::Extend(paths) => {
                for path in paths {
                    self.push_template_dir(index, path);
                }
            }
            templates::DirsExtensionEffect::Partial => self.bindings.mark_templates_partial(),
        }
    }

    fn push_template_dir(&mut self, index: usize, path: TemplateDirPath) {
        let path_is_unknown = path == TemplateDirPath::Unknown;

        let Some(templates) = self.bindings.templates.as_mut() else {
            self.bindings.mark_templates_partial();
            return;
        };
        let Some(backend) = templates.backends.get_mut(index) else {
            self.bindings.mark_templates_partial();
            return;
        };
        if path_is_unknown {
            backend.mark_partial();
        }
        backend.dirs.push(path);
        if path_is_unknown {
            self.bindings.mark_templates_partial();
        }
    }

    fn mark_unknown_targets(&mut self, target: &ast::Expr) {
        if target_touches_name(target, INSTALLED_APPS) {
            self.bindings.mark_installed_apps_unsupported();
        }
        if target_touches_name(target, TEMPLATES) {
            self.bindings.mark_templates_unsupported();
        }
        clear_local_target_names(target, &mut |name| self.bindings.locals.clear_name(name));
    }

    fn mark_definition_name(&mut self, name: &str) {
        match name {
            INSTALLED_APPS => self.bindings.mark_installed_apps_unsupported(),
            TEMPLATES => self.bindings.mark_templates_unsupported(),
            _ => self.bindings.locals.clear_name(name),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truthiness {
    AlwaysTrue,
    AlwaysFalse,
    Ambiguous,
}

impl Truthiness {
    const fn from_bool(value: bool) -> Self {
        if value {
            Self::AlwaysTrue
        } else {
            Self::AlwaysFalse
        }
    }

    const fn negate(self) -> Self {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Ambiguous => Self::Ambiguous,
        }
    }
}

/// Deliberately a separate pass from the collector walk: the walk skips
/// statically-false branches, but join conservatism must see writes in every
/// arm, dead or not.
fn collect_watched_writes(body: &[ast::Stmt]) -> TouchedBindings {
    let mut writes = TouchedBindings::default();
    walk_stmts(body, Recurse::Flat, |stmt| {
        record_stmt_writes(stmt, &mut writes);
        ControlFlow::Continue(())
    });
    writes
}

fn record_stmt_writes(stmt: &ast::Stmt, writes: &mut TouchedBindings) {
    match stmt {
        ast::Stmt::Assign(assign) => {
            for target in &assign.targets {
                record_target_writes(target, writes);
            }
        }
        ast::Stmt::AnnAssign(assign) => record_target_writes(&assign.target, writes),
        ast::Stmt::AugAssign(assign) => record_target_writes(&assign.target, writes),
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                record_target_writes(target, writes);
            }
        }
        ast::Stmt::For(stmt_for) => {
            record_target_writes(&stmt_for.target, writes);
            writes.merge(&collect_watched_writes(&stmt_for.body));
            writes.merge(&collect_watched_writes(&stmt_for.orelse));
        }
        ast::Stmt::While(stmt_while) => {
            writes.merge(&collect_watched_writes(&stmt_while.body));
            writes.merge(&collect_watched_writes(&stmt_while.orelse));
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    record_target_writes(optional_vars, writes);
                }
            }
            writes.merge(&collect_watched_writes(&stmt_with.body));
        }
        ast::Stmt::Try(stmt_try) => {
            writes.merge(&collect_watched_writes(&stmt_try.body));
            for handler in &stmt_try.handlers {
                let ast::ExceptHandler::ExceptHandler(handler) = handler;
                writes.merge(&collect_watched_writes(&handler.body));
            }
            writes.merge(&collect_watched_writes(&stmt_try.orelse));
            writes.merge(&collect_watched_writes(&stmt_try.finalbody));
        }
        ast::Stmt::If(stmt_if) => {
            writes.merge(&collect_watched_writes(&stmt_if.body));
            for clause in &stmt_if.elif_else_clauses {
                writes.merge(&collect_watched_writes(&clause.body));
            }
        }
        ast::Stmt::Expr(expr) => {
            if expr_touches_name(&expr.value, INSTALLED_APPS) {
                writes.installed_apps = true;
            }
            if expr_touches_name(&expr.value, TEMPLATES) {
                writes.templates = true;
            }
            record_expr_local_mutations(&expr.value, writes);
        }
        ast::Stmt::Import(import) => {
            for alias in &import.names {
                let bound_name = alias.asname.as_ref().map_or_else(
                    || first_import_segment(alias.name.as_str()),
                    |asname| asname.as_str(),
                );
                record_name_write(bound_name, writes);
            }
        }
        ast::Stmt::ImportFrom(import) => {
            for alias in &import.names {
                if alias.name.as_str() == "*" {
                    writes.installed_apps = true;
                    writes.templates = true;
                } else {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    record_name_write(bound_name, writes);
                }
            }
        }
        ast::Stmt::FunctionDef(function) => record_name_write(function.name.as_str(), writes),
        ast::Stmt::ClassDef(class) => record_name_write(class.name.as_str(), writes),
        ast::Stmt::TypeAlias(type_alias) => record_target_writes(&type_alias.name, writes),
        ast::Stmt::Return(_)
        | ast::Stmt::Raise(_)
        | ast::Stmt::Assert(_)
        | ast::Stmt::Global(_)
        | ast::Stmt::Nonlocal(_)
        | ast::Stmt::Match(_)
        | ast::Stmt::Pass(_)
        | ast::Stmt::Break(_)
        | ast::Stmt::Continue(_)
        | ast::Stmt::IpyEscapeCommand(_) => {}
    }
}

fn record_target_writes(target: &ast::Expr, writes: &mut TouchedBindings) {
    if target_touches_name(target, INSTALLED_APPS) {
        writes.installed_apps = true;
    }
    if target_touches_name(target, TEMPLATES) {
        writes.templates = true;
    }
    record_local_target_writes(target, writes);
}

fn record_local_target_writes(target: &ast::Expr, writes: &mut TouchedBindings) {
    if let Some(name) = target.name_target() {
        if !matches!(name, INSTALLED_APPS | TEMPLATES) {
            writes.record_local(name);
        }
        return;
    }

    match target {
        ast::Expr::Attribute(attribute) => record_local_target_writes(&attribute.value, writes),
        ast::Expr::Subscript(subscript) => record_local_target_writes(&subscript.value, writes),
        ast::Expr::Tuple(tuple) => {
            for expr in &tuple.elts {
                record_local_target_writes(expr, writes);
            }
        }
        ast::Expr::List(list) => {
            for expr in &list.elts {
                record_local_target_writes(expr, writes);
            }
        }
        ast::Expr::Starred(starred) => record_local_target_writes(&starred.value, writes),
        _ => {}
    }
}

fn record_expr_local_mutations(expr: &ast::Expr, writes: &mut TouchedBindings) {
    let ast::Expr::Call(call) = expr else {
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        return;
    };
    if let Some(name) = attribute.value.name_target()
        && !matches!(name, INSTALLED_APPS | TEMPLATES)
    {
        writes.record_local(name);
    }
}

fn record_name_write(name: &str, writes: &mut TouchedBindings) {
    match name {
        INSTALLED_APPS => writes.installed_apps = true,
        TEMPLATES => writes.templates = true,
        _ => writes.record_local(name),
    }
}

fn templates_dirs_target(expr: &ast::Expr) -> Option<usize> {
    let ast::Expr::Subscript(outer) = expr else {
        return None;
    };
    if outer.slice.string_literal() != Some("DIRS") {
        return None;
    }
    let ast::Expr::Subscript(inner) = outer.value.as_ref() else {
        return None;
    };
    if inner.value.name_target() != Some(TEMPLATES) {
        return None;
    }
    inner.slice.non_negative_integer()
}

fn clear_local_target_names(target: &ast::Expr, clear: &mut impl FnMut(&str)) {
    if let Some(name) = target.name_target() {
        if !matches!(name, INSTALLED_APPS | TEMPLATES) {
            clear(name);
        }
        return;
    }

    match target {
        ast::Expr::Attribute(attribute) => clear_local_target_names(&attribute.value, clear),
        ast::Expr::Subscript(subscript) => clear_local_target_names(&subscript.value, clear),
        ast::Expr::Tuple(tuple) => {
            for expr in &tuple.elts {
                clear_local_target_names(expr, clear);
            }
        }
        ast::Expr::List(list) => {
            for expr in &list.elts {
                clear_local_target_names(expr, clear);
            }
        }
        ast::Expr::Starred(starred) => clear_local_target_names(&starred.value, clear),
        _ => {}
    }
}

fn target_touches_name(target: &ast::Expr, expected: &str) -> bool {
    match target {
        expr if expr.name_target() == Some(expected) => true,
        ast::Expr::Attribute(attribute) => target_touches_name(&attribute.value, expected),
        ast::Expr::Subscript(subscript) => target_touches_name(&subscript.value, expected),
        ast::Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .any(|expr| target_touches_name(expr, expected)),
        ast::Expr::List(list) => list
            .elts
            .iter()
            .any(|expr| target_touches_name(expr, expected)),
        ast::Expr::Starred(starred) => target_touches_name(&starred.value, expected),
        _ => false,
    }
}

fn expr_touches_name(expr: &ast::Expr, expected: &str) -> bool {
    match expr {
        expr if expr.name_target() == Some(expected) => true,
        ast::Expr::Attribute(attribute) => expr_touches_name(&attribute.value, expected),
        ast::Expr::Subscript(subscript) => expr_touches_name(&subscript.value, expected),
        ast::Expr::Call(call) => expr_touches_name(&call.func, expected),
        ast::Expr::BinOp(bin_op) => {
            expr_touches_name(&bin_op.left, expected) || expr_touches_name(&bin_op.right, expected)
        }
        ast::Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .any(|expr| expr_touches_name(expr, expected)),
        ast::Expr::List(list) => list
            .elts
            .iter()
            .any(|expr| expr_touches_name(expr, expected)),
        ast::Expr::Starred(starred) => expr_touches_name(&starred.value, expected),
        _ => false,
    }
}

fn first_import_segment(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}
