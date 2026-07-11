use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::ArgumentCountConstraint;
use djls_project::ChoiceAt;
use djls_project::Db as ProjectDb;
use djls_project::ExtractedDiagnosticConstraint;
use djls_project::ExtractedDiagnosticMessage;
use djls_project::ExtractedMessageTemplate;
use djls_project::FilterArity;
use djls_project::Interpreter;
use djls_project::LibraryName;
use djls_project::Project;
use djls_project::PythonModuleName;
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
use djls_project::testing::TemplateLibraryInput;
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

use crate::Corpus;
use crate::TestDatabase;
use crate::extract_bundle;
use crate::module_name_from_file;

#[must_use]
pub fn builtin_tag(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
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
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "library_kind": "installed",
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
        "library_kind": "builtin",
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
        "library_kind": "installed",
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
    Installed { load_name: String },
}

/// Build Template Library facts from JSON fixture rows.
///
/// # Panics
///
/// Panics if a fixture row does not match the expected `TemplateSymbolFixture` shape.
pub fn make_template_libraries(
    db: &dyn ProjectDb,
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> TemplateLibraries {
    let mut builtin_symbols = builtin_symbol_buckets(builtins);
    let mut installed_symbols = installed_symbol_buckets(libraries);

    for fixture in tags
        .iter()
        .chain(filters.iter())
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<TemplateSymbolFixture>, _>>()
        .unwrap()
    {
        add_fixture_symbol(fixture, &mut builtin_symbols, &mut installed_symbols);
    }

    let mut library_inputs = Vec::new();
    library_inputs.extend(
        builtin_symbols
            .into_iter()
            .map(|(module, symbols)| TemplateLibraryInput::Builtin { module, symbols }),
    );
    library_inputs.extend(
        installed_symbols
            .into_iter()
            .map(
                |(load_name, (module, symbols))| TemplateLibraryInput::Installed {
                    load_name,
                    module,
                    symbols,
                },
            ),
    );
    testing::template_libraries(db, library_inputs)
}

type BuiltinSymbolBuckets = Vec<(PythonModuleName, Vec<TemplateSymbol>)>;
type InstalledLibrarySymbolBuckets = BTreeMap<LibraryName, (PythonModuleName, Vec<TemplateSymbol>)>;

fn builtin_symbol_buckets(builtins: &[String]) -> BuiltinSymbolBuckets {
    builtins
        .iter()
        .filter_map(|module_name| PythonModuleName::parse(module_name).ok())
        .map(|module| (module, Vec::new()))
        .collect()
}

fn installed_symbol_buckets(
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
) -> InstalledLibrarySymbolBuckets {
    let mut buckets = BTreeMap::new();
    for (load_name, module_name) in libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            continue;
        };
        let Ok(module) = PythonModuleName::parse(module_name) else {
            continue;
        };
        buckets.insert(load_name, (module, Vec::new()));
    }
    buckets
}

fn add_fixture_symbol(
    fixture: TemplateSymbolFixture,
    builtin_symbols: &mut BuiltinSymbolBuckets,
    installed_symbols: &mut InstalledLibrarySymbolBuckets,
) {
    let TemplateSymbolFixture {
        kind,
        name,
        library,
        library_module,
        module,
        doc,
    } = fixture;
    let Ok(name) = TemplateSymbolName::parse(&name) else {
        return;
    };
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
            add_builtin_symbol(builtin_symbols, &library_module, &symbol);
        }
        TemplateSymbolLibraryFixture::Installed { load_name } => {
            add_installed_symbol(installed_symbols, &load_name, &library_module, symbol);
        }
    }
}

fn add_builtin_symbol(
    buckets: &mut BuiltinSymbolBuckets,
    module_name: &str,
    symbol: &TemplateSymbol,
) {
    let Ok(module) = PythonModuleName::parse(module_name) else {
        return;
    };
    for (builtin_module, symbols) in buckets.iter_mut() {
        if builtin_module == &module {
            symbols.push(symbol.clone());
        }
    }
}

fn add_installed_symbol(
    buckets: &mut InstalledLibrarySymbolBuckets,
    load_name: &str,
    module_name: &str,
    symbol: TemplateSymbol,
) {
    let Ok(load_name) = LibraryName::parse(load_name) else {
        return;
    };
    let Ok(module) = PythonModuleName::parse(module_name) else {
        return;
    };
    let entry = buckets
        .entry(load_name)
        .or_insert_with(|| (module.clone(), Vec::new()));
    if entry.0 == module {
        entry.1.push(symbol);
    }
}

pub struct ProjectFixture {
    root: Utf8PathBuf,
    files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: Option<PythonModuleName>,
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

    /// Set the fixture's Django settings module.
    ///
    /// # Panics
    ///
    /// Panics if `module` is not a valid Python module name.
    #[must_use]
    pub fn django_settings_module(mut self, module: impl Into<String>) -> Self {
        let module = module.into();
        self.django_settings_module = Some(
            PythonModuleName::parse(&module)
                .expect("fixture Django settings module should be a valid Python module name"),
        );
        self
    }

    #[must_use]
    fn tag_specs(mut self, tag_specs: TagSpecDef) -> Self {
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

    pub fn install(mut self, db: &mut TestDatabase) -> Project {
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

    djls_semantic::validate_template_file(db, file);

    djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
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

    djls_semantic::validate_template_file(db, file);

    djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file)
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

        let module_name = module_name_from_file(file_path);
        let Ok(module_name) = PythonModuleName::parse(&module_name) else {
            continue;
        };
        db.add_file(file_path.as_str(), &source);
        let file = db.file(file_path);
        let bundle = extract_bundle(&db, file, module_name);

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
    let mut specs = builtin_tag_specs();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

#[must_use]
pub fn build_entry_specs(corpus: &Corpus, entry_dir: &Utf8Path) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = builtin_tag_specs();
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
pub fn snapshot_validate_files<'a>(
    primary_path: &str,
    primary_source: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let db = standard_validation_db();
    for (path, source) in files {
        db.add_file(path, source);
    }

    let file = db.create_file_with_revision(Utf8Path::new(primary_path), 0);

    djls_semantic::validate_template_file(&db, file);

    let mut errors: Vec<ValidationError> =
        djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(&db, file)
            .into_iter()
            .map(|acc| acc.0.clone())
            .collect();

    errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));

    render_diagnostic_snapshot(primary_path, primary_source, &errors)
}

/// Curated validation environment for mdtest snapshots.
///
/// This keeps diagnostic snapshots deterministic and easy to author. It is not
/// a live Django project inspection fixture; add libraries, tags, and filters
/// here when a scenario needs them.
#[must_use]
pub fn standard_validation_db() -> TestDatabase {
    validation_db(false)
}

#[must_use]
pub fn partial_validation_db() -> TestDatabase {
    validation_db(true)
}

#[allow(clippy::too_many_lines)]
fn validation_db(partial: bool) -> TestDatabase {
    let specs = standard_tag_specs();
    let configured_tags = specs
        .keys()
        .map(|name| serde_json::json!({"name": name, "type": "standalone"}))
        .collect::<Vec<_>>();
    let configured_fallback = serde_json::from_value(serde_json::json!({
        "libraries": [{"module": "djls.testing.fallback", "tags": configured_tags}]
    }))
    .expect("validation fallback tag specs should deserialize");
    let mut db = TestDatabase::new()
        .with_specs(specs)
        .with_arity_specs(standard_filter_arities());
    let open_key = if partial {
        ", UNKNOWN: 'maybe'"
    } else {
        Default::default()
    };
    let settings = format!(
        "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/'], 'APP_DIRS': False, 'OPTIONS': {{'builtins': ['example.templatetags.custom'], 'libraries': {{'alpha': 'example.alpha.templatetags.alpha', 'beta': 'example.beta.templatetags.beta', 'cache': 'django.templatetags.cache', 'humanize': 'django.contrib.humanize.templatetags.humanize', 'i18n': 'django.templatetags.i18n', 'l10n': 'django.templatetags.l10n', 'static': 'django.templatetags.static', 'tz': 'django.templatetags.tz'}}}}{open_key}}}]\n"
    );
    let register = "from django import template\nregister = template.Library()\n";
    let tags = |names: &[&str]| {
        let mut source = register.to_string();
        for (index, name) in names.iter().enumerate() {
            writeln!(
                source,
                "@register.tag(name='{name}')\ndef tag_{index}(parser, token): pass"
            )
            .unwrap();
        }
        source
    };
    let filters = |names: &[&str]| {
        let mut source = register.to_string();
        for name in names {
            writeln!(
                source,
                "@register.filter\ndef {name}(value, arg=None): pass"
            )
            .unwrap();
        }
        source
    };

    ProjectFixture::new("/")
        .django_settings_module("project.settings")
        .tag_specs(configured_fallback)
        .file("/project/settings.py", settings)
        .file("/project/__init__.py", "")
        .file("/django/__init__.py", "")
        .file("/django/template/__init__.py", "")
        .file("/django/templatetags/__init__.py", "")
        .file("/django/contrib/__init__.py", "")
        .file("/django/contrib/humanize/__init__.py", "")
        .file("/django/contrib/humanize/templatetags/__init__.py", "")
        .file("/example/__init__.py", "")
        .file("/example/templatetags/__init__.py", "")
        .file("/example/alpha/__init__.py", "")
        .file("/example/alpha/templatetags/__init__.py", "")
        .file("/example/beta/__init__.py", "")
        .file("/example/beta/templatetags/__init__.py", "")
        .file(
            "/django/template/defaulttags.py",
            tags(&[
                "autoescape", "comment", "csrf_token", "cycle", "debug", "filter",
                "firstof", "for", "if", "ifchanged", "load", "lorem", "now", "regroup",
                "spaceless", "templatetag", "url", "verbatim", "widthratio", "with",
            ]),
        )
        .file(
            "/django/template/defaultfilters.py",
            format!(
                "{register}@register.filter\ndef title(value): pass\n@register.filter\ndef lower(value): pass\n@register.filter\ndef length(value): pass\n@register.filter\ndef default(value, arg): pass\n@register.filter\ndef truncatewords(value, arg): pass\n@register.filter\ndef date(value, arg=None): pass\n@register.filter\ndef upper(value): pass\n"
            ),
        )
        .file(
            "/django/template/loader_tags.py",
            tags(&["block", "extends", "include"]),
        )
        .file(
            "/example/templatetags/custom.py",
            format!("{register}@register.simple_tag\ndef one_arg_tag(value): pass\n"),
        )
        .file(
            "/example/alpha/templatetags/alpha.py",
            format!(
                "{}{}",
                tags(&["ambiguous_tag", "shared"]),
                filters(&["ambiguous_filter", "shared_filter"])
            ),
        )
        .file(
            "/example/beta/templatetags/beta.py",
            format!(
                "{}{}",
                tags(&["ambiguous_tag", "shared"]),
                filters(&["ambiguous_filter", "shared_filter"])
            ),
        )
        .file("/django/templatetags/cache.py", tags(&["cache"]))
        .file(
            "/django/templatetags/i18n.py",
            format!(
                "{}{}",
                tags(&["blocktrans", "blocktranslate", "trans", "translate"]),
                filters(&["trans"])
            ),
        )
        .file("/django/templatetags/l10n.py", tags(&["localize"]))
        .file("/django/templatetags/static.py", tags(&["static"]))
        .file("/django/templatetags/tz.py", tags(&["localtime", "timezone"]))
        .file(
            "/django/contrib/humanize/templatetags/humanize.py",
            filters(&["intcomma"]),
        )
        .install(&mut db);
    db
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

fn default_filters_module() -> &'static str {
    "django.template.defaultfilters"
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
