//! Django settings fact extraction.
//!
//! This module extracts a narrow Tier 2 subset from settings files: literal
//! `INSTALLED_APPS`, literal `TEMPLATES`, simple template directory path
//! expressions, relative star imports, and simple list mutations. It reports
//! unsupported settings shapes as partial or unknown facts instead of importing
//! the settings module.

#![allow(
    dead_code,
    reason = "Milestone A5 expands settings facts before project facts are assembled."
)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::Utf8PathClean;
use ruff_python_ast::Expr;
use ruff_python_ast::Number;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;

use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::Reason;
use crate::project::facts::SettingsFacts;
use crate::project::facts::TemplateBackendFact;
use crate::project::facts::TemplateDirFact;
use crate::project::facts::TemplateDirSource;
use crate::project::facts::TemplateLibraryFact;
use crate::project::facts::TemplateLibrarySource;
use crate::project::module_resolver::resolve_module;
use crate::project::module_resolver::resolve_relative_import_module;
use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;

const INSTALLED_APPS: &str = "INSTALLED_APPS";
const TEMPLATES: &str = "TEMPLATES";

struct SettingsImportScope<'a> {
    project_root: &'a Utf8Path,
    search_paths: &'a [ModuleSearchPathEntry],
}

struct SettingsExtractionState {
    file: Utf8PathBuf,
    files_read: Vec<Utf8PathBuf>,
    path_values: BTreeMap<String, Utf8PathBuf>,
    installed_apps: Option<Fact<Vec<String>>>,
    template_backends: Option<Fact<Vec<TemplateBackendFact>>>,
    installed_apps_import_reasons: Vec<Reason>,
    template_backends_import_reasons: Vec<Reason>,
    load_failed: bool,
}

impl SettingsExtractionState {
    fn new(file: &Utf8Path) -> Self {
        Self {
            file: file.to_path_buf(),
            files_read: vec![file.to_path_buf()],
            path_values: BTreeMap::new(),
            installed_apps: None,
            template_backends: None,
            installed_apps_import_reasons: Vec::new(),
            template_backends_import_reasons: Vec::new(),
            load_failed: false,
        }
    }

    fn failed(file: &Utf8Path, message: impl Into<String>) -> Self {
        let facts = unknown_settings_facts(file, message);
        Self {
            file: facts.file,
            files_read: facts.files_read,
            path_values: BTreeMap::new(),
            installed_apps: Some(facts.installed_apps),
            template_backends: Some(facts.template_backends),
            installed_apps_import_reasons: Vec::new(),
            template_backends_import_reasons: Vec::new(),
            load_failed: true,
        }
    }

    fn apply_star_import(&mut self, imported: Self) {
        self.files_read.extend(imported.files_read);
        if imported.load_failed {
            if let Some(installed_apps) = imported.installed_apps {
                self.installed_apps_import_reasons
                    .extend(installed_apps.reasons().iter().cloned());
            }
            if let Some(template_backends) = imported.template_backends {
                self.template_backends_import_reasons
                    .extend(template_backends.reasons().iter().cloned());
            }
            return;
        }

        self.installed_apps_import_reasons
            .extend(imported.installed_apps_import_reasons);
        self.template_backends_import_reasons
            .extend(imported.template_backends_import_reasons);
        self.path_values.extend(imported.path_values);
        if let Some(installed_apps) = imported.installed_apps {
            self.installed_apps = Some(installed_apps);
        }
        if let Some(template_backends) = imported.template_backends {
            self.template_backends = Some(template_backends);
        }
    }

    fn add_import_reasons(&mut self, reasons: Vec<Reason>) {
        self.installed_apps_import_reasons.extend(reasons.clone());
        self.template_backends_import_reasons.extend(reasons);
    }

    fn add_files_read(&mut self, files: impl IntoIterator<Item = Utf8PathBuf>) {
        self.files_read.extend(files);
    }

    fn into_facts(self) -> SettingsFacts {
        let file = self.file;
        let mut files_read = self.files_read;
        files_read.sort();
        files_read.dedup();
        let installed_apps = finalize_setting_fact(
            self.installed_apps,
            self.installed_apps_import_reasons,
            Field::SettingsInstalledApps,
            &file,
            "INSTALLED_APPS is not assigned in this settings file",
        );
        let template_backends = finalize_setting_fact(
            self.template_backends,
            self.template_backends_import_reasons,
            Field::SettingsTemplates,
            &file,
            "TEMPLATES is not assigned in this settings file",
        );

        SettingsFacts {
            file,
            files_read,
            installed_apps,
            template_backends,
        }
    }
}

#[must_use]
pub(crate) fn extract_settings_facts(file: &Utf8Path) -> SettingsFacts {
    let mut active_files = BTreeSet::new();
    extract_settings_state(file, None, None, &mut active_files).into_facts()
}

#[must_use]
pub(crate) fn extract_settings_facts_for_module(
    file: &Utf8Path,
    current_module: &PyModuleName,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> SettingsFacts {
    let imports = SettingsImportScope {
        project_root,
        search_paths,
    };
    let mut active_files = BTreeSet::new();
    extract_settings_state(
        file,
        Some(current_module),
        Some(&imports),
        &mut active_files,
    )
    .into_facts()
}

fn extract_settings_state(
    file: &Utf8Path,
    current_module: Option<&PyModuleName>,
    imports: Option<&SettingsImportScope>,
    active_files: &mut BTreeSet<Utf8PathBuf>,
) -> SettingsExtractionState {
    let file = file.to_path_buf();
    if !active_files.insert(file.clone()) {
        return SettingsExtractionState::failed(
            &file,
            "cycle detected while following settings star imports",
        );
    }

    let state = extract_settings_state_inner(&file, current_module, imports, active_files);
    active_files.remove(&file);
    state
}

fn extract_settings_state_inner(
    file: &Utf8Path,
    current_module: Option<&PyModuleName>,
    imports: Option<&SettingsImportScope>,
    active_files: &mut BTreeSet<Utf8PathBuf>,
) -> SettingsExtractionState {
    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            return SettingsExtractionState::failed(
                file,
                format!("failed to read settings file: {error}"),
            );
        }
    };

    let module = match ruff_python_parser::parse_module(&source) {
        Ok(parsed) => parsed.into_syntax(),
        Err(error) => {
            return SettingsExtractionState::failed(
                file,
                format!("failed to parse settings file: {error}"),
            );
        }
    };

    let mut state = SettingsExtractionState::new(file);

    for stmt in &module.body {
        if let Some(import_from) = star_import(stmt) {
            follow_star_import(
                &mut state,
                import_from,
                current_module,
                imports,
                active_files,
            );
            continue;
        }

        record_path_assignment(stmt, &mut state.path_values, file);

        if let Some(value) = assigned_value(stmt, INSTALLED_APPS) {
            state.installed_apps = Some(extract_string_list(
                value,
                Field::SettingsInstalledApps,
                file,
                "INSTALLED_APPS",
            ));
            continue;
        }

        if let Some(value) = assigned_value(stmt, TEMPLATES) {
            state.template_backends =
                Some(extract_template_backends(value, &state.path_values, file));
            continue;
        }

        if let Some(mutation) = list_mutation(stmt, INSTALLED_APPS) {
            state.installed_apps = Some(apply_installed_apps_mutation(
                state.installed_apps.take(),
                mutation,
                file,
            ));
            continue;
        }

        if let Some(mutation) = list_mutation(stmt, TEMPLATES) {
            state.template_backends = Some(apply_template_backends_mutation(
                state.template_backends.take(),
                mutation,
                &state.path_values,
                file,
            ));
        }
    }

    state
}

fn assigned_value<'a>(stmt: &'a Stmt, target_name: &str) -> Option<&'a Expr> {
    match stmt {
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .any(|target| is_name(target, target_name))
            .then_some(assign.value.as_ref()),
        Stmt::AnnAssign(assign) if is_name(assign.target.as_ref(), target_name) => {
            assign.value.as_deref()
        }
        _ => None,
    }
}

fn record_path_assignment(
    stmt: &Stmt,
    path_values: &mut BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) {
    let Some((name, value)) = simple_assignment(stmt) else {
        return;
    };
    let Some(path) = evaluate_path(value, path_values, settings_file) else {
        return;
    };
    path_values.insert(name.to_string(), path);
}

fn simple_assignment(stmt: &Stmt) -> Option<(&str, &Expr)> {
    match stmt {
        Stmt::Assign(assign) => {
            let target = assign.targets.first()?;
            let Expr::Name(name) = target else {
                return None;
            };
            Some((name.id.as_str(), assign.value.as_ref()))
        }
        Stmt::AnnAssign(assign) => {
            let Expr::Name(name) = assign.target.as_ref() else {
                return None;
            };
            Some((name.id.as_str(), assign.value.as_deref()?))
        }
        _ => None,
    }
}

fn star_import(stmt: &Stmt) -> Option<&ruff_python_ast::StmtImportFrom> {
    let Stmt::ImportFrom(import_from) = stmt else {
        return None;
    };
    let [alias] = import_from.names.as_slice() else {
        return None;
    };
    (alias.name.as_str() == "*" && alias.asname.is_none()).then_some(import_from)
}

fn follow_star_import(
    state: &mut SettingsExtractionState,
    import_from: &ruff_python_ast::StmtImportFrom,
    current_module: Option<&PyModuleName>,
    imports: Option<&SettingsImportScope>,
    active_files: &mut BTreeSet<Utf8PathBuf>,
) {
    let Some(current_module) = current_module else {
        return;
    };
    let Some(imports) = imports else {
        return;
    };

    let target_module_fact = star_import_module(import_from, current_module);
    let Some(target_module) = target_module_fact.value().cloned() else {
        state.add_import_reasons(target_module_fact.reasons().to_vec());
        return;
    };
    state.add_import_reasons(target_module_fact.reasons().to_vec());

    let resolution = resolve_module(
        target_module.clone(),
        imports.search_paths,
        imports.project_root,
    );
    let target_file = match resolution.resolved {
        Fact::Known { value } => value.file,
        Fact::Partial { value, reasons } => {
            state.add_import_reasons(reasons);
            value.file
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => {
            state.add_import_reasons(reasons);
            state.add_files_read(unresolved_module_candidate_files(
                &target_module,
                imports.search_paths,
            ));
            return;
        }
    };

    let imported = extract_settings_state(
        &target_file,
        Some(&target_module),
        Some(imports),
        active_files,
    );
    state.apply_star_import(imported);
}

fn unresolved_module_candidate_files(
    module: &PyModuleName,
    search_paths: &[ModuleSearchPathEntry],
) -> Vec<Utf8PathBuf> {
    let relative_module_path = module.as_str().replace('.', "/");
    search_paths
        .iter()
        .flat_map(|search_path| {
            let module_file = search_path.path.join(format!("{relative_module_path}.py"));
            let package_file = search_path
                .path
                .join(&relative_module_path)
                .join("__init__.py");
            [module_file, package_file]
        })
        .collect()
}

fn star_import_module(
    import_from: &ruff_python_ast::StmtImportFrom,
    current_module: &PyModuleName,
) -> Fact<PyModuleName> {
    let module = import_from
        .module
        .as_ref()
        .map(ruff_python_ast::Identifier::as_str);
    if import_from.level > 0 {
        return resolve_relative_import_module(current_module, import_from.level as usize, module);
    }

    let Some(module) = module else {
        return Fact::unknown(vec![Reason::module(
            Field::ResolverModule,
            current_module.clone(),
            "absolute star import must name a module",
        )]);
    };

    match PyModuleName::parse(module) {
        Ok(module) => Fact::known(module),
        Err(error) => Fact::unknown(vec![Reason::module(
            Field::ResolverModule,
            current_module.clone(),
            format!("star import resolves to an invalid module path: {error}"),
        )]),
    }
}

#[derive(Clone, Copy)]
enum ListMutation<'a> {
    Append(&'a Expr),
    Extend(&'a Expr),
    Insert { index: usize, value: &'a Expr },
    Unsupported,
}

fn list_mutation<'a>(stmt: &'a Stmt, target_name: &str) -> Option<ListMutation<'a>> {
    match stmt {
        Stmt::AugAssign(assign) if is_name(assign.target.as_ref(), target_name) => {
            if assign.op == Operator::Add {
                Some(ListMutation::Extend(assign.value.as_ref()))
            } else {
                Some(ListMutation::Unsupported)
            }
        }
        Stmt::Expr(expr_stmt) => call_list_mutation(expr_stmt.value.as_ref(), target_name),
        _ => None,
    }
}

fn call_list_mutation<'a>(expr: &'a Expr, target_name: &str) -> Option<ListMutation<'a>> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Attribute(attribute) = call.func.as_ref() else {
        return None;
    };
    if !is_name(attribute.value.as_ref(), target_name) {
        return None;
    }
    if !call.arguments.keywords.is_empty() {
        return Some(ListMutation::Unsupported);
    }

    match attribute.attr.as_str() {
        "append" => {
            let [value] = call.arguments.args.as_ref() else {
                return Some(ListMutation::Unsupported);
            };
            Some(ListMutation::Append(value))
        }
        "extend" => {
            let [value] = call.arguments.args.as_ref() else {
                return Some(ListMutation::Unsupported);
            };
            Some(ListMutation::Extend(value))
        }
        "insert" => {
            let [index, value] = call.arguments.args.as_ref() else {
                return Some(ListMutation::Unsupported);
            };
            let Some(index) = integer_literal(index) else {
                return Some(ListMutation::Unsupported);
            };
            Some(ListMutation::Insert { index, value })
        }
        "update" => Some(ListMutation::Unsupported),
        _ => None,
    }
}

fn is_name(expr: &Expr, expected: &str) -> bool {
    matches!(expr, Expr::Name(name) if name.id.as_str() == expected)
}

fn extract_string_list(
    expr: &Expr,
    field: Field,
    file: &Utf8Path,
    setting_name: &str,
) -> Fact<Vec<String>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            field,
            file,
            format!("{setting_name} must be a literal list or tuple of strings"),
        )]);
    };

    let mut values = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        if let Some(value) = string_literal(element) {
            values.push(value);
        } else {
            reasons.push(reason(
                field,
                file,
                format!("{setting_name} contains a non-string entry"),
            ));
        }
    }

    known_or_partial(values, reasons)
}

fn apply_installed_apps_mutation(
    current: Option<Fact<Vec<String>>>,
    mutation: ListMutation,
    file: &Utf8Path,
) -> Fact<Vec<String>> {
    let Some(current) = current else {
        return mutation_before_assignment(
            Field::SettingsInstalledApps,
            file,
            "INSTALLED_APPS mutation appears before assignment or import",
        );
    };

    match mutation {
        ListMutation::Append(value) => {
            let Some(app) = string_literal(value) else {
                return current.with_reason(reason(
                    Field::SettingsInstalledApps,
                    file,
                    "INSTALLED_APPS append value must be a string literal",
                ));
            };
            append_vec_fact(
                current,
                app,
                Vec::new(),
                reason(
                    Field::SettingsInstalledApps,
                    file,
                    "cannot apply INSTALLED_APPS append because the current value is unknown or ambiguous",
                ),
            )
        }
        ListMutation::Extend(value) => extend_vec_fact(
            current,
            extract_string_list(
                value,
                Field::SettingsInstalledApps,
                file,
                "INSTALLED_APPS mutation",
            ),
            reason(
                Field::SettingsInstalledApps,
                file,
                "cannot apply INSTALLED_APPS extend because the current value is unknown or ambiguous",
            ),
        ),
        ListMutation::Insert { index, value } => {
            let Some(app) = string_literal(value) else {
                return current.with_reason(reason(
                    Field::SettingsInstalledApps,
                    file,
                    "INSTALLED_APPS insert value must be a string literal",
                ));
            };
            insert_vec_fact(
                current,
                index,
                app,
                Vec::new(),
                reason(
                    Field::SettingsInstalledApps,
                    file,
                    "cannot apply INSTALLED_APPS insert because the current value is unknown or ambiguous",
                ),
            )
        }
        ListMutation::Unsupported => current.with_reason(reason(
            Field::SettingsInstalledApps,
            file,
            "unsupported dynamic mutation of INSTALLED_APPS",
        )),
    }
}

fn extract_template_backends(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateBackendFact>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplates,
            file,
            "TEMPLATES must be assigned a literal list or tuple of dictionaries",
        )]);
    };

    let mut backends = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        let Expr::Dict(dict) = element else {
            reasons.push(reason(
                Field::SettingsTemplates,
                file,
                "TEMPLATES contains a non-dictionary backend entry",
            ));
            continue;
        };
        backends.push(extract_template_backend(
            dict,
            path_values,
            file,
            &mut reasons,
        ));
    }

    known_or_partial(backends, reasons)
}

fn apply_template_backends_mutation(
    current: Option<Fact<Vec<TemplateBackendFact>>>,
    mutation: ListMutation,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateBackendFact>> {
    let Some(current) = current else {
        return mutation_before_assignment(
            Field::SettingsTemplates,
            file,
            "TEMPLATES mutation appears before assignment or import",
        );
    };

    match mutation {
        ListMutation::Append(value) => extend_vec_fact(
            current,
            extract_template_backend_list_item(value, path_values, file),
            reason(
                Field::SettingsTemplates,
                file,
                "cannot apply TEMPLATES append because the current value is unknown or ambiguous",
            ),
        ),
        ListMutation::Extend(value) => extend_vec_fact(
            current,
            extract_template_backends(value, path_values, file),
            reason(
                Field::SettingsTemplates,
                file,
                "cannot apply TEMPLATES extend because the current value is unknown or ambiguous",
            ),
        ),
        ListMutation::Insert { index, value } => insert_vec_fact_from_fact(
            current,
            index,
            extract_template_backend_item(value, path_values, file),
            reason(
                Field::SettingsTemplates,
                file,
                "cannot apply TEMPLATES insert because the current value is unknown or ambiguous",
            ),
        ),
        ListMutation::Unsupported => current.with_reason(reason(
            Field::SettingsTemplates,
            file,
            "unsupported dynamic mutation of TEMPLATES",
        )),
    }
}

fn extract_template_backend_list_item(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateBackendFact>> {
    extract_template_backend_item(expr, path_values, file).map(|backend| vec![backend])
}

fn extract_template_backend_item(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<TemplateBackendFact> {
    let Expr::Dict(dict) = expr else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplates,
            file,
            "TEMPLATES mutation value must be a dictionary literal",
        )]);
    };

    let mut reasons = Vec::new();
    let backend = extract_template_backend(dict, path_values, file, &mut reasons);
    if reasons.is_empty() {
        Fact::known(backend)
    } else {
        Fact::partial(backend, reasons)
    }
}

fn extract_template_backend(
    dict: &ruff_python_ast::ExprDict,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
    reasons: &mut Vec<Reason>,
) -> TemplateBackendFact {
    let backend = match dict_value(dict, "BACKEND") {
        Some(value) => {
            if let Some(backend) = string_literal(value) {
                Some(backend)
            } else {
                reasons.push(reason(
                    Field::SettingsTemplates,
                    file,
                    "TEMPLATES BACKEND must be a string literal",
                ));
                None
            }
        }
        None => None,
    };

    let dirs = match dict_value(dict, "DIRS") {
        Some(value) => extract_template_dirs(value, path_values, file),
        None => Fact::known(Vec::new()),
    };

    let app_dirs = match dict_value(dict, "APP_DIRS") {
        Some(Expr::BooleanLiteral(boolean)) => Fact::known(boolean.value),
        Some(_) => Fact::unknown(vec![reason(
            Field::SettingsTemplates,
            file,
            "TEMPLATES APP_DIRS must be a boolean literal",
        )]),
        None => Fact::known(false),
    };

    let (option_libraries, option_builtins) = match dict_value(dict, "OPTIONS") {
        Some(Expr::Dict(options)) => (
            extract_option_libraries(options, file),
            extract_option_builtins(options, file),
        ),
        Some(_) => {
            let option_reason = reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS must be a dictionary literal",
            );
            (
                Fact::unknown(vec![option_reason.clone()]),
                Fact::unknown(vec![option_reason]),
            )
        }
        None => (Fact::known(Vec::new()), Fact::known(Vec::new())),
    };

    TemplateBackendFact {
        backend,
        dirs,
        app_dirs,
        option_libraries,
        option_builtins,
    }
}

fn extract_template_dirs(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateDirFact>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateDirs,
            file,
            "TEMPLATES DIRS must be a literal list or tuple",
        )]);
    };

    let mut dirs = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        if let Some(path) = evaluate_path(element, path_values, file) {
            dirs.push(TemplateDirFact {
                path,
                source: TemplateDirSource::SettingsDir,
            });
        } else {
            reasons.push(reason(
                Field::SettingsTemplateDirs,
                file,
                "TEMPLATES DIRS contains an unsupported path expression",
            ));
        }
    }

    known_or_partial(dirs, reasons)
}

fn extract_option_libraries(
    options: &ruff_python_ast::ExprDict,
    file: &Utf8Path,
) -> Fact<Vec<TemplateLibraryFact>> {
    let Some(value) = dict_value(options, "libraries") else {
        return Fact::known(Vec::new());
    };
    let Expr::Dict(libraries) = value else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateOptions,
            file,
            "TEMPLATES OPTIONS.libraries must be a dictionary literal",
        )]);
    };

    let mut facts = Vec::new();
    let mut reasons = Vec::new();
    for item in &libraries.items {
        let Some(key) = item.key.as_ref() else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains dictionary unpacking",
            ));
            continue;
        };
        let Some(load_name) = string_literal(key) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains a non-string load name",
            ));
            continue;
        };
        let Some(module) = string_literal(&item.value) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains a non-string module path",
            ));
            continue;
        };

        match (LibraryName::parse(&load_name), PyModuleName::parse(&module)) {
            (Ok(load_name), Ok(module)) => facts.push(TemplateLibraryFact {
                load_name,
                module,
                source: TemplateLibrarySource::SettingsLibraries,
            }),
            (Err(error), _) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.libraries load name: {error}"),
            )),
            (_, Err(error)) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.libraries module path: {error}"),
            )),
        }
    }

    known_or_partial(facts, reasons)
}

fn extract_option_builtins(
    options: &ruff_python_ast::ExprDict,
    file: &Utf8Path,
) -> Fact<Vec<PyModuleName>> {
    let Some(value) = dict_value(options, "builtins") else {
        return Fact::known(Vec::new());
    };
    let Some(elements) = collection_elements(value) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateOptions,
            file,
            "TEMPLATES OPTIONS.builtins must be a literal list or tuple",
        )]);
    };

    let mut builtins = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        let Some(module) = string_literal(element) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.builtins contains a non-string module path",
            ));
            continue;
        };
        match PyModuleName::parse(&module) {
            Ok(module) => builtins.push(module),
            Err(error) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.builtins module path: {error}"),
            )),
        }
    }

    known_or_partial(builtins, reasons)
}

fn string_literal(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(string) = expr {
        return Some(string.value.to_str().to_string());
    }
    None
}

fn collection_elements(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::List(list) => Some(list.elts.as_slice()),
        Expr::Tuple(tuple) => Some(tuple.elts.as_slice()),
        _ => None,
    }
}

fn dict_value<'a>(dict: &'a ruff_python_ast::ExprDict, key: &str) -> Option<&'a Expr> {
    dict.items.iter().find_map(|item| {
        let item_key = string_literal(item.key.as_ref()?)?;
        (item_key == key).then_some(&item.value)
    })
}

fn evaluate_path(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    match expr {
        Expr::StringLiteral(string) => Some(Utf8PathBuf::from(string.value.to_str())),
        Expr::Name(name) if name.id.as_str() == "__file__" => Some(settings_file.to_path_buf()),
        Expr::Name(name) => path_values.get(name.id.as_str()).cloned(),
        Expr::Attribute(_) if path_constant(expr).is_some() => {
            path_constant(expr).map(Utf8PathBuf::from)
        }
        Expr::BinOp(binop) if binop.op == Operator::Add => {
            let value = evaluate_path_string(expr, path_values, settings_file)?;
            Some(Utf8PathBuf::from(value))
        }
        Expr::BinOp(binop) if binop.op == Operator::Div => {
            let left = evaluate_path(binop.left.as_ref(), path_values, settings_file)?;
            let right = evaluate_path(binop.right.as_ref(), path_values, settings_file)?;
            Some(left.join(right))
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
            let value = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
            value.parent().map(Utf8Path::to_path_buf)
        }
        Expr::Subscript(subscript) => {
            evaluate_path_parents_subscript(subscript, path_values, settings_file)
        }
        Expr::Call(call) => evaluate_path_call(call, path_values, settings_file),
        _ => None,
    }
}

fn evaluate_path_string(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<String> {
    match expr {
        Expr::StringLiteral(string) => Some(string.value.to_str().to_string()),
        Expr::Name(name) if name.id.as_str() == "__file__" => Some(settings_file.to_string()),
        Expr::Name(name) => path_values
            .get(name.id.as_str())
            .map(|path| path.as_str().to_string()),
        Expr::Attribute(_) if path_constant(expr).is_some() => {
            path_constant(expr).map(str::to_string)
        }
        Expr::BinOp(binop) if binop.op == Operator::Add => {
            let left = evaluate_path_string(binop.left.as_ref(), path_values, settings_file)?;
            let right = evaluate_path_string(binop.right.as_ref(), path_values, settings_file)?;
            Some(format!("{left}{right}"))
        }
        _ => evaluate_path(expr, path_values, settings_file).map(|path| path.as_str().to_string()),
    }
}

fn evaluate_path_call(
    call: &ruff_python_ast::ExprCall,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    match call.func.as_ref() {
        Expr::Name(name) if name.id.as_str() == "Path" => {
            let first = call.arguments.args.first()?;
            evaluate_path(first, path_values, settings_file)
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "resolve" => {
            evaluate_path(attribute.value.as_ref(), path_values, settings_file)
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "joinpath" => {
            let mut path = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
            for arg in &call.arguments.args {
                let part = evaluate_path(arg, path_values, settings_file)?;
                path = path.join(part);
            }
            Some(path)
        }
        _ if dotted_name(call.func.as_ref()).as_deref() == Some("os.path.join") => {
            let mut args = call.arguments.args.iter();
            let mut path = evaluate_path(args.next()?, path_values, settings_file)?;
            for arg in args {
                let part = evaluate_path(arg, path_values, settings_file)?;
                path = path.join(part);
            }
            Some(path)
        }
        _ if matches!(
            dotted_name(call.func.as_ref()).as_deref(),
            Some("os.path.abspath" | "os.path.realpath")
        ) =>
        {
            let path = evaluate_path(call.arguments.args.first()?, path_values, settings_file)?;
            path.is_absolute().then_some(path)
        }
        _ if dotted_name(call.func.as_ref()).as_deref() == Some("os.path.normpath") => {
            Some(evaluate_path(call.arguments.args.first()?, path_values, settings_file)?.clean())
        }
        _ if dotted_name(call.func.as_ref()).as_deref() == Some("os.path.dirname") => {
            let path = evaluate_path(call.arguments.args.first()?, path_values, settings_file)?;
            path.parent().map(Utf8Path::to_path_buf)
        }
        _ => None,
    }
}

fn path_constant(expr: &Expr) -> Option<&'static str> {
    match dotted_name(expr).as_deref()? {
        "os.pardir" | "os.path.pardir" => Some(".."),
        "os.curdir" | "os.path.curdir" => Some("."),
        _ => None,
    }
}

fn evaluate_path_parents_subscript(
    subscript: &ruff_python_ast::ExprSubscript,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let Expr::Attribute(attribute) = subscript.value.as_ref() else {
        return None;
    };
    if attribute.attr.as_str() != "parents" {
        return None;
    }

    let mut path = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
    let parent_index = integer_literal(subscript.slice.as_ref())?;
    for _ in 0..=parent_index {
        path = path.parent()?.to_path_buf();
    }
    Some(path)
}

fn integer_literal(expr: &Expr) -> Option<usize> {
    let Expr::NumberLiteral(number) = expr else {
        return None;
    };
    let Number::Int(value) = &number.value else {
        return None;
    };
    value.as_usize()
}

fn dotted_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.to_string()),
        Expr::Attribute(attribute) => Some(format!(
            "{}.{}",
            dotted_name(attribute.value.as_ref())?,
            attribute.attr.as_str()
        )),
        _ => None,
    }
}

fn known_or_partial<T>(values: Vec<T>, reasons: Vec<Reason>) -> Fact<Vec<T>> {
    if reasons.is_empty() {
        Fact::known(values)
    } else {
        Fact::partial(values, reasons)
    }
}

fn finalize_setting_fact<T>(
    fact: Option<Fact<T>>,
    mut reasons: Vec<Reason>,
    field: Field,
    file: &Utf8Path,
    missing_message: &'static str,
) -> Fact<T> {
    if let Some(fact) = fact {
        add_reasons(fact, reasons)
    } else {
        reasons.push(reason(field, file, missing_message));
        Fact::unknown(reasons)
    }
}

fn add_reasons<T>(mut fact: Fact<T>, reasons: Vec<Reason>) -> Fact<T> {
    for reason in reasons {
        fact = fact.with_reason(reason);
    }
    fact
}

fn mutation_before_assignment<T>(
    field: Field,
    file: &Utf8Path,
    message: &'static str,
) -> Fact<Vec<T>> {
    Fact::unknown(vec![reason(field, file, message)])
}

fn append_vec_fact<T>(
    fact: Fact<Vec<T>>,
    item: T,
    reasons: Vec<Reason>,
    unavailable_reason: Reason,
) -> Fact<Vec<T>> {
    extend_vec_items(fact, vec![item], reasons, unavailable_reason)
}

fn extend_vec_fact<T>(
    fact: Fact<Vec<T>>,
    items: Fact<Vec<T>>,
    unavailable_reason: Reason,
) -> Fact<Vec<T>> {
    match items {
        Fact::Known { value } => extend_vec_items(fact, value, Vec::new(), unavailable_reason),
        Fact::Partial { value, reasons } => {
            extend_vec_items(fact, value, reasons, unavailable_reason)
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => add_reasons(fact, reasons),
    }
}

fn insert_vec_fact_from_fact<T>(
    fact: Fact<Vec<T>>,
    index: usize,
    item: Fact<T>,
    unavailable_reason: Reason,
) -> Fact<Vec<T>> {
    match item {
        Fact::Known { value } => {
            insert_vec_fact(fact, index, value, Vec::new(), unavailable_reason)
        }
        Fact::Partial { value, reasons } => {
            insert_vec_fact(fact, index, value, reasons, unavailable_reason)
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => add_reasons(fact, reasons),
    }
}

fn insert_vec_fact<T>(
    fact: Fact<Vec<T>>,
    index: usize,
    item: T,
    reasons: Vec<Reason>,
    unavailable_reason: Reason,
) -> Fact<Vec<T>> {
    match fact {
        Fact::Known { mut value } => {
            let index = index.min(value.len());
            value.insert(index, item);
            known_or_partial(value, reasons)
        }
        Fact::Partial {
            mut value,
            reasons: mut existing_reasons,
        } => {
            let index = index.min(value.len());
            value.insert(index, item);
            existing_reasons.extend(reasons);
            Fact::partial(value, existing_reasons)
        }
        Fact::Unknown { mut reasons } => {
            reasons.push(unavailable_reason);
            Fact::unknown(reasons)
        }
        Fact::Ambiguous {
            candidates,
            mut reasons,
        } => {
            reasons.push(unavailable_reason);
            Fact::ambiguous(candidates, reasons)
        }
    }
}

fn extend_vec_items<T>(
    fact: Fact<Vec<T>>,
    items: Vec<T>,
    item_reasons: Vec<Reason>,
    unavailable_reason: Reason,
) -> Fact<Vec<T>> {
    match fact {
        Fact::Known { mut value } => {
            value.extend(items);
            known_or_partial(value, item_reasons)
        }
        Fact::Partial {
            mut value,
            mut reasons,
        } => {
            value.extend(items);
            reasons.extend(item_reasons);
            Fact::partial(value, reasons)
        }
        Fact::Unknown { mut reasons } => {
            reasons.push(unavailable_reason);
            Fact::unknown(reasons)
        }
        Fact::Ambiguous {
            candidates,
            mut reasons,
        } => {
            reasons.push(unavailable_reason);
            Fact::ambiguous(candidates, reasons)
        }
    }
}

fn unknown_settings_facts(file: &Utf8Path, message: impl Into<String>) -> SettingsFacts {
    let message = message.into();
    SettingsFacts {
        file: file.to_path_buf(),
        files_read: vec![file.to_path_buf()],
        installed_apps: Fact::unknown(vec![reason(
            Field::SettingsInstalledApps,
            file,
            message.clone(),
        )]),
        template_backends: Fact::unknown(vec![reason(Field::SettingsTemplates, file, message)]),
    }
}

fn reason(field: Field, file: &Utf8Path, message: impl Into<String>) -> Reason {
    Reason::file(field, file, message)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use djls_conf::Settings;
    use tempfile::tempdir;

    use super::*;
    use crate::project::django_environments::discover_django_environments;
    use crate::project::module_resolver::discover_module_search_paths;
    use crate::project::names::LibraryName;

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn write_settings(root: &Utf8Path, source: &str) -> Utf8PathBuf {
        let path = root.join("project/settings.py");
        write_file(&path, source);
        path
    }

    fn search_paths(root: &Utf8Path) -> Vec<ModuleSearchPathEntry> {
        discover_module_search_paths(root, &[], &[])
            .value()
            .cloned()
            .unwrap()
    }

    fn extract_project_settings_module(
        root: &Utf8Path,
        settings_file: &Utf8Path,
        module: &str,
    ) -> SettingsFacts {
        let module = PyModuleName::parse(module).unwrap();
        let search_paths = search_paths(root);
        extract_settings_facts_for_module(settings_file, &module, root, &search_paths)
    }

    fn known_vec<T: Clone + std::fmt::Debug>(fact: &Fact<Vec<T>>) -> Vec<T> {
        let Fact::Known { value } = fact else {
            panic!("expected known fact, got {fact:?}");
        };
        value.clone()
    }

    fn partial_vec<T: Clone + std::fmt::Debug>(fact: &Fact<Vec<T>>) -> (Vec<T>, Vec<Reason>) {
        let Fact::Partial { value, reasons } = fact else {
            panic!("expected partial fact, got {fact:?}");
        };
        (value.clone(), reasons.clone())
    }

    fn known_bool(fact: &Fact<bool>) -> bool {
        let Fact::Known { value } = fact else {
            panic!("expected known bool, got {fact:?}");
        };
        *value
    }

    fn unknown_reasons<T: std::fmt::Debug>(fact: &Fact<T>) -> Vec<Reason> {
        let Fact::Unknown { reasons } = fact else {
            panic!("expected unknown fact, got {fact:?}");
        };
        reasons.clone()
    }

    #[test]
    fn extracts_literal_installed_apps_and_templates() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = [
    "django.contrib.auth",
    "project.app",
]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates", BASE_DIR.joinpath("more_templates")],
        "APP_DIRS": True,
        "OPTIONS": {
            "libraries": {
                "custom_tags": "project.templatetags.custom_tags",
            },
            "builtins": ["project.templatetags.builtins"],
        },
    }
]
"#,
        );

        let facts = extract_settings_facts(&settings);

        assert_eq!(
            known_vec(&facts.installed_apps),
            ["django.contrib.auth", "project.app"]
        );
        let backends = known_vec(&facts.template_backends);
        assert_eq!(backends.len(), 1);
        assert_eq!(
            backends[0].backend.as_deref(),
            Some("django.template.backends.django.DjangoTemplates")
        );
        assert!(known_bool(&backends[0].app_dirs));
        assert_eq!(
            known_vec(&backends[0].dirs)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("templates"), root.join("more_templates")]
        );
        assert_eq!(
            known_vec(&backends[0].option_libraries)[0].load_name,
            LibraryName::parse("custom_tags").unwrap()
        );
        assert_eq!(
            known_vec(&backends[0].option_builtins),
            [PyModuleName::parse("project.templatetags.builtins").unwrap()]
        );
    }

    #[test]
    fn extracts_typed_settings_assignments() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
INSTALLED_APPS: list[str] = ["django.contrib.auth"]
TEMPLATES: list[dict] = []
"#,
        );

        let facts = extract_settings_facts(&settings);

        assert_eq!(known_vec(&facts.installed_apps), ["django.contrib.auth"]);
        assert!(known_vec(&facts.template_backends).is_empty());
    }

    #[test]
    fn extracts_pathlib_parents_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[1]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }

    #[test]
    fn extracts_pathlib_parents_zero_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

PROJECT_DIR = Path(__file__).resolve().parents[0]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [PROJECT_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(
            known_vec(&backends[0].dirs)[0].path,
            root.join("project/templates")
        );
    }

    #[test]
    fn marks_out_of_range_pathlib_parents_template_dirs_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[100]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);
        let (dirs, reasons) = partial_vec(&backends[0].dirs);

        assert!(dirs.is_empty());
        assert!(reasons[0].message.contains("unsupported path expression"));
    }

    #[test]
    fn marks_unreadable_settings_file_unknown() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let facts = extract_settings_facts(&root.join("project/missing.py"));

        let reasons = unknown_reasons(&facts.installed_apps);
        assert!(reasons[0].message.contains("failed to read settings file"));
        let reasons = unknown_reasons(&facts.template_backends);
        assert!(reasons[0].message.contains("failed to read settings file"));
    }

    #[test]
    fn marks_parse_error_settings_file_unknown() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(&root, "INSTALLED_APPS = [\n");

        let facts = extract_settings_facts(&settings);

        let reasons = unknown_reasons(&facts.installed_apps);
        assert!(reasons[0].message.contains("failed to parse settings file"));
        let reasons = unknown_reasons(&facts.template_backends);
        assert!(reasons[0].message.contains("failed to parse settings file"));
    }

    #[test]
    fn extracts_settings_facts_from_resolved_settings_file() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
INSTALLED_APPS = ["project.app"]
TEMPLATES = [{"DIRS": ["templates"]}]
"#,
        );
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_module = "project.settings""#,
        );

        let settings = Settings::new(&root, None).unwrap();
        let search_paths = discover_module_search_paths(&root, &[], &[])
            .value()
            .cloned()
            .unwrap();
        let environments = discover_django_environments(&root, &settings, &search_paths);
        let Fact::Known {
            value: django_settings,
        } = &environments[0].django_settings
        else {
            panic!(
                "expected resolved settings module, got {:?}",
                environments[0].django_settings
            );
        };

        let facts = extract_settings_facts(&django_settings.file);

        assert_eq!(known_vec(&facts.installed_apps), ["project.app"]);
        assert_eq!(known_vec(&facts.template_backends).len(), 1);
    }

    #[test]
    fn extracts_split_settings_from_relative_star_import() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[2]
INSTALLED_APPS = ["project.base"]
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );
        let dev_settings = root.join("project/settings/dev.py");
        write_file(
            &dev_settings,
            r"
from .base import *
",
        );

        let facts = extract_project_settings_module(&root, &dev_settings, "project.settings.dev");

        assert_eq!(known_vec(&facts.installed_apps), ["project.base"]);
        let backends = known_vec(&facts.template_backends);
        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }

    #[test]
    fn leaf_settings_override_star_imported_settings() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            r#"
INSTALLED_APPS = ["project.base"]
TEMPLATES = [{"APP_DIRS": True}]
"#,
        );
        let dev_settings = root.join("project/settings/dev.py");
        write_file(
            &dev_settings,
            r#"
from .base import *

INSTALLED_APPS = ["project.dev"]
"#,
        );

        let facts = extract_project_settings_module(&root, &dev_settings, "project.settings.dev");

        assert_eq!(known_vec(&facts.installed_apps), ["project.dev"]);
        assert!(known_bool(&known_vec(&facts.template_backends)[0].app_dirs));
    }

    #[test]
    fn leaf_settings_mutate_star_imported_settings() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            r#"
INSTALLED_APPS = ["project.base"]
TEMPLATES = []
"#,
        );
        let dev_settings = root.join("project/settings/dev.py");
        write_file(
            &dev_settings,
            r#"
from .base import *

INSTALLED_APPS += ["project.dev"]
TEMPLATES.append({"APP_DIRS": True})
"#,
        );

        let facts = extract_project_settings_module(&root, &dev_settings, "project.settings.dev");

        assert_eq!(
            known_vec(&facts.installed_apps),
            ["project.base", "project.dev"]
        );
        assert!(known_bool(&known_vec(&facts.template_backends)[0].app_dirs));
    }

    #[test]
    fn failed_star_import_preserves_leaf_values_as_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            "INSTALLED_APPS = [\n",
        );
        let dev_settings = root.join("project/settings/dev.py");
        write_file(
            &dev_settings,
            r#"
INSTALLED_APPS = ["project.dev"]
TEMPLATES = []
from .base import *
"#,
        );

        let facts = extract_project_settings_module(&root, &dev_settings, "project.settings.dev");
        let (apps, reasons) = partial_vec(&facts.installed_apps);

        assert_eq!(apps, ["project.dev"]);
        assert_eq!(reasons[0].field, Field::SettingsInstalledApps);
        assert!(reasons[0].message.contains("failed to parse settings file"));

        let (templates, reasons) = partial_vec(&facts.template_backends);
        assert!(templates.is_empty());
        assert_eq!(reasons[0].field, Field::SettingsTemplates);
        assert!(reasons[0].message.contains("failed to parse settings file"));
    }

    #[test]
    fn star_imported_path_values_feed_leaf_settings() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            r"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[2]
INSTALLED_APPS = []
",
        );
        let dev_settings = root.join("project/settings/dev.py");
        write_file(
            &dev_settings,
            r#"
from .base import *

TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );

        let facts = extract_project_settings_module(&root, &dev_settings, "project.settings.dev");
        let backends = known_vec(&facts.template_backends);

        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }

    #[test]
    fn extracts_settings_facts_from_discovered_split_settings_file() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings/__init__.py"), "");
        write_file(
            &root.join("project/settings/base.py"),
            r#"
INSTALLED_APPS = ["project.base"]
TEMPLATES = []
"#,
        );
        write_file(
            &root.join("project/settings/dev.py"),
            r"
from .base import *
",
        );
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_module = "project.settings.dev""#,
        );

        let settings = Settings::new(&root, None).unwrap();
        let search_paths = search_paths(&root);
        let environments = discover_django_environments(&root, &settings, &search_paths);
        let Fact::Known {
            value: django_settings,
        } = &environments[0].django_settings
        else {
            panic!(
                "expected resolved settings module, got {:?}",
                environments[0].django_settings
            );
        };

        let facts = extract_settings_facts_for_module(
            &django_settings.file,
            &django_settings.module,
            &root,
            &search_paths,
        );

        assert_eq!(known_vec(&facts.installed_apps), ["project.base"]);
    }

    #[test]
    fn marks_unsupported_installed_apps_entries_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
LOCAL_APPS = ["project.app"]
INSTALLED_APPS = ["django.contrib.auth", *LOCAL_APPS]
TEMPLATES = []
"#,
        );

        let facts = extract_settings_facts(&settings);

        let (apps, reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(apps, ["django.contrib.auth"]);
        assert_eq!(reasons[0].field, Field::SettingsInstalledApps);
        assert!(reasons[0].message.contains("non-string"));
    }

    #[test]
    fn applies_installed_apps_list_mutations() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
INSTALLED_APPS = ["django.contrib.auth"]
INSTALLED_APPS.append("project.appended")
INSTALLED_APPS.extend(["project.extended"])
INSTALLED_APPS += ["project.added"]
INSTALLED_APPS.insert(1, "project.inserted")
TEMPLATES = []
"#,
        );

        let facts = extract_settings_facts(&settings);

        assert_eq!(
            known_vec(&facts.installed_apps),
            [
                "django.contrib.auth",
                "project.inserted",
                "project.appended",
                "project.extended",
                "project.added",
            ]
        );
    }

    #[test]
    fn applies_template_backend_list_mutations() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
INSTALLED_APPS = []
TEMPLATES = []
TEMPLATES.append({"APP_DIRS": True})
TEMPLATES += [{"DIRS": ["templates"]}]
TEMPLATES.insert(1, {"BACKEND": "django.template.backends.django.DjangoTemplates"})
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(backends.len(), 3);
        assert!(known_bool(&backends[0].app_dirs));
        assert_eq!(
            backends[1].backend.as_deref(),
            Some("django.template.backends.django.DjangoTemplates")
        );
        assert_eq!(
            known_vec(&backends[2].dirs)[0].path,
            Utf8PathBuf::from("templates")
        );
    }

    #[test]
    fn marks_unsupported_settings_mutations_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
LOCAL_APPS = ["project.dynamic"]
INSTALLED_APPS = ["django.contrib.auth"]
INSTALLED_APPS.extend(LOCAL_APPS)
TEMPLATES = []
TEMPLATES.update({"APP_DIRS": True})
"#,
        );

        let facts = extract_settings_facts(&settings);

        let (apps, app_reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(apps, ["django.contrib.auth"]);
        assert!(app_reasons[0].message.contains("literal list or tuple"));

        let (templates, template_reasons) = partial_vec(&facts.template_backends);
        assert!(templates.is_empty());
        assert!(template_reasons[0]
            .message
            .contains("unsupported dynamic mutation"));
    }

    #[test]
    fn extracts_os_path_join_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path
import os

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [os.path.join(BASE_DIR, "templates")]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }

    #[test]
    fn extracts_os_path_dirname_and_concatenated_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
import os

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEMPLATES_DIR = BASE_DIR + "/templates"
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [TEMPLATES_DIR, BASE_DIR + "/more_templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(
            known_vec(&backends[0].dirs)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("templates"), root.join("more_templates")]
        );
    }

    #[test]
    fn marks_os_path_abspath_relative_template_dirs_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
import os

INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [os.path.abspath("templates")]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);
        let (dirs, reasons) = partial_vec(&backends[0].dirs);

        assert!(dirs.is_empty());
        assert!(reasons[0].message.contains("unsupported path expression"));
    }

    #[test]
    fn extracts_os_pardir_normpath_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = root.join("project/conf/settings.py");
        write_file(
            &settings,
            r#"
import os

PROJECT_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), os.pardir))
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [os.path.join(PROJECT_ROOT, "templates")]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(
            known_vec(&backends[0].dirs)[0].path,
            root.join("project/templates")
        );
    }

    #[test]
    fn extracts_os_path_pardir_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = root.join("project/conf/settings.py");
        write_file(
            &settings,
            r#"
import os

PROJECT_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), os.path.pardir))
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [os.path.join(PROJECT_ROOT, "templates")]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(
            known_vec(&backends[0].dirs)[0].path,
            root.join("project/templates")
        );
    }
}
