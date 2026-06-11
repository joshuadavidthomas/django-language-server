mod mdtest;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
#[cfg(test)]
use djls_conf::Settings;
#[cfg(test)]
use djls_conf::TagSpecDef;
use djls_corpus::Corpus;
use djls_corpus::module_path_from_file;
#[cfg(test)]
use djls_source::Db as _;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::Severity;
use djls_source::SourceFiles;
use djls_source::Span;
use djls_templates::parse_template;

use crate::db::Db as SemanticDb;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::filters::FilterAritySpecs;
use crate::project::Db as ProjectDb;
#[cfg(test)]
use crate::project::Interpreter;
use crate::project::Project;
use crate::project::ProjectIntrospector;
use crate::project::TemplateLibraries;
use crate::project::TemplateLibrarySnapshot;
use crate::project::TemplateSymbolSnapshot;
#[cfg(test)]
use crate::project::resolve::SearchPaths;
use crate::python::ArgumentCountConstraint;
use crate::python::ChoiceAt;
use crate::python::ExtractedDiagnosticConstraint;
use crate::python::ExtractedDiagnosticMessage;
use crate::python::ExtractedMessageTemplate;
use crate::python::FilterArity;
use crate::python::ModelGraph;
use crate::python::RequiredKeyword;
use crate::python::SplitPosition;
use crate::python::SymbolKey;
use crate::python::TagRule;
use crate::tags::TagSpec;
use crate::tags::TagSpecs;
use crate::tags::builtin_tag_specs;

pub(crate) fn builtin_tag_json(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn builtin_filter_json(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn library_filter_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn make_template_libraries(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> TemplateLibraries {
    let mut symbols: Vec<TemplateSymbolSnapshot> = tags
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .unwrap();

    symbols.extend(
        filters
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<Vec<TemplateSymbolSnapshot>, _>>()
            .unwrap(),
    );

    let response = TemplateLibrarySnapshot {
        symbols,
        libraries: libraries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        builtins: builtins.to_vec(),
    };

    TemplateLibraries::default().apply_active_snapshot(Some(response))
}

pub(crate) fn make_template_libraries_tags_only(
    tags: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> TemplateLibraries {
    make_template_libraries(tags, &[], libraries, builtins)
}

#[salsa::db]
#[derive(Clone)]
pub(crate) struct TestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    files: SourceFiles,
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
    template_libraries: TemplateLibraries,
    project: Option<Project>,
    project_introspector: Arc<ProjectIntrospector>,
}

impl TestDatabase {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            files: SourceFiles::default(),
            tag_specs: builtin_tag_specs(),
            filter_arity_specs: FilterAritySpecs::new(),
            template_libraries: TemplateLibraries::default(),
            project: None,
            project_introspector: Arc::new(ProjectIntrospector::new()),
        }
    }

    #[must_use]
    pub(crate) fn with_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = specs;
        self
    }

    #[must_use]
    pub(crate) fn with_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = specs;
        self
    }

    #[must_use]
    pub(crate) fn with_template_libraries(mut self, template_libraries: TemplateLibraries) -> Self {
        self.template_libraries = template_libraries;
        self
    }

    pub(crate) fn add_file(&self, path: &str, content: &str) {
        self.fs
            .lock()
            .unwrap()
            .add_file(path.into(), content.to_string());
    }

    pub(crate) fn remove_file(&self, path: &str) {
        self.fs.lock().unwrap().remove_file(Utf8Path::new(path));
    }

    pub(crate) fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    #[must_use]
    pub(crate) fn get_or_create_file(&self, path: &Utf8Path) -> File {
        <Self as djls_source::Db>::get_or_create_file(self, path)
    }

    #[must_use]
    pub(crate) fn create_file_with_revision(&self, path: &Utf8Path, revision: u64) -> File {
        File::builder(path.to_owned(), revision)
            .durability(salsa::Durability::LOW)
            .path_durability(salsa::Durability::HIGH)
            .new(self)
    }
}

#[cfg(test)]
pub(crate) struct ProjectFixture {
    root: Utf8PathBuf,
    files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: Option<String>,
    pythonpath: Vec<String>,
    env_vars: Vec<(String, String)>,
    interpreter: Interpreter,
    search_paths: Option<SearchPaths>,
    register_roots: bool,
    tag_specs: TagSpecDef,
    template_libraries: TemplateLibraries,
}

#[cfg(test)]
impl ProjectFixture {
    #[must_use]
    pub(crate) fn new(root: impl Into<Utf8PathBuf>) -> Self {
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
            template_libraries: TemplateLibraries::default(),
        }
    }

    #[must_use]
    pub(crate) fn file(mut self, path: impl Into<Utf8PathBuf>, source: impl Into<String>) -> Self {
        self.files.push((path.into(), source.into()));
        self
    }

    #[must_use]
    pub(crate) fn django_settings_module(mut self, module: impl Into<String>) -> Self {
        self.django_settings_module = Some(module.into());
        self
    }

    #[must_use]
    pub(crate) fn pythonpath(mut self, path: impl Into<String>) -> Self {
        self.pythonpath.push(path.into());
        self
    }

    #[must_use]
    pub(crate) fn interpreter(mut self, interpreter: Interpreter) -> Self {
        self.interpreter = interpreter;
        self
    }

    #[must_use]
    pub(crate) fn search_paths(mut self, search_paths: SearchPaths) -> Self {
        self.search_paths = Some(search_paths);
        self
    }

    #[must_use]
    pub(crate) fn register_roots(mut self, register_roots: bool) -> Self {
        self.register_roots = register_roots;
        self
    }

    #[must_use]
    pub(crate) fn template_libraries(mut self, template_libraries: TemplateLibraries) -> Self {
        self.template_libraries = template_libraries;
        self
    }

    #[must_use]
    pub(crate) fn template_file(
        self,
        _name: impl Into<String>,
        path: impl Into<Utf8PathBuf>,
        source: impl Into<String>,
    ) -> Self {
        self.file(path, source)
    }

    pub(crate) fn build(self, db: &TestDatabase) -> Project {
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
            self.template_libraries,
        )
    }

    pub(crate) fn install(self, db: &mut TestDatabase) -> Project {
        let project = self.build(db);
        db.set_project(project);
        project
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl djls_source::Db for TestDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl ProjectDb for TestDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }

    fn project_introspector(&self) -> Arc<ProjectIntrospector> {
        self.project_introspector.clone()
    }
}

#[salsa::db]
impl SemanticDb for TestDatabase {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        self.project.and_then(|project| {
            let (dirs, knowledge) = crate::project::template_dirs(self, project);
            (*knowledge == crate::project::StaticKnowledge::Known).then(|| dirs.clone())
        })
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn template_libraries(&self) -> &TemplateLibraries {
        &self.template_libraries
    }

    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.filter_arity_specs
    }

    fn model_graph(&self) -> &ModelGraph {
        ModelGraph::empty_ref()
    }
}

pub(crate) fn collect_errors(db: &TestDatabase, path: &str, source: &str) -> Vec<ValidationError> {
    collect_errors_with_revision(db, path, 0, source)
}

pub(crate) fn collect_errors_with_revision(
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

    crate::validate_nodelist(db, nodelist);

    crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
}

pub(crate) fn is_argument_validation_error(err: &ValidationError) -> bool {
    matches!(
        err,
        ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
    )
}

pub(crate) fn collect_argument_validation_errors_with_revision(
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

    crate::validate_nodelist(db, nodelist);

    crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .filter(is_argument_validation_error)
        .collect()
}

pub(crate) fn extract_and_merge(
    corpus: &Corpus,
    dir: &Utf8Path,
    specs: &mut TagSpecs,
    arities: &mut FilterAritySpecs,
) {
    for file_path in &corpus.extraction_targets_in(dir) {
        let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
            continue;
        };

        let module_path = module_path_from_file(file_path);
        let result = crate::extract_rules(&source, &module_path);
        arities.merge_extraction_result(&result);
        specs.merge_extraction_results(&result);
    }
}

pub(crate) fn build_specs_from_extraction(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

pub(crate) fn build_entry_specs(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();

    if !corpus.is_django_entry(entry_dir)
        && let Some(django_dir) = corpus.latest_package("django")
    {
        extract_and_merge(corpus, &django_dir, &mut specs, &mut arities);
    }

    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);

    (specs, arities)
}

pub(crate) fn render_diagnostic_snapshot(
    path: &str,
    source: &str,
    errors: &[ValidationError],
) -> String {
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

pub(crate) fn snapshot_validate(source: &str) -> String {
    snapshot_validate_file("test.html", source)
}

pub(crate) fn snapshot_validate_file(path: &str, source: &str) -> String {
    render_validate_snapshot(&standard_validation_db(), path, 0, source)
}

/// Curated validation environment for mdtest snapshots.
///
/// This keeps diagnostic snapshots deterministic and easy to author. It is not
/// a live Django project inspection fixture; add libraries, tags, and filters
/// here when a scenario needs them.
pub(crate) fn standard_validation_db() -> TestDatabase {
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
        builtin_tag_json("autoescape", default_builtins_module()),
        builtin_tag_json("block", default_loader_tags_module()),
        builtin_tag_json("comment", default_builtins_module()),
        builtin_tag_json("csrf_token", default_builtins_module()),
        builtin_tag_json("cycle", default_builtins_module()),
        builtin_tag_json("debug", default_builtins_module()),
        builtin_tag_json("extends", default_loader_tags_module()),
        builtin_tag_json("filter", default_builtins_module()),
        builtin_tag_json("firstof", default_builtins_module()),
        builtin_tag_json("for", default_builtins_module()),
        builtin_tag_json("if", default_builtins_module()),
        builtin_tag_json("ifchanged", default_builtins_module()),
        builtin_tag_json("include", default_loader_tags_module()),
        builtin_tag_json("load", default_builtins_module()),
        builtin_tag_json("lorem", default_builtins_module()),
        builtin_tag_json("now", default_builtins_module()),
        builtin_tag_json("one_arg_tag", "example.templatetags.custom"),
        builtin_tag_json("regroup", default_builtins_module()),
        builtin_tag_json("spaceless", default_builtins_module()),
        builtin_tag_json("templatetag", default_builtins_module()),
        builtin_tag_json("url", default_builtins_module()),
        builtin_tag_json("verbatim", default_builtins_module()),
        builtin_tag_json("widthratio", default_builtins_module()),
        builtin_tag_json("with", default_builtins_module()),
        library_tag_json("ambiguous_tag", "alpha", "example.alpha.templatetags.alpha"),
        library_tag_json("ambiguous_tag", "beta", "example.beta.templatetags.beta"),
        library_tag_json("blocktrans", "i18n", "django.templatetags.i18n"),
        library_tag_json("blocktranslate", "i18n", "django.templatetags.i18n"),
        library_tag_json("cache", "cache", "django.templatetags.cache"),
        library_tag_json("localize", "l10n", "django.templatetags.l10n"),
        library_tag_json("localtime", "tz", "django.templatetags.tz"),
        library_tag_json("static", "static", "django.templatetags.static"),
        library_tag_json("timezone", "tz", "django.templatetags.tz"),
        library_tag_json("trans", "i18n", "django.templatetags.i18n"),
        library_tag_json("translate", "i18n", "django.templatetags.i18n"),
    ];
    let filters = vec![
        builtin_filter_json("title", default_filters_module()),
        builtin_filter_json("lower", default_filters_module()),
        builtin_filter_json("length", default_filters_module()),
        builtin_filter_json("default", default_filters_module()),
        builtin_filter_json("truncatewords", default_filters_module()),
        builtin_filter_json("date", default_filters_module()),
        builtin_filter_json("upper", default_filters_module()),
        library_filter_json(
            "intcomma",
            "humanize",
            "django.contrib.humanize.templatetags.humanize",
        ),
        library_filter_json(
            "ambiguous_filter",
            "alpha",
            "example.alpha.templatetags.alpha",
        ),
        library_filter_json("ambiguous_filter", "beta", "example.beta.templatetags.beta"),
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

pub(crate) fn render_validate_snapshot(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> String {
    render_validate_snapshot_filtered(db, path, revision, source, |_| true)
}

pub(crate) fn render_validate_snapshot_filtered<F>(
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

#[cfg(test)]
mod project_fixture_tests {
    use djls_source::Db as _;

    use super::*;

    #[test]
    fn project_fixture_builds_project_with_settings_and_search_paths() {
        let mut db = TestDatabase::new();
        let project = ProjectFixture::new("/fixture")
            .file("/fixture/app/__init__.py", "")
            .django_settings_module("fixture.settings")
            .pythonpath("/fixture/app")
            .install(&mut db);

        assert_eq!(
            project.django_settings_module(&db).as_deref(),
            Some("fixture.settings")
        );

        let paths: Vec<_> = project
            .search_paths(&db)
            .iter()
            .map(super::super::project::resolve::SearchPath::path)
            .collect();
        assert_eq!(
            paths,
            [Utf8Path::new("/fixture"), Utf8Path::new("/fixture/app")]
        );
        assert!(
            db.files()
                .root(&db, Utf8Path::new("/fixture/app/__init__.py"))
                .is_some()
        );
        assert_eq!(db.project(), Some(project));
    }
}
