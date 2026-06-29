use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::ArgumentCountConstraint;
use djls_project::ChoiceAt;
use djls_project::ExtractedDiagnosticConstraint;
use djls_project::ExtractedDiagnosticMessage;
use djls_project::ExtractedMessageTemplate;
use djls_project::FilterArity;
use djls_project::Interpreter;
use djls_project::LibraryName;
use djls_project::Project;
use djls_project::PythonModulePath;
use djls_project::RequiredKeyword;
use djls_project::SearchPaths;
use djls_project::SplitPosition;
use djls_project::SymbolDefinition;
use djls_project::SymbolKey;
use djls_project::TagRule;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_project::testing;
use djls_project::testing::BuiltinInput;
use djls_project::testing::InactiveInput;
use djls_project::testing::LoadableInput;
use djls_project::testing::StaticKnowledge;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::builtin_tag_specs;
use djls_source::Db as _;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::parse_template;

use crate::Corpus;
use crate::TestDatabase;
use crate::extract_bundle;
use crate::module_path_from_file;

#[must_use]
pub fn builtin_tag(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn library_tag(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn builtin_filter(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn library_filter(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveTemplateLibraryFixture {
    load_name: String,
    app: String,
    module: String,
}

#[must_use]
pub fn inactive_template_library(
    load_name: &str,
    app: &str,
    module: &str,
) -> InactiveTemplateLibraryFixture {
    InactiveTemplateLibraryFixture {
        load_name: load_name.to_string(),
        app: app.to_string(),
        module: module.to_string(),
    }
}

#[must_use]
pub fn inactive_library_tag(
    name: &str,
    load_name: &str,
    app: &str,
    module: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": load_name,
        "inactive_app": app,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn inactive_library_filter(
    name: &str,
    load_name: &str,
    app: &str,
    module: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": load_name,
        "inactive_app": app,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[derive(serde::Deserialize)]
struct TemplateSymbolFixture {
    kind: TemplateSymbolKind,
    name: String,
    #[serde(default)]
    load_name: Option<String>,
    #[serde(default)]
    inactive_app: Option<String>,
    library_module: String,
    module: String,
    #[serde(default)]
    doc: Option<String>,
}

/// Build Template Library facts from JSON fixture rows.
///
/// # Panics
///
/// Panics if a fixture row does not match the expected `TemplateSymbolFixture` shape.
pub fn make_template_libraries(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> TemplateLibraries {
    make_template_libraries_with_knowledge(
        tags,
        filters,
        libraries,
        builtins,
        StaticKnowledge::Known,
    )
}

/// Build Template Library facts from JSON fixture rows with explicit knowledge.
///
/// # Panics
///
/// Panics if a fixture row does not match the expected `TemplateSymbolFixture` shape.
pub fn make_template_libraries_with_knowledge(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
    knowledge: StaticKnowledge,
) -> TemplateLibraries {
    make_template_libraries_with_inactive_and_knowledge(
        tags,
        filters,
        libraries,
        builtins,
        &[],
        knowledge,
    )
}

/// Build Template Library facts from JSON fixture rows plus inactive libraries.
///
/// # Panics
///
/// Panics if a fixture row does not match the expected `TemplateSymbolFixture` shape.
pub fn make_template_libraries_with_inactive(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
    inactive_libraries: &[InactiveTemplateLibraryFixture],
) -> TemplateLibraries {
    make_template_libraries_with_inactive_and_knowledge(
        tags,
        filters,
        libraries,
        builtins,
        inactive_libraries,
        StaticKnowledge::Known,
    )
}

/// Build Template Library facts from JSON fixture rows plus inactive libraries with explicit knowledge.
///
/// # Panics
///
/// Panics if a fixture row does not match the expected `TemplateSymbolFixture` shape.
pub fn make_template_libraries_with_inactive_and_knowledge(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
    inactive_libraries: &[InactiveTemplateLibraryFixture],
    knowledge: StaticKnowledge,
) -> TemplateLibraries {
    let mut builtin_symbols = builtin_symbol_buckets(builtins);
    let mut loadable_symbols = loadable_symbol_buckets(libraries);
    let mut inactive_symbols = inactive_symbol_buckets(inactive_libraries);

    for fixture in tags
        .iter()
        .chain(filters.iter())
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<TemplateSymbolFixture>, _>>()
        .unwrap()
    {
        add_fixture_symbol(
            fixture,
            &mut builtin_symbols,
            &mut loadable_symbols,
            &mut inactive_symbols,
        );
    }

    let builtins = builtin_symbols
        .into_iter()
        .map(|(module, symbols)| BuiltinInput { module, symbols })
        .collect();
    let loadables = loadable_symbols
        .into_iter()
        .map(|(load_name, (module, symbols))| LoadableInput {
            load_name,
            module,
            symbols,
        })
        .collect();
    let inactives = inactive_symbols
        .into_iter()
        .map(|((load_name, app, module), symbols)| InactiveInput {
            load_name,
            app,
            module,
            symbols,
        })
        .collect();

    testing::template_libraries(knowledge, builtins, loadables, inactives)
}

type BuiltinSymbolBuckets = Vec<(PythonModulePath, Vec<TemplateSymbol>)>;
type LoadableSymbolBuckets = BTreeMap<LibraryName, (PythonModulePath, Vec<TemplateSymbol>)>;
type InactiveSymbolBuckets =
    BTreeMap<(LibraryName, PythonModulePath, PythonModulePath), Vec<TemplateSymbol>>;

fn builtin_symbol_buckets(builtins: &[String]) -> BuiltinSymbolBuckets {
    builtins
        .iter()
        .filter_map(|module_name| PythonModulePath::parse(module_name).ok())
        .map(|module| (module, Vec::new()))
        .collect()
}

fn loadable_symbol_buckets(
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
) -> LoadableSymbolBuckets {
    let mut buckets = BTreeMap::new();
    for (load_name, module_name) in libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            continue;
        };
        let Ok(module) = PythonModulePath::parse(module_name) else {
            continue;
        };
        buckets.insert(load_name, (module, Vec::new()));
    }
    buckets
}

fn inactive_symbol_buckets(
    inactive_libraries: &[InactiveTemplateLibraryFixture],
) -> InactiveSymbolBuckets {
    let mut buckets = BTreeMap::new();
    for library in inactive_libraries {
        let Ok(load_name) = LibraryName::parse(&library.load_name) else {
            continue;
        };
        let Ok(app) = PythonModulePath::parse(&library.app) else {
            continue;
        };
        let Ok(module) = PythonModulePath::parse(&library.module) else {
            continue;
        };
        buckets.entry((load_name, app, module)).or_default();
    }
    buckets
}

fn add_fixture_symbol(
    fixture: TemplateSymbolFixture,
    builtin_symbols: &mut BuiltinSymbolBuckets,
    loadable_symbols: &mut LoadableSymbolBuckets,
    inactive_symbols: &mut InactiveSymbolBuckets,
) {
    let TemplateSymbolFixture {
        kind,
        name,
        load_name,
        inactive_app,
        library_module,
        module,
        doc,
    } = fixture;
    let Ok(name) = TemplateSymbolName::parse(&name) else {
        return;
    };
    let definition = PythonModulePath::parse(&module)
        .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module);
    let symbol = TemplateSymbol {
        kind,
        name,
        definition,
        doc,
    };

    match (load_name, inactive_app) {
        (None, _) => add_builtin_symbol(builtin_symbols, &library_module, &symbol),
        (Some(load_name), Some(app)) => {
            add_inactive_symbol(inactive_symbols, &load_name, &app, &library_module, symbol);
        }
        (Some(load_name), None) => {
            add_loadable_symbol(loadable_symbols, &load_name, &library_module, symbol);
        }
    }
}

fn add_builtin_symbol(
    buckets: &mut BuiltinSymbolBuckets,
    module_name: &str,
    symbol: &TemplateSymbol,
) {
    let Ok(module) = PythonModulePath::parse(module_name) else {
        return;
    };
    for (builtin_module, symbols) in buckets.iter_mut() {
        if builtin_module == &module {
            symbols.push(symbol.clone());
        }
    }
}

fn add_loadable_symbol(
    buckets: &mut LoadableSymbolBuckets,
    load_name: &str,
    module_name: &str,
    symbol: TemplateSymbol,
) {
    let Ok(load_name) = LibraryName::parse(load_name) else {
        return;
    };
    let Ok(module) = PythonModulePath::parse(module_name) else {
        return;
    };
    let entry = buckets
        .entry(load_name)
        .or_insert_with(|| (module.clone(), Vec::new()));
    if entry.0 == module {
        entry.1.push(symbol);
    }
}

fn add_inactive_symbol(
    buckets: &mut InactiveSymbolBuckets,
    load_name: &str,
    app: &str,
    module_name: &str,
    symbol: TemplateSymbol,
) {
    let Ok(load_name) = LibraryName::parse(load_name) else {
        return;
    };
    let Ok(app) = PythonModulePath::parse(app) else {
        return;
    };
    let Ok(module) = PythonModulePath::parse(module_name) else {
        return;
    };
    buckets
        .entry((load_name, app, module))
        .or_default()
        .push(symbol);
}

pub struct ProjectFixture {
    root: Utf8PathBuf,
    files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: Option<String>,
    pythonpath: Vec<String>,
    env_vars: Vec<(String, String)>,
    interpreter: Interpreter,
    search_paths: Option<SearchPaths>,
    register_roots: bool,
    tag_specs: TagSpecDef,
}

impl ProjectFixture {
    #[must_use]
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        let settings = Settings::default();
        Self {
            root: root.into(),
            files: Vec::new(),
            django_settings_module: None,
            pythonpath: Vec::new(),
            env_vars: Vec::new(),
            interpreter: Interpreter::discover(settings.venv_path()),
            search_paths: None,
            register_roots: true,
            tag_specs: settings.tagspecs().clone(),
        }
    }

    #[must_use]
    pub fn file(mut self, path: impl Into<Utf8PathBuf>, source: impl Into<String>) -> Self {
        self.files.push((path.into(), source.into()));
        self
    }

    #[must_use]
    pub fn django_settings_module(mut self, module: impl Into<String>) -> Self {
        self.django_settings_module = Some(module.into());
        self
    }

    #[must_use]
    pub fn interpreter(mut self, interpreter: Interpreter) -> Self {
        self.interpreter = interpreter;
        self
    }

    #[must_use]
    pub fn search_paths(mut self, search_paths: SearchPaths) -> Self {
        self.search_paths = Some(search_paths);
        self
    }

    #[must_use]
    pub fn register_roots(mut self, register_roots: bool) -> Self {
        self.register_roots = register_roots;
        self
    }

    #[must_use]
    pub fn template_file(
        self,
        _name: impl Into<String>,
        path: impl Into<Utf8PathBuf>,
        source: impl Into<String>,
    ) -> Self {
        self.file(path, source)
    }

    pub fn build(self, db: &TestDatabase) -> Project {
        for (path, source) in self.files {
            db.add_file(path.as_str(), &source);
        }

        let search_paths = self.search_paths.unwrap_or_else(|| {
            SearchPaths::from_project_settings(
                db.file_system(),
                &self.root,
                &self.interpreter,
                &self.pythonpath,
            )
        });
        if self.register_roots {
            search_paths.register_roots(db);
        }

        Project::new(
            db,
            self.root,
            search_paths,
            self.interpreter,
            self.django_settings_module,
            self.pythonpath,
            self.env_vars,
            self.tag_specs,
        )
    }

    pub fn install(self, db: &mut TestDatabase) -> Project {
        let project = self.build(db);
        db.set_project(project);
        project
    }
}

pub fn collect_errors(db: &TestDatabase, path: &str, source: &str) -> Vec<ValidationError> {
    collect_errors_with_revision(db, path, 0, source)
}

pub fn collect_errors_with_revision(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> Vec<ValidationError> {
    db.add_file(path, source);
    let file = db.create_file_with_revision(Utf8Path::new(path), revision);

    let Some(nodelist) = parse_template(db, file) else {
        return Vec::new();
    };

    djls_semantic::validate_nodelist(db, nodelist);

    djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
}

#[must_use]
pub fn is_argument_validation_error(err: &ValidationError) -> bool {
    matches!(
        err,
        ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
    )
}

pub fn collect_argument_validation_errors_with_revision(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> Vec<ValidationError> {
    db.add_file(path, source);
    let file = db.create_file_with_revision(Utf8Path::new(path), revision);

    let Some(nodelist) = parse_template(db, file) else {
        return Vec::new();
    };

    djls_semantic::validate_nodelist(db, nodelist);

    djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .filter(is_argument_validation_error)
        .collect()
}

pub fn extract_and_merge(
    _corpus: &Corpus,
    dir: &Utf8Path,
    specs: &mut TagSpecs,
    arities: &mut FilterAritySpecs,
) {
    let db = TestDatabase::new();

    for file_path in &Corpus::extraction_targets_in(dir) {
        let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
            continue;
        };

        let module_path = module_path_from_file(file_path);
        let Ok(module_path) = PythonModulePath::parse(&module_path) else {
            continue;
        };
        db.add_file(file_path.as_str(), &source);
        let file = db.get_or_create_file(file_path);
        let bundle = extract_bundle(&db, file, module_path);

        arities.merge_filter_arities(&bundle.filter_arities);
        specs
            .merge_block_specs(&bundle.block_specs)
            .merge_tag_rules(&bundle.tag_rules);
    }
}

#[must_use]
pub fn build_specs_from_extraction(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

#[must_use]
pub fn build_entry_specs(corpus: &Corpus, entry_dir: &Utf8Path) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();

    if !Corpus::is_django_entry(entry_dir)
        && let Some(django_dir) = corpus.latest_package("django")
    {
        extract_and_merge(corpus, &django_dir, &mut specs, &mut arities);
    }

    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);

    (specs, arities)
}

/// Render validation errors into a plain-text diagnostic snapshot.
///
/// # Panics
///
/// Panics if a validation error has no primary span.
#[must_use]
pub fn render_diagnostic_snapshot(path: &str, source: &str, errors: &[ValidationError]) -> String {
    let renderer = DiagnosticRenderer::plain();
    let mut parts = Vec::new();

    for err in errors {
        let span = err
            .primary_span()
            .expect("all validation errors have a span");
        let message = err.to_string();
        let code = err.code();

        let mut diag = Diagnostic::new(source, path, code, &message, Severity::Error, span, "");

        if let ValidationError::UnbalancedStructure {
            closing_span: Some(cs),
            ..
        } = err
        {
            diag = diag.annotation(*cs, "", false);
        }

        parts.push(renderer.render(&diag));
    }

    parts.join("\n")
}

#[must_use]
pub fn snapshot_validate(source: &str) -> String {
    snapshot_validate_file("test.html", source)
}

#[must_use]
pub fn snapshot_validate_file(path: &str, source: &str) -> String {
    render_validate_snapshot(&standard_validation_db(), path, 0, source)
}

/// Curated validation environment for mdtest snapshots.
///
/// This keeps diagnostic snapshots deterministic and easy to author. It is not
/// a live Django project inspection fixture; add libraries, tags, and filters
/// here when a scenario needs them.
#[must_use]
pub fn standard_validation_db() -> TestDatabase {
    TestDatabase::new()
        .with_specs(standard_tag_specs())
        .with_template_libraries(standard_template_libraries())
        .with_arity_specs(standard_filter_arities())
}

fn standard_tag_specs() -> TagSpecs {
    let mut specs = builtin_tag_specs();

    set_tag_rule(&mut specs, "autoescape", autoescape_rule());
    set_tag_rule(&mut specs, "cycle", cycle_rule());
    set_tag_rule(&mut specs, "lorem", lorem_rule());
    set_tag_rule(&mut specs, "now", now_rule());
    set_tag_rule(&mut specs, "regroup", regroup_rule());
    set_tag_rule(&mut specs, "url", url_rule());
    set_tag_rule(&mut specs, "widthratio", widthratio_rule());

    specs.insert(
        "one_arg_tag".to_string(),
        TagSpec::new(
            "example.templatetags.custom".into(),
            None,
            Cow::Borrowed(&[]),
            false,
        )
        .with_extracted_rules(
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
                ..TagRule::default()
            }
            .into(),
        ),
    );
    specs
}

fn autoescape_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
        choice_at_constraints: vec![ChoiceAt {
            position: SplitPosition::Forward(1),
            values: vec!["on".to_string(), "off".to_string()],
        }],
        diagnostic_messages: Some(vec![
            count_message(
                ArgumentCountConstraint::Exact(2),
                "'autoescape' tag requires exactly one argument.",
            ),
            choice_message(
                SplitPosition::Forward(1),
                &["on", "off"],
                "'autoescape' argument should be 'on' or 'off'",
            ),
        ]),
        ..TagRule::default()
    }
}

fn cycle_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Min(2)],
        diagnostic_messages: Some(vec![count_message(
            ArgumentCountConstraint::Min(2),
            "'cycle' tag requires at least two arguments",
        )]),
        ..TagRule::default()
    }
}

fn lorem_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Exact(4)],
        diagnostic_messages: Some(vec![count_message(
            ArgumentCountConstraint::Exact(4),
            "Incorrect format for 'lorem' tag",
        )]),
        ..TagRule::default()
    }
}

fn now_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
        diagnostic_messages: Some(vec![count_message(
            ArgumentCountConstraint::Exact(2),
            "'now' statement takes one argument",
        )]),
        ..TagRule::default()
    }
}

fn regroup_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Exact(6)],
        required_keywords: vec![
            RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "by".to_string(),
            },
            RequiredKeyword {
                position: SplitPosition::Forward(4),
                value: "as".to_string(),
            },
        ],
        diagnostic_messages: Some(vec![
            count_message(
                ArgumentCountConstraint::Exact(6),
                "'regroup' tag takes five arguments",
            ),
            keyword_message(
                SplitPosition::Forward(2),
                "by",
                "second argument to 'regroup' tag must be 'by'",
            ),
            keyword_message(
                SplitPosition::Forward(4),
                "as",
                "next-to-last argument to 'regroup' tag must be 'as'",
            ),
        ]),
        ..TagRule::default()
    }
}

fn url_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::Min(2)],
        diagnostic_messages: Some(vec![count_message(
            ArgumentCountConstraint::Min(2),
            "'url' takes at least one argument, a URL pattern name.",
        )]),
        ..TagRule::default()
    }
}

fn widthratio_rule() -> TagRule {
    TagRule {
        arg_constraints: vec![ArgumentCountConstraint::OneOf(vec![4, 6])],
        required_keywords: vec![RequiredKeyword {
            position: SplitPosition::Forward(4),
            value: "as".to_string(),
        }],
        diagnostic_messages: Some(vec![
            count_message(
                ArgumentCountConstraint::OneOf(vec![4, 6]),
                "widthratio takes at least three arguments",
            ),
            keyword_message(
                SplitPosition::Forward(4),
                "as",
                "Invalid syntax in widthratio tag. Expecting 'as' keyword",
            ),
        ]),
        ..TagRule::default()
    }
}

fn set_tag_rule(specs: &mut TagSpecs, name: &str, rule: TagRule) {
    if let Some(spec) = specs.get_mut(name) {
        spec.set_extracted_rules(rule.into());
    }
}

fn count_message(constraint: ArgumentCountConstraint, message: &str) -> ExtractedDiagnosticMessage {
    ExtractedDiagnosticMessage {
        constraint: ExtractedDiagnosticConstraint::ArgumentCount(constraint),
        message: ExtractedMessageTemplate::Static(message.to_string()),
    }
}

fn keyword_message(
    position: SplitPosition,
    value: &str,
    message: &str,
) -> ExtractedDiagnosticMessage {
    ExtractedDiagnosticMessage {
        constraint: ExtractedDiagnosticConstraint::RequiredKeyword {
            position,
            value: value.to_string(),
        },
        message: ExtractedMessageTemplate::Static(message.to_string()),
    }
}

fn choice_message(
    position: SplitPosition,
    values: &[&str],
    message: &str,
) -> ExtractedDiagnosticMessage {
    ExtractedDiagnosticMessage {
        constraint: ExtractedDiagnosticConstraint::ChoiceAt {
            position,
            values: values.iter().map(|value| (*value).to_string()).collect(),
        },
        message: ExtractedMessageTemplate::Static(message.to_string()),
    }
}

fn standard_template_libraries() -> TemplateLibraries {
    let tags = vec![
        builtin_tag("autoescape", default_builtins_module()),
        builtin_tag("block", default_loader_tags_module()),
        builtin_tag("comment", default_builtins_module()),
        builtin_tag("csrf_token", default_builtins_module()),
        builtin_tag("cycle", default_builtins_module()),
        builtin_tag("debug", default_builtins_module()),
        builtin_tag("extends", default_loader_tags_module()),
        builtin_tag("filter", default_builtins_module()),
        builtin_tag("firstof", default_builtins_module()),
        builtin_tag("for", default_builtins_module()),
        builtin_tag("if", default_builtins_module()),
        builtin_tag("ifchanged", default_builtins_module()),
        builtin_tag("include", default_loader_tags_module()),
        builtin_tag("load", default_builtins_module()),
        builtin_tag("lorem", default_builtins_module()),
        builtin_tag("now", default_builtins_module()),
        builtin_tag("one_arg_tag", "example.templatetags.custom"),
        builtin_tag("regroup", default_builtins_module()),
        builtin_tag("spaceless", default_builtins_module()),
        builtin_tag("templatetag", default_builtins_module()),
        builtin_tag("url", default_builtins_module()),
        builtin_tag("verbatim", default_builtins_module()),
        builtin_tag("widthratio", default_builtins_module()),
        builtin_tag("with", default_builtins_module()),
        library_tag("ambiguous_tag", "alpha", "example.alpha.templatetags.alpha"),
        library_tag("ambiguous_tag", "beta", "example.beta.templatetags.beta"),
        library_tag("blocktrans", "i18n", "django.templatetags.i18n"),
        library_tag("blocktranslate", "i18n", "django.templatetags.i18n"),
        library_tag("cache", "cache", "django.templatetags.cache"),
        library_tag("localize", "l10n", "django.templatetags.l10n"),
        library_tag("localtime", "tz", "django.templatetags.tz"),
        library_tag("static", "static", "django.templatetags.static"),
        library_tag("timezone", "tz", "django.templatetags.tz"),
        library_tag("trans", "i18n", "django.templatetags.i18n"),
        library_tag("translate", "i18n", "django.templatetags.i18n"),
    ];
    let filters = vec![
        builtin_filter("title", default_filters_module()),
        builtin_filter("lower", default_filters_module()),
        builtin_filter("length", default_filters_module()),
        builtin_filter("default", default_filters_module()),
        builtin_filter("truncatewords", default_filters_module()),
        builtin_filter("date", default_filters_module()),
        builtin_filter("upper", default_filters_module()),
        library_filter(
            "intcomma",
            "humanize",
            "django.contrib.humanize.templatetags.humanize",
        ),
        library_filter(
            "ambiguous_filter",
            "alpha",
            "example.alpha.templatetags.alpha",
        ),
        library_filter("ambiguous_filter", "beta", "example.beta.templatetags.beta"),
    ];
    let mut libraries = HashMap::new();
    libraries.insert("cache".to_string(), "django.templatetags.cache".to_string());
    libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
    libraries.insert("l10n".to_string(), "django.templatetags.l10n".to_string());
    libraries.insert("tz".to_string(), "django.templatetags.tz".to_string());
    libraries.insert(
        "humanize".to_string(),
        "django.contrib.humanize.templatetags.humanize".to_string(),
    );
    libraries.insert(
        "static".to_string(),
        "django.templatetags.static".to_string(),
    );
    libraries.insert(
        "alpha".to_string(),
        "example.alpha.templatetags.alpha".to_string(),
    );
    libraries.insert(
        "beta".to_string(),
        "example.beta.templatetags.beta".to_string(),
    );
    let builtins = vec![
        default_builtins_module().to_string(),
        default_filters_module().to_string(),
        default_loader_tags_module().to_string(),
        "example.templatetags.custom".to_string(),
    ];

    make_template_libraries(&tags, &filters, &libraries, &builtins)
}

fn standard_filter_arities() -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();
    specs.insert(
        SymbolKey::filter(default_filters_module(), "title"),
        FilterArity {
            expects_arg: false,
            arg_optional: false,
        },
    );
    specs.insert(
        SymbolKey::filter(default_filters_module(), "lower"),
        FilterArity {
            expects_arg: false,
            arg_optional: false,
        },
    );
    specs.insert(
        SymbolKey::filter(default_filters_module(), "upper"),
        FilterArity {
            expects_arg: false,
            arg_optional: false,
        },
    );
    specs.insert(
        SymbolKey::filter(default_filters_module(), "default"),
        FilterArity {
            expects_arg: true,
            arg_optional: false,
        },
    );
    specs.insert(
        SymbolKey::filter(default_filters_module(), "truncatewords"),
        FilterArity {
            expects_arg: true,
            arg_optional: false,
        },
    );
    specs.insert(
        SymbolKey::filter(default_filters_module(), "date"),
        FilterArity {
            expects_arg: true,
            arg_optional: true,
        },
    );
    specs
}

fn default_builtins_module() -> &'static str {
    "django.template.defaulttags"
}

fn default_filters_module() -> &'static str {
    "django.template.defaultfilters"
}

fn default_loader_tags_module() -> &'static str {
    "django.template.loader_tags"
}

pub fn render_validate_snapshot(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> String {
    render_validate_snapshot_filtered(db, path, revision, source, |_| true)
}

pub fn render_validate_snapshot_filtered<F>(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
    filter: F,
) -> String
where
    F: Fn(&ValidationError) -> bool,
{
    let mut errors: Vec<ValidationError> = collect_errors_with_revision(db, path, revision, source)
        .into_iter()
        .filter(|e| filter(e))
        .collect();

    errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));

    render_diagnostic_snapshot(path, source, &errors)
}
