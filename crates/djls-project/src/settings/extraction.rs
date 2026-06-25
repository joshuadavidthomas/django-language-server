//! Bounded Django settings extraction.
//!
//! This extractor intentionally recognizes a small set of settings idioms. For
//! string-list settings such as `INSTALLED_APPS`, unsupported elements are
//! skipped and the setting becomes [`StaticKnowledge::Partial`] instead of failing
//! the whole list. That differs from ty's `__all__` collector, but it matches
//! Django settings in practice: one environment-driven entry should not hide
//! the static entries around it.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast as ast;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::ast::ExprExt;
use crate::settings::paths::evaluate_path_expr;
use crate::settings::types::DjangoSettings;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalBindings;
use crate::settings::types::Reason;
use crate::settings::types::SettingsSource;
use crate::settings::types::SettingsSourceResolver;
use crate::settings::types::SettingsStarImport;
use crate::settings::types::StaticKnowledge;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateDirPath;
use crate::settings::types::TemplateSettings;

const INSTALLED_APPS: &str = "INSTALLED_APPS";
const TEMPLATES: &str = "TEMPLATES";

/// Extract Django settings from Python source.
#[must_use]
pub(crate) fn extract_settings(
    source: &str,
    module_path: &Utf8Path,
    resolver: &mut dyn SettingsSourceResolver,
) -> DjangoSettings {
    let mut extraction = SettingsExtraction::default();
    extraction
        .extract_module(source, module_path, resolver)
        .into_settings()
}

#[derive(Debug, Default)]
struct SettingsExtraction {
    active: FxHashSet<Utf8PathBuf>,
    cache: FxHashMap<Utf8PathBuf, SettingsBindings>,
}

impl SettingsExtraction {
    fn extract_module(
        &mut self,
        source: &str,
        module_path: &Utf8Path,
        resolver: &mut dyn SettingsSourceResolver,
    ) -> SettingsBindings {
        let path = module_path.to_path_buf();
        if let Some(cached) = self.cache.get(&path) {
            return cached.clone();
        }
        if !self.active.insert(path.clone()) {
            return SettingsBindings::new();
        }

        let mut collector = SettingsBindingsCollector {
            bindings: SettingsBindings::new(),
            module_path,
            resolver,
            extraction: self,
        };

        if let Ok(parsed) = parse_module(source) {
            let module = parsed.into_syntax();
            collector.walk_body(&module.body);
        } else {
            collector.mark_syntax_error();
        }

        let bindings = collector.into_bindings();
        self.active.remove(&path);
        self.cache.insert(path, bindings.clone());
        bindings
    }

    fn extract_star_import(
        &mut self,
        imported: &SettingsSource,
        star_imports: &mut dyn SettingsSourceResolver,
    ) -> Option<SettingsBindings> {
        if self.active.contains(&imported.path) {
            return None;
        }
        Some(self.extract_module(&imported.source, &imported.path, star_imports))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SettingsBindings {
    installed_apps: Option<InstalledAppsSetting>,
    templates: Option<TemplateSettings>,
    locals: LocalBindings,
}

impl SettingsBindings {
    fn new() -> Self {
        Self::default()
    }

    fn into_settings(self) -> DjangoSettings {
        DjangoSettings {
            installed_apps: self.installed_apps.unwrap_or_default(),
            templates: self.templates.unwrap_or_default(),
        }
    }

    fn merge_star_import(&mut self, other: Self) {
        if let Some(installed_apps) = other.installed_apps {
            self.installed_apps = Some(installed_apps);
        }
        if let Some(templates) = other.templates {
            self.templates = Some(templates);
        }
        self.locals.extend(other.locals);
    }

    fn assign_installed_apps(&mut self, values: Vec<String>) {
        self.installed_apps = Some(InstalledAppsSetting::known(values));
    }

    fn installed_apps_mut(&mut self) -> &mut InstalledAppsSetting {
        self.installed_apps
            .get_or_insert_with(InstalledAppsSetting::default)
    }

    fn make_installed_apps_unknown(&mut self, reason: Reason) {
        self.installed_apps_mut().make_unknown(reason);
    }

    fn can_mutate_installed_apps(&self) -> bool {
        self.installed_apps
            .as_ref()
            .is_some_and(|setting| setting.knowledge != StaticKnowledge::Unknown)
    }

    fn assign_templates(&mut self, backends: Vec<TemplateBackend>) {
        self.templates = Some(TemplateSettings::known(backends));
    }

    fn templates_mut(&mut self) -> &mut TemplateSettings {
        self.templates.get_or_insert_with(TemplateSettings::default)
    }

    fn make_templates_partial(&mut self) {
        match &mut self.templates {
            Some(templates) => templates.make_partial(),
            None => self.templates = Some(TemplateSettings::partial()),
        }
    }

    fn make_templates_unknown(&mut self) {
        self.templates_mut().make_unknown();
    }
}

struct SettingsBindingsCollector<'a> {
    bindings: SettingsBindings,
    module_path: &'a Utf8Path,
    resolver: &'a mut dyn SettingsSourceResolver,
    extraction: &'a mut SettingsExtraction,
}

impl SettingsBindingsCollector<'_> {
    fn into_bindings(self) -> SettingsBindings {
        self.bindings
    }

    fn mark_syntax_error(&mut self) {
        self.bindings
            .installed_apps_mut()
            .make_partial(Reason::SyntaxErrors);
        self.bindings.make_templates_partial();
    }

    fn walk_body(&mut self, body: &[ast::Stmt]) {
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
        } else if let Some(index) = templates_dirs_target(&attribute.value) {
            self.apply_template_dirs_call(index, attribute.attr.as_str(), &call.arguments);
        } else if expr_touches_name(expr, INSTALLED_APPS) {
            self.bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation);
        } else if expr_touches_name(expr, TEMPLATES) {
            self.bindings.make_templates_unknown();
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
        let is_star_import = import.names.iter().any(|alias| alias.name.as_str() == "*");
        if is_star_import {
            let star_import = SettingsStarImport {
                level: import.level,
                module: import.module.as_ref().map(ToString::to_string),
            };
            if let Some(bindings) = self
                .resolver
                .resolve_star_import(&star_import, self.module_path)
                .and_then(|resolved| {
                    self.extraction
                        .extract_star_import(&resolved, self.resolver)
                })
            {
                self.bindings.merge_star_import(bindings);
            } else {
                self.bindings
                    .installed_apps_mut()
                    .make_partial(Reason::UnresolvedSettingsStarImport);
                self.bindings.make_templates_partial();
            }
            return;
        }

        for alias in &import.names {
            let bound_name = alias
                .asname
                .as_ref()
                .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
            self.mark_definition_name(bound_name);
        }
    }

    fn walk_if(&mut self, stmt_if: &ast::StmtIf) {
        match self.evaluate_test_expr(&stmt_if.test) {
            Truthiness::AlwaysTrue => self.walk_body(&stmt_if.body),
            Truthiness::AlwaysFalse => self.walk_false_if_clauses(&stmt_if.elif_else_clauses),
            Truthiness::Ambiguous => {
                let arms = ambiguous_if_arms(&stmt_if.body, &stmt_if.elif_else_clauses);
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
                    let arms = ambiguous_clause_arms(&clauses[index..]);
                    self.walk_ambiguous_arms(&arms);
                    return;
                }
            }
        }
    }

    fn walk_ambiguous_arms(&mut self, arms: &[&[ast::Stmt]]) {
        let mut writes = TouchedNames::default();
        for arm in arms {
            writes.merge(collect_watched_writes(arm));
        }

        let base = self.bindings.clone();
        let mut branch_bindings = Vec::with_capacity(arms.len());
        for arm in arms {
            self.bindings = base.clone();
            self.walk_body(arm);
            branch_bindings.push(self.bindings.clone());
        }
        self.bindings = join_ambiguous_bindings(base, &branch_bindings, writes);
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
            ast::Expr::BooleanLiteral(literal) => {
                if literal.value {
                    Truthiness::AlwaysTrue
                } else {
                    Truthiness::AlwaysFalse
                }
            }
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

    fn assign_aux(&mut self, name: &str, value: &ast::Expr) {
        match value.bool_literal() {
            Some(value) => self.bindings.locals.set_bool(name, value),
            None => self.bindings.locals.remove_bool(name),
        }

        match evaluate_path_expr(value, self.module_path, &self.bindings.locals) {
            TemplateDirPath::Resolved(path) => self.bindings.locals.set_path(name, path),
            TemplateDirPath::Unknown => self.bindings.locals.remove_path(name),
        }
    }

    fn assign_installed_apps(&mut self, value: &ast::Expr) {
        let Some((values, reasons)) = self.extract_string_list_assignment(value) else {
            self.bindings
                .make_installed_apps_unknown(Reason::UnsupportedAssignment);
            return;
        };

        self.bindings.assign_installed_apps(values);
        for reason in reasons {
            self.bindings.installed_apps_mut().make_partial(reason);
        }
    }

    fn extend_installed_apps(&mut self, value: &ast::Expr) {
        if !self.bindings.can_mutate_installed_apps() {
            self.bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation);
            return;
        }

        let (values, reasons) = self.extract_string_list_operand(value);
        self.bindings.installed_apps_mut().values.extend(values);
        for reason in reasons {
            self.bindings.installed_apps_mut().make_partial(reason);
        }
    }

    fn apply_installed_apps_call(&mut self, method: &str, arguments: &ast::Arguments) {
        if !self.bindings.can_mutate_installed_apps() {
            self.bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation);
            return;
        }

        match method {
            "append" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                if let Some(value) = arguments.args[0].string_literal() {
                    self.bindings
                        .installed_apps_mut()
                        .values
                        .push(value.to_string());
                } else {
                    self.bindings
                        .installed_apps_mut()
                        .make_partial(Reason::NonLiteralElement);
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
                        let values = &mut self.bindings.installed_apps_mut().values;
                        let index = index.min(values.len());
                        values.insert(index, value.to_string());
                    }
                    _ => self
                        .bindings
                        .installed_apps_mut()
                        .make_partial(Reason::UnsupportedMutation),
                }
            }
            "remove" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                if let Some(value) = arguments.args[0].string_literal() {
                    if let Some(position) = self
                        .bindings
                        .installed_apps_mut()
                        .values
                        .iter()
                        .position(|item| item == value)
                    {
                        self.bindings.installed_apps_mut().values.remove(position);
                    }
                } else {
                    self.bindings
                        .installed_apps_mut()
                        .make_partial(Reason::NonLiteralElement);
                }
            }
            _ => self
                .bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation),
        }
    }

    fn extract_string_list_assignment(
        &self,
        value: &ast::Expr,
    ) -> Option<(Vec<String>, Vec<Reason>)> {
        match value {
            ast::Expr::List(_)
            | ast::Expr::Tuple(_)
            | ast::Expr::BinOp(ast::ExprBinOp {
                op: ast::Operator::Add,
                ..
            }) => Some(self.extract_string_list_operand(value)),
            expr if expr.name_target() == Some(INSTALLED_APPS) => {
                Some(self.extract_string_list_operand(value))
            }
            _ => None,
        }
    }

    fn extract_string_list_operand(&self, value: &ast::Expr) -> (Vec<String>, Vec<Reason>) {
        match value {
            ast::Expr::List(list) => extract_string_elements(&list.elts),
            ast::Expr::Tuple(tuple) => extract_string_elements(&tuple.elts),
            ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
                let mut values = Vec::new();
                let mut reasons = Vec::new();
                for operand in flatten_addition(value) {
                    let (operand_values, operand_reasons) =
                        self.extract_string_list_operand(operand);
                    values.extend(operand_values);
                    reasons.extend(operand_reasons);
                }
                (values, reasons)
            }
            expr if expr.name_target() == Some(INSTALLED_APPS) => {
                self.bindings.installed_apps.as_ref().map_or_else(
                    || (Vec::new(), vec![Reason::UnsupportedValue]),
                    |setting| (setting.values.clone(), setting.reasons.clone()),
                )
            }
            _ => (Vec::new(), vec![Reason::UnsupportedValue]),
        }
    }

    fn assign_templates(&mut self, value: &ast::Expr) {
        let ast::Expr::List(list) = value else {
            self.bindings.make_templates_unknown();
            return;
        };

        let mut backends = Vec::new();
        let mut templates_partial = false;
        for element in &list.elts {
            let ast::Expr::Dict(dict) = element else {
                templates_partial = true;
                continue;
            };
            backends.push(self.extract_template_backend(dict));
        }

        self.bindings.assign_templates(backends);
        if templates_partial {
            self.bindings.make_templates_partial();
        }
        if self.bindings.templates.as_ref().is_some_and(|templates| {
            templates
                .backends
                .iter()
                .any(|backend| backend.knowledge != StaticKnowledge::Known)
        }) {
            self.bindings.make_templates_partial();
        }
    }

    fn extract_template_backend(&self, dict: &ast::ExprDict) -> TemplateBackend {
        let mut backend = TemplateBackend::default();
        for item in &dict.items {
            let Some(key_expr) = &item.key else {
                backend.make_partial(Reason::DictUnpack);
                continue;
            };
            let Some(key) = key_expr.string_literal() else {
                backend.make_partial(Reason::NonLiteralKey);
                continue;
            };
            match key {
                "BACKEND" => match item.value.string_literal() {
                    Some(value) => backend.backend = Some(value.to_string()),
                    None => backend.make_partial(Reason::UnsupportedValue),
                },
                "DIRS" => self.extract_template_dirs(&item.value, &mut backend),
                "APP_DIRS" => match item.value.bool_literal() {
                    Some(value) => backend.app_dirs = Some(value),
                    None => backend.make_partial(Reason::UnsupportedValue),
                },
                "OPTIONS" => Self::extract_template_options(&item.value, &mut backend),
                _ => {}
            }
        }
        backend
    }

    fn extract_template_dirs(&self, value: &ast::Expr, backend: &mut TemplateBackend) {
        let ast::Expr::List(list) = value else {
            backend.make_partial(Reason::UnsupportedValue);
            return;
        };
        for element in &list.elts {
            let path = evaluate_path_expr(element, self.module_path, &self.bindings.locals);
            if path == TemplateDirPath::Unknown {
                backend.make_partial(Reason::UnsupportedPathExpression);
            }
            backend.dirs.push(path);
        }
    }

    fn extract_template_options(value: &ast::Expr, backend: &mut TemplateBackend) {
        let ast::Expr::Dict(dict) = value else {
            backend.make_partial(Reason::UnsupportedValue);
            return;
        };

        for item in &dict.items {
            let Some(key_expr) = &item.key else {
                backend.make_partial(Reason::DictUnpack);
                continue;
            };
            let Some(key) = key_expr.string_literal() else {
                backend.make_partial(Reason::NonLiteralKey);
                continue;
            };
            match key {
                "libraries" => {
                    let (libraries, reasons) = extract_string_pair_dict(&item.value);
                    backend.libraries.extend(libraries);
                    for reason in reasons {
                        backend.make_partial(reason);
                    }
                }
                "builtins" => {
                    let (builtins, reasons) = extract_string_list_literal(&item.value);
                    backend.builtins.extend(builtins);
                    for reason in reasons {
                        backend.make_partial(reason);
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_template_dirs_call(&mut self, index: usize, method: &str, arguments: &ast::Arguments) {
        match method {
            "append" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                self.append_template_dir(index, &arguments.args[0]);
            }
            "extend" if arguments.args.len() == 1 && arguments.keywords.is_empty() => {
                self.extend_template_dirs(index, &arguments.args[0]);
            }
            _ => self.bindings.make_templates_unknown(),
        }
    }

    fn append_template_dir(&mut self, index: usize, value: &ast::Expr) {
        let path = evaluate_path_expr(value, self.module_path, &self.bindings.locals);
        let path_is_unknown = path == TemplateDirPath::Unknown;

        let Some(templates) = self.bindings.templates.as_mut() else {
            self.bindings.make_templates_partial();
            return;
        };
        let Some(backend) = templates.backends.get_mut(index) else {
            self.bindings.make_templates_partial();
            return;
        };
        if path_is_unknown {
            backend.make_partial(Reason::UnsupportedPathExpression);
        }
        backend.dirs.push(path);
        if path_is_unknown {
            self.bindings.make_templates_partial();
        }
    }

    fn extend_template_dirs(&mut self, index: usize, value: &ast::Expr) {
        let ast::Expr::List(list) = value else {
            self.bindings.make_templates_partial();
            return;
        };

        for element in &list.elts {
            self.append_template_dir(index, element);
        }
    }

    fn mark_unknown_targets(&mut self, target: &ast::Expr) {
        if target_touches_name(target, INSTALLED_APPS) {
            self.bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation);
        }
        if target_touches_name(target, TEMPLATES) {
            self.bindings.make_templates_unknown();
        }
    }

    fn mark_definition_name(&mut self, name: &str) {
        match name {
            INSTALLED_APPS => self
                .bindings
                .make_installed_apps_unknown(Reason::UnsupportedMutation),
            TEMPLATES => self.bindings.make_templates_unknown(),
            _ => {}
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

#[derive(Default, Clone, Copy)]
struct TouchedNames {
    installed_apps: bool,
    templates: bool,
}

impl TouchedNames {
    fn merge(&mut self, other: Self) {
        self.installed_apps |= other.installed_apps;
        self.templates |= other.templates;
    }
}

fn ambiguous_if_arms<'a>(
    body: &'a [ast::Stmt],
    clauses: &'a [ast::ElifElseClause],
) -> Vec<&'a [ast::Stmt]> {
    let mut arms = Vec::with_capacity(clauses.len() + 2);
    arms.push(body);
    arms.extend(clauses.iter().map(|clause| clause.body.as_slice()));
    if !clauses.iter().any(|clause| clause.test.is_none()) {
        arms.push(&[]);
    }
    arms
}

fn ambiguous_clause_arms(clauses: &[ast::ElifElseClause]) -> Vec<&[ast::Stmt]> {
    let mut arms: Vec<&[ast::Stmt]> = clauses
        .iter()
        .map(|clause| clause.body.as_slice())
        .collect();
    if !clauses.iter().any(|clause| clause.test.is_none()) {
        arms.push(&[]);
    }
    arms
}

fn join_ambiguous_bindings(
    mut base: SettingsBindings,
    branch_bindings: &[SettingsBindings],
    writes: TouchedNames,
) -> SettingsBindings {
    if writes.installed_apps {
        base.installed_apps = Some(join_installed_apps(branch_bindings));
    }
    if writes.templates {
        base.templates = Some(TemplateSettings {
            backends: join_template_backends(branch_bindings),
            knowledge: StaticKnowledge::Partial,
        });
    }
    base
}

fn join_installed_apps(branch_bindings: &[SettingsBindings]) -> InstalledAppsSetting {
    let mut values = Vec::new();
    let mut reasons = vec![Reason::AmbiguousCondition];

    for bindings in branch_bindings {
        let Some(setting) = &bindings.installed_apps else {
            continue;
        };
        for value in &setting.values {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
        reasons.extend(setting.reasons.clone());
    }

    InstalledAppsSetting {
        values,
        knowledge: StaticKnowledge::Partial,
        reasons,
    }
}

fn join_template_backends(branch_bindings: &[SettingsBindings]) -> Vec<TemplateBackend> {
    let mut backends = Vec::new();
    for bindings in branch_bindings {
        let Some(templates) = &bindings.templates else {
            continue;
        };
        for backend in &templates.backends {
            if !backends.contains(backend) {
                backends.push(backend.clone());
            }
        }
    }
    backends
}

fn collect_watched_writes(body: &[ast::Stmt]) -> TouchedNames {
    let mut writes = TouchedNames::default();
    for stmt in body {
        collect_stmt_writes(stmt, &mut writes);
    }
    writes
}

fn collect_stmt_writes(stmt: &ast::Stmt, writes: &mut TouchedNames) {
    match stmt {
        ast::Stmt::Assign(assign) => {
            for target in &assign.targets {
                collect_target_writes(target, writes);
            }
        }
        ast::Stmt::AnnAssign(assign) => collect_target_writes(&assign.target, writes),
        ast::Stmt::AugAssign(assign) => collect_target_writes(&assign.target, writes),
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                collect_target_writes(target, writes);
            }
        }
        ast::Stmt::For(stmt_for) => {
            collect_target_writes(&stmt_for.target, writes);
            collect_body_writes(&stmt_for.body, writes);
            collect_body_writes(&stmt_for.orelse, writes);
        }
        ast::Stmt::While(stmt_while) => {
            collect_body_writes(&stmt_while.body, writes);
            collect_body_writes(&stmt_while.orelse, writes);
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    collect_target_writes(optional_vars, writes);
                }
            }
            collect_body_writes(&stmt_with.body, writes);
        }
        ast::Stmt::Try(stmt_try) => {
            collect_body_writes(&stmt_try.body, writes);
            for handler in &stmt_try.handlers {
                let ast::ExceptHandler::ExceptHandler(handler) = handler;
                collect_body_writes(&handler.body, writes);
            }
            collect_body_writes(&stmt_try.orelse, writes);
            collect_body_writes(&stmt_try.finalbody, writes);
        }
        ast::Stmt::If(stmt_if) => {
            collect_body_writes(&stmt_if.body, writes);
            for clause in &stmt_if.elif_else_clauses {
                collect_body_writes(&clause.body, writes);
            }
        }
        ast::Stmt::Expr(expr) => {
            if expr_touches_name(&expr.value, INSTALLED_APPS) {
                writes.installed_apps = true;
            }
            if expr_touches_name(&expr.value, TEMPLATES) {
                writes.templates = true;
            }
        }
        ast::Stmt::Import(import) => {
            for alias in &import.names {
                let bound_name = alias.asname.as_ref().map_or_else(
                    || first_import_segment(alias.name.as_str()),
                    |asname| asname.as_str(),
                );
                collect_name_write(bound_name, writes);
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
                    collect_name_write(bound_name, writes);
                }
            }
        }
        ast::Stmt::FunctionDef(function) => collect_name_write(function.name.as_str(), writes),
        ast::Stmt::ClassDef(class) => collect_name_write(class.name.as_str(), writes),
        ast::Stmt::TypeAlias(type_alias) => collect_target_writes(&type_alias.name, writes),
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

fn collect_body_writes(body: &[ast::Stmt], writes: &mut TouchedNames) {
    for stmt in body {
        collect_stmt_writes(stmt, writes);
    }
}

fn collect_target_writes(target: &ast::Expr, writes: &mut TouchedNames) {
    if target_touches_name(target, INSTALLED_APPS) {
        writes.installed_apps = true;
    }
    if target_touches_name(target, TEMPLATES) {
        writes.templates = true;
    }
}

fn collect_name_write(name: &str, writes: &mut TouchedNames) {
    match name {
        INSTALLED_APPS => writes.installed_apps = true,
        TEMPLATES => writes.templates = true,
        _ => {}
    }
}

fn extract_string_pair_dict(value: &ast::Expr) -> (Vec<(String, String)>, Vec<Reason>) {
    let ast::Expr::Dict(dict) = value else {
        return (Vec::new(), vec![Reason::UnsupportedValue]);
    };

    let mut values = Vec::new();
    let mut reasons = Vec::new();
    for item in &dict.items {
        match (
            item.key.as_ref().and_then(ExprExt::string_literal),
            item.value.string_literal(),
        ) {
            (Some(key), Some(value)) => values.push((key.to_string(), value.to_string())),
            _ => reasons.push(Reason::UnsupportedValue),
        }
    }
    (values, reasons)
}

fn extract_string_list_literal(value: &ast::Expr) -> (Vec<String>, Vec<Reason>) {
    let ast::Expr::List(list) = value else {
        return (Vec::new(), vec![Reason::UnsupportedValue]);
    };
    extract_string_elements(&list.elts)
}

fn extract_string_elements(elements: &[ast::Expr]) -> (Vec<String>, Vec<Reason>) {
    let mut values = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        if let Some(value) = element.string_literal() {
            values.push(value.to_string());
        } else {
            reasons.push(Reason::NonLiteralElement);
        }
    }
    (values, reasons)
}

fn flatten_addition(expr: &ast::Expr) -> Vec<&ast::Expr> {
    let mut operands = Vec::new();
    push_addition_operands(expr, &mut operands);
    operands
}

fn push_addition_operands<'a>(expr: &'a ast::Expr, operands: &mut Vec<&'a ast::Expr>) {
    match expr {
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            push_addition_operands(&bin_op.left, operands);
            push_addition_operands(&bin_op.right, operands);
        }
        _ => operands.push(expr),
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

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use rustc_hash::FxHashMap;

    use super::*;

    #[derive(Default)]
    struct MapResolver {
        modules: FxHashMap<String, String>,
    }

    impl MapResolver {
        fn with_module(mut self, name: &str, source: &str) -> Self {
            self.modules.insert(name.to_string(), source.to_string());
            self
        }
    }

    impl SettingsSourceResolver for MapResolver {
        fn resolve_star_import(
            &mut self,
            import: &SettingsStarImport,
            _importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            let module = import.module.as_ref()?;
            let source = self.modules.get(module)?.clone();
            Some(SettingsSource {
                source,
                path: Utf8PathBuf::from(format!(
                    "/project/settings/{}.py",
                    module.replace('.', "/")
                )),
            })
        }
    }

    fn extract(source: &str) -> DjangoSettings {
        extract_settings(
            source,
            Utf8Path::new("/project/config/settings.py"),
            &mut MapResolver::default(),
        )
    }

    fn installed_apps(source: &str) -> Vec<String> {
        extract(source).installed_apps.values
    }

    #[test]
    fn literal_list_assignment_is_known() {
        let settings = extract("INSTALLED_APPS = ['django.contrib.admin', 'app']");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(
            settings.installed_apps.values,
            ["django.contrib.admin", "app"]
        );
    }

    #[test]
    fn literal_tuple_assignment_is_known() {
        let settings = extract("INSTALLED_APPS = ('django.contrib.auth', 'app')");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(
            settings.installed_apps.values,
            ["django.contrib.auth", "app"]
        );
    }

    #[test]
    fn annotated_assignment_is_known() {
        let settings = extract("INSTALLED_APPS: list[str] = ['app']");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(settings.installed_apps.values, ["app"]);
    }

    #[test]
    fn plus_equals_extends_existing_values() {
        let values = installed_apps("INSTALLED_APPS = ['base']\nINSTALLED_APPS += ['extra']");
        assert_eq!(values, ["base", "extra"]);
    }

    #[test]
    fn plus_chain_combines_literal_lists() {
        let values = installed_apps("INSTALLED_APPS = ['a'] + ['b'] + ('c',)");
        assert_eq!(values, ["a", "b", "c"]);
    }

    #[test]
    fn plus_chain_splices_watched_name() {
        let values =
            installed_apps("INSTALLED_APPS = ['a']\nINSTALLED_APPS = INSTALLED_APPS + ['b']");
        assert_eq!(values, ["a", "b"]);
    }

    #[test]
    fn mutation_methods_update_values() {
        let values = installed_apps(
            "INSTALLED_APPS = ['a', 'c']\n\
             INSTALLED_APPS.append('d')\n\
             INSTALLED_APPS.extend(['e'])\n\
             INSTALLED_APPS.insert(1, 'b')\n\
             INSTALLED_APPS.remove('c')",
        );
        assert_eq!(values, ["a", "b", "d", "e"]);
    }

    #[test]
    fn reassignment_replaces_prior_values() {
        let values = installed_apps(
            "INSTALLED_APPS = ['old']\nINSTALLED_APPS.append('ignored')\nINSTALLED_APPS = ['new']",
        );
        assert_eq!(values, ["new"]);
    }

    #[test]
    fn non_literal_element_is_partial_and_skipped() {
        let settings = extract("INSTALLED_APPS = ['a', env('EXTRA'), 'b']");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Partial);
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
        assert_eq!(settings.installed_apps.reasons, [Reason::NonLiteralElement]);
    }

    #[test]
    fn unsupported_assignment_is_unknown() {
        let settings = extract("INSTALLED_APPS = get_apps()");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Unknown);
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn decidable_if_true_picks_body() {
        let values = installed_apps(
            "if True:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']",
        );
        assert_eq!(values, ["body"]);
    }

    #[test]
    fn decidable_if_false_picks_else() {
        let values = installed_apps(
            "if False:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']",
        );
        assert_eq!(values, ["else"]);
    }

    #[test]
    fn bool_name_condition_is_decidable() {
        let values = installed_apps(
            "DEBUG = True\nif DEBUG:\n    INSTALLED_APPS = ['debug']\nelse:\n    INSTALLED_APPS = ['prod']",
        );
        assert_eq!(values, ["debug"]);
    }

    #[test]
    fn ambiguous_condition_walks_all_arms_and_marks_partial() {
        let settings = extract(
            "INSTALLED_APPS = ['base']\nif os.environ.get('X'):\n    INSTALLED_APPS.append('debug')\nelse:\n    INSTALLED_APPS.append('prod')",
        );
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Partial);
        assert_eq!(settings.installed_apps.values, ["base", "debug", "prod"]);
    }

    #[test]
    fn star_import_layering_can_be_extended() {
        let mut resolver = MapResolver::default().with_module(
            "base",
            "INSTALLED_APPS = ['base']\nBASE_DIR = Path(__file__).resolve().parent",
        );
        let settings = extract_settings(
            "from base import *\nINSTALLED_APPS += ['local']",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(settings.installed_apps.values, ["base", "local"]);
    }

    #[test]
    fn star_import_without_setting_does_not_overwrite_existing_fact() {
        let mut resolver = MapResolver::default()
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent");
        let settings = extract_settings(
            "INSTALLED_APPS = ['local']\nfrom paths import *",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn star_imported_local_path_can_feed_later_template_dirs() {
        let mut resolver = MapResolver::default()
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent.parent");
        let settings = extract_settings(
            "from paths import *\nTEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(settings.templates.knowledge, StaticKnowledge::Known);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn cyclic_star_import_does_not_recurse_forever() {
        let mut resolver = MapResolver::default()
            .with_module("cycle", "from cycle import *\nINSTALLED_APPS = ['local']");
        let settings = extract_settings(
            "from cycle import *",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Known);
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn unresolvable_star_import_is_partial() {
        let settings = extract("from missing import *");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Partial);
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Partial);
    }

    #[test]
    fn templates_literal_dict_extracts_backend_options_and_paths() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{\n\
             'BACKEND': 'django.template.backends.django.DjangoTemplates',\n\
             'DIRS': [BASE_DIR / 'templates'],\n\
             'APP_DIRS': True,\n\
             'OPTIONS': {\n\
             'libraries': {'custom': 'app.templatetags.custom'},\n\
             'builtins': ['django.templatetags.static'],\n\
             },\n\
             }]",
        );
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Known);
        let backend = &settings.templates.backends[0];
        assert_eq!(
            backend.backend.as_deref(),
            Some("django.template.backends.django.DjangoTemplates")
        );
        assert_eq!(backend.app_dirs, Some(true));
        assert_eq!(
            backend.libraries,
            [("custom".to_string(), "app.templatetags.custom".to_string())]
        );
        assert_eq!(backend.builtins, ["django.templatetags.static"]);
        assert_eq!(
            backend.dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn templates_dirs_append_mutates_existing_backend() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': []}]\n\
             TEMPLATES[0]['DIRS'].append(BASE_DIR / 'templates')",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn templates_dirs_plus_equals_extends_existing_backend() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': []}]\n\
             TEMPLATES[0]['DIRS'] += [BASE_DIR / 'templates']",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn non_literal_backend_is_partial() {
        let settings = extract("TEMPLATES = [{'BACKEND': backend_name}]");
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Partial);
        assert_eq!(
            settings.templates.backends[0].knowledge,
            StaticKnowledge::Partial
        );
    }

    #[test]
    fn template_backend_unpack_is_partial() {
        let settings = extract("TEMPLATES = [{'DIRS': [], **extra}]");
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Partial);
        assert_eq!(
            settings.templates.backends[0].knowledge,
            StaticKnowledge::Partial
        );
    }

    #[test]
    fn os_path_join_resolves_relative_to_base_dir() {
        let settings = extract(
            "from pathlib import Path\n\
             import os\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': [os.path.join(BASE_DIR, 'templates')]}]",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn unknown_path_call_becomes_unknown_path_value() {
        let settings = extract("TEMPLATES = [{'DIRS': [dynamic_path()]}]");
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Partial);
        assert!(matches!(
            settings.templates.backends[0].dirs[0],
            TemplateDirPath::Unknown
        ));
    }

    #[test]
    fn ambiguous_assignment_preserves_pre_branch_possibility() {
        let settings =
            extract("INSTALLED_APPS = ['base']\nif FLAG:\n    INSTALLED_APPS = ['debug']");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Partial);
        assert_eq!(settings.installed_apps.values, ["debug", "base"]);
    }

    #[test]
    fn syntax_error_source_returns_partial_settings() {
        let settings = extract("INSTALLED_APPS = [");
        assert_eq!(settings.installed_apps.knowledge, StaticKnowledge::Partial);
        assert_eq!(settings.templates.knowledge, StaticKnowledge::Partial);
    }
}
