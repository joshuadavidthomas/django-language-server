use std::collections::BTreeMap;
use std::collections::HashMap;

use anyhow::Context as _;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticSeverity;
use djls_conf::DiagnosticsConfig;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::Db as ProjectDb;
use djls_project::Interpreter;
use djls_project::LibraryName;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::SearchPath;
use djls_project::SearchPaths;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraryCatalog;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_project::testing;
use djls_project::testing::TemplateLibraryInput;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::builtin_tag_specs;
use djls_semantic::validate_template_file;
use djls_source::Db as _;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::Span;
use serde_json::from_value;
use serde_json::json;

use crate::Corpus;
use crate::OsTestDatabase;
use crate::TestDatabase;
use crate::extract_bundle;
use crate::module_name_from_file;
use crate::settings::ProjectSettings;

#[must_use]
pub fn builtin_tag(name: &str, module: &str) -> serde_json::Value {
    json!({
        "kind": "tag",
        "name": name,
        "library_kind": "builtin",
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn library_tag(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    json!({
        "kind": "tag",
        "name": name,
        "library_kind": "loadable",
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn builtin_filter(name: &str, module: &str) -> serde_json::Value {
    json!({
        "kind": "filter",
        "name": name,
        "library_kind": "builtin",
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[must_use]
pub fn library_filter(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    json!({
        "kind": "filter",
        "name": name,
        "library_kind": "loadable",
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

#[derive(serde::Deserialize)]
struct TemplateSymbolFixture {
    kind: TemplateSymbolKind,
    name: String,
    #[serde(flatten)]
    library: TemplateSymbolLibraryFixture,
    library_module: String,
    module: String,
    #[serde(default)]
    doc: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "library_kind", rename_all = "snake_case")]
enum TemplateSymbolLibraryFixture {
    Builtin,
    Loadable { load_name: String },
}

/// Build Template Library facts from JSON fixture rows.
pub fn make_template_library_catalog<'db>(
    db: &'db dyn ProjectDb,
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> anyhow::Result<TemplateLibraryCatalog<'db>> {
    let mut builtin_symbols = builtin_symbol_buckets(builtins)?;
    let mut loadable_symbols = loadable_symbol_buckets(libraries)?;

    let fixtures = tags
        .iter()
        .chain(filters.iter())
        .cloned()
        .map(from_value)
        .collect::<Result<Vec<TemplateSymbolFixture>, _>>()
        .context("failed to deserialize template symbol fixture")?;
    for fixture in fixtures {
        add_fixture_symbol(fixture, &mut builtin_symbols, &mut loadable_symbols)?;
    }

    let mut library_inputs = Vec::new();
    library_inputs.extend(
        builtin_symbols
            .into_iter()
            .map(|(module, symbols)| TemplateLibraryInput::Builtin { module, symbols }),
    );
    library_inputs.extend(
        loadable_symbols
            .into_iter()
            .map(
                |(load_name, (module, symbols))| TemplateLibraryInput::Loadable {
                    load_name,
                    module,
                    symbols,
                },
            ),
    );
    Ok(testing::template_library_catalog(db, library_inputs))
}

type BuiltinSymbolBuckets = Vec<(PythonModuleName, Vec<TemplateSymbol<'static>>)>;
type LoadableLibrarySymbolBuckets =
    BTreeMap<LibraryName, (PythonModuleName, Vec<TemplateSymbol<'static>>)>;

fn builtin_symbol_buckets(builtins: &[String]) -> anyhow::Result<BuiltinSymbolBuckets> {
    builtins
        .iter()
        .map(|module_name| {
            PythonModuleName::parse(module_name)
                .with_context(|| format!("invalid builtin fixture module `{module_name}`"))
                .map(|module| (module, Vec::new()))
        })
        .collect()
}

fn loadable_symbol_buckets(
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
) -> anyhow::Result<LoadableLibrarySymbolBuckets> {
    let mut buckets = BTreeMap::new();
    for (load_name, module_name) in libraries {
        let load_name = LibraryName::parse(load_name)
            .with_context(|| format!("invalid fixture library name `{load_name}`"))?;
        let module = PythonModuleName::parse(module_name)
            .with_context(|| format!("invalid fixture library module `{module_name}`"))?;
        buckets.insert(load_name, (module, Vec::new()));
    }
    Ok(buckets)
}

fn add_fixture_symbol(
    fixture: TemplateSymbolFixture,
    builtin_symbols: &mut BuiltinSymbolBuckets,
    loadable_symbols: &mut LoadableLibrarySymbolBuckets,
) -> anyhow::Result<()> {
    let TemplateSymbolFixture {
        kind,
        name,
        library,
        library_module,
        module,
        doc,
    } = fixture;
    let name = TemplateSymbolName::parse(&name)
        .with_context(|| format!("invalid fixture template symbol name `{name}`"))?;
    let definition = PythonModuleName::parse(&module)
        .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module);
    let symbol = TemplateSymbol {
        kind,
        name,
        definition,
        doc,
    };

    match library {
        TemplateSymbolLibraryFixture::Builtin => {
            add_builtin_symbol(builtin_symbols, &library_module, &symbol)?;
        }
        TemplateSymbolLibraryFixture::Loadable { load_name } => {
            add_loadable_symbol(loadable_symbols, &load_name, &library_module, symbol)?;
        }
    }
    Ok(())
}

fn add_builtin_symbol(
    buckets: &mut BuiltinSymbolBuckets,
    module_name: &str,
    symbol: &TemplateSymbol<'static>,
) -> anyhow::Result<()> {
    let module = PythonModuleName::parse(module_name)
        .with_context(|| format!("invalid builtin fixture module `{module_name}`"))?;
    for (builtin_module, symbols) in buckets.iter_mut() {
        if builtin_module == &module {
            symbols.push(symbol.clone());
        }
    }
    Ok(())
}

fn add_loadable_symbol(
    buckets: &mut LoadableLibrarySymbolBuckets,
    load_name: &str,
    module_name: &str,
    symbol: TemplateSymbol<'static>,
) -> anyhow::Result<()> {
    let load_name = LibraryName::parse(load_name)
        .with_context(|| format!("invalid fixture library name `{load_name}`"))?;
    let module = PythonModuleName::parse(module_name)
        .with_context(|| format!("invalid fixture library module `{module_name}`"))?;
    let entry = buckets
        .entry(load_name)
        .or_insert_with(|| (module.clone(), Vec::new()));
    if entry.0 == module {
        entry.1.push(symbol);
    }
    Ok(())
}

pub struct ProjectFixture {
    root: Utf8PathBuf,
    files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: anyhow::Result<Option<PythonModuleName>>,
    pythonpath: Vec<Utf8PathBuf>,
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
            django_settings_module: Ok(None),
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
    pub fn settings(self, settings: &ProjectSettings) -> Self {
        let path = self.root.join("settings.py");
        self.file(path, settings.settings_py())
            .django_settings_module("settings")
    }

    /// Set the fixture's Django settings module.
    #[must_use]
    pub fn django_settings_module(mut self, module: impl Into<String>) -> Self {
        let module = module.into();
        self.django_settings_module = PythonModuleName::parse(&module)
            .with_context(|| format!("invalid fixture Django settings module `{module}`"))
            .map(Some);
        self
    }

    #[must_use]
    pub fn pythonpath(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.pythonpath.push(path.into());
        self
    }

    #[must_use]
    pub fn tag_specs(mut self, tag_specs: TagSpecDef) -> Self {
        self.tag_specs = tag_specs;
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

    pub fn build(self, db: &TestDatabase) -> anyhow::Result<Project> {
        let django_settings_module = self.django_settings_module?;
        for (path, source) in self.files {
            db.add_file(path.as_str(), &source)
                .with_context(|| format!("failed to add fixture file `{path}`"))?;
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

        Ok(Project::new(
            db,
            self.root,
            search_paths,
            self.interpreter,
            django_settings_module,
            self.pythonpath,
            self.env_vars,
            self.tag_specs,
        ))
    }

    pub fn install(mut self, db: &mut TestDatabase) -> anyhow::Result<Project> {
        // Template-analysis fixtures model an installed Django package so project-scoped builtin
        // meaning is definite rather than supplied by a global fallback. Project-discovery-only
        // fixtures intentionally retain full control over their discovered file inventory.
        let has_templates = self
            .files
            .iter()
            .any(|(path, _)| path.extension() == Some("html"));
        let builtin_files = has_templates.then(|| {
            let django = self.root.join("django");
            let template = django.join("template");
            [
            (django.join("__init__.py"), ""),
            (template.join("__init__.py"), ""),
            (
                template.join("defaulttags.py"),
                "from django import template\nregister = template.Library()\n@register.tag\ndef autoescape(parser, token): pass\n@register.tag\ndef comment(parser, token): pass\n@register.tag\ndef csrf_token(parser, token): pass\n@register.tag\ndef cycle(parser, token): pass\n@register.tag\ndef debug(parser, token): pass\n@register.tag\ndef filter(parser, token): pass\n@register.tag\ndef firstof(parser, token): pass\n@register.tag(name='for')\ndef for_tag(parser, token): pass\n@register.tag(name='if')\ndef if_tag(parser, token): pass\n@register.tag\ndef ifchanged(parser, token): pass\n@register.tag\ndef load(parser, token): pass\n@register.tag\ndef lorem(parser, token): pass\n@register.tag\ndef now(parser, token): pass\n@register.tag\ndef regroup(parser, token): pass\n@register.tag\ndef spaceless(parser, token): pass\n@register.tag\ndef templatetag(parser, token): pass\n@register.tag\ndef url(parser, token): pass\n@register.tag\ndef verbatim(parser, token): pass\n@register.tag\ndef widthratio(parser, token): pass\n@register.tag(name='with')\ndef with_tag(parser, token): pass\n",
            ),
            (
                template.join("loader_tags.py"),
                "from django import template\nregister = template.Library()\n@register.tag\ndef block(parser, token): pass\n@register.tag\ndef extends(parser, token): pass\n@register.tag\ndef include(parser, token): pass\n",
            ),
            ]
        });
        for (path, source) in builtin_files.into_iter().flatten() {
            if !self.files.iter().any(|(candidate, _)| candidate == &path) {
                self.files.push((path, source.to_string()));
            }
        }
        let project = self.build(db)?;
        db.set_project(project);
        Ok(project)
    }
}

#[must_use]
pub fn collect_errors(db: &dyn djls_semantic::Db, file: File) -> Vec<ValidationError> {
    validate_template_file(db, file);

    validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
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
) -> anyhow::Result<Vec<ValidationError>> {
    db.add_file(path, source)?;
    let file = db.create_file_with_revision(Utf8Path::new(path), revision)?;

    Ok(collect_errors(db, file)
        .into_iter()
        .filter(is_argument_validation_error)
        .collect())
}

pub fn extract_and_merge(
    _corpus: &Corpus,
    dir: &Utf8Path,
    specs: &mut TagSpecs,
    arities: &mut FilterAritySpecs,
) -> anyhow::Result<()> {
    let db = TestDatabase::new();

    for file_path in &Corpus::extraction_targets_in(dir) {
        let source = std::fs::read_to_string(file_path.as_std_path())
            .with_context(|| format!("failed to read extraction fixture `{file_path}`"))?;

        let module_name = module_name_from_file(file_path);
        let module_name = PythonModuleName::parse(&module_name)
            .with_context(|| format!("invalid module name derived from `{file_path}`"))?;
        db.add_file(file_path.as_str(), &source)?;
        let file = db.file(file_path)?;
        let bundle = extract_bundle(&db, file, module_name);

        arities.merge_filter_arities(&bundle.filter_arities);
        specs
            .merge_block_specs(&bundle.block_specs)
            .merge_tag_rules(&bundle.tag_rules);
    }
    Ok(())
}

pub fn build_specs_from_extraction(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> anyhow::Result<(TagSpecs, FilterAritySpecs)> {
    let mut specs = builtin_tag_specs();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities)?;
    Ok((specs, arities))
}

pub fn build_entry_specs(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> anyhow::Result<(TagSpecs, FilterAritySpecs)> {
    let mut specs = builtin_tag_specs();
    let mut arities = FilterAritySpecs::new();

    if !Corpus::is_django_entry(entry_dir)
        && let Some(django_dir) = corpus.latest_package("django")
    {
        extract_and_merge(corpus, &django_dir, &mut specs, &mut arities)?;
    }

    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities)?;

    Ok((specs, arities))
}

/// Render validation errors into a plain-text diagnostic snapshot.
pub fn render_diagnostic_snapshot(
    path: &str,
    source: &str,
    errors: &[ValidationError],
) -> anyhow::Result<String> {
    let renderer = DiagnosticRenderer::plain();
    let mut parts = Vec::new();

    for err in errors {
        let span = err
            .primary_span()
            .ok_or_else(|| anyhow::anyhow!("validation error `{err}` has no primary span"))?;
        let message = err.to_string();
        let code = err.code();
        let severity = match DiagnosticsConfig::default().get_severity(code) {
            DiagnosticSeverity::Off => continue,
            DiagnosticSeverity::Error => Severity::Error,
            DiagnosticSeverity::Warning => Severity::Warning,
            DiagnosticSeverity::Info => Severity::Info,
            DiagnosticSeverity::Hint => Severity::Hint,
        };

        let mut diag = Diagnostic::new(source, path, code, &message, severity, span, "");

        if let ValidationError::UnbalancedStructure {
            closing_span: Some(cs),
            ..
        } = err
        {
            diag = diag.annotation(*cs, "", false);
        }

        parts.push(renderer.render(&diag));
    }

    Ok(parts.join("\n"))
}

pub fn snapshot_validate_files<'a>(
    db: &mut OsTestDatabase,
    primary_database_path: &str,
    primary_display_path: &str,
    primary_source: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> anyhow::Result<String> {
    for (path, source) in files {
        db.add_file(path, source)?;
    }

    let file = db.file(Utf8Path::new(primary_database_path))?;
    let mut errors = collect_errors(db, file);
    errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));

    render_diagnostic_snapshot(primary_display_path, primary_source, &errors)
}

/// Validation fixture for mdtest snapshots backed by the pinned Django corpus.
pub fn standard_validation_db() -> anyhow::Result<OsTestDatabase> {
    validation_db(&ProjectSettings::default().settings_py())
}

pub fn validation_db(settings_py: &str) -> anyhow::Result<OsTestDatabase> {
    let corpus = Corpus::require()?;
    let django_source_root = corpus.root().join("repos/django-5.2");
    anyhow::ensure!(
        django_source_root.join("django/__init__.py").is_file(),
        "pinned Django 5.2 corpus source is missing"
    );

    let project_root = Utf8PathBuf::from("/fixture");
    let interpreter = Interpreter::VenvPath(corpus.root().join("hermetic-no-venv"));
    let pythonpath = vec![django_source_root.clone()];
    let search_paths = SearchPaths::from_paths(vec![
        SearchPath::FirstParty(project_root.clone()),
        SearchPath::SitePackages(django_source_root.clone()),
    ]);

    let mut db = OsTestDatabase::with_disk_roots([django_source_root]);
    search_paths.register_roots(&db);
    db.add_file("/fixture/settings.py", settings_py)?;

    let project = Project::new(
        &db,
        project_root,
        search_paths,
        interpreter,
        Some(PythonModuleName::parse("settings")?),
        pythonpath,
        Vec::new(),
        Settings::default().tagspecs().clone(),
    );
    db.set_project(project);
    Ok(db)
}

pub fn render_validate_snapshot(
    db: &mut OsTestDatabase,
    path: &str,
    source: &str,
) -> anyhow::Result<String> {
    let file = db.add_file(path, source)?;
    let mut errors = collect_errors(db, file);
    errors.sort_by_key(|error| error.primary_span().map_or(0, Span::start));
    render_diagnostic_snapshot(path, source, &errors)
}
