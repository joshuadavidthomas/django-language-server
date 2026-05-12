use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::parse_template;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

use crate::specs::tags::builtin_tag_specs;
use crate::ArgumentCountConstraint;
use crate::ChoiceAt;
use crate::ExtractedDiagnosticConstraint;
use crate::ExtractedDiagnosticMessage;
use crate::ExtractedMessageTemplate;
use crate::FilterArity;
use crate::FilterAritySpecs;
use crate::Knowledge;
use crate::LibraryName;
use crate::LibraryOrigin;
use crate::PyModuleName;
use crate::RequiredKeyword;
use crate::SplitPosition;
use crate::SymbolDefinition;
use crate::SymbolKey;
use crate::TagIndex;
use crate::TagRule;
use crate::TagSpec;
use crate::TagSpecs;
use crate::TemplateLibraries;
use crate::TemplateLibrary;
use crate::TemplateLibrarySnapshot;
use crate::TemplateSymbol;
use crate::TemplateSymbolKind;
use crate::TemplateSymbolName;
use crate::TemplateSymbolSnapshot;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

mod mdtest;

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
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
    template_libraries: TemplateLibraries,
}

impl TestDatabase {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            tag_specs: builtin_tag_specs(),
            filter_arity_specs: FilterAritySpecs::new(),
            template_libraries: TemplateLibraries::default(),
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

    #[must_use]
    pub(crate) fn create_file(&self, path: &Utf8Path) -> File {
        <Self as djls_source::Db>::create_file(self, path)
    }

    #[must_use]
    pub(crate) fn create_file_with_revision(&self, path: &Utf8Path, revision: u64) -> File {
        File::new(self, path.to_owned(), revision)
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl djls_source::Db for TestDatabase {
    fn create_file(&self, path: &Utf8Path) -> File {
        File::new(self, path.to_owned(), 0)
    }

    fn get_file(&self, _path: &Utf8Path) -> Option<File> {
        None
    }

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.fs.lock().unwrap().read_to_string(path)
    }
}

#[salsa::db]
impl djls_templates::Db for TestDatabase {}

#[salsa::db]
impl crate::Db for TestDatabase {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs
    }

    fn tag_index(&self) -> TagIndex<'_> {
        TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        None
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

    fn model_graph(&self) -> &crate::ModelGraph {
        crate::ModelGraph::empty_ref()
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

    if !corpus.is_django_entry(entry_dir) {
        if let Some(django_dir) = corpus.latest_package("django") {
            extract_and_merge(corpus, &django_dir, &mut specs, &mut arities);
        }
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

        // Build notes for variants that carry extra context beyond the
        // primary message. When adding new ValidationError variants, consider
        // whether they have fields that would be useful as notes here.
        let mut notes: Vec<String> = Vec::new();
        match err {
            ValidationError::TagNotInInstalledApps { load_name, .. }
            | ValidationError::FilterNotInInstalledApps { load_name, .. } => {
                notes.push(format!("load_name: {load_name}"));
            }
            ValidationError::LibraryNotInInstalledApps { candidates, .. }
                if !candidates.is_empty() =>
            {
                notes.push(format!("candidates: {candidates:?}"));
            }
            ValidationError::UnclosedTag { .. }
            | ValidationError::OrphanedTag { .. }
            | ValidationError::OrphanedClosingTag { .. }
            | ValidationError::UnbalancedStructure { .. }
            | ValidationError::UnmatchedBlockName { .. }
            | ValidationError::UnknownTag { .. }
            | ValidationError::UnloadedTag { .. }
            | ValidationError::AmbiguousUnloadedTag { .. }
            | ValidationError::UnknownFilter { .. }
            | ValidationError::UnloadedFilter { .. }
            | ValidationError::AmbiguousUnloadedFilter { .. }
            | ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
            | ValidationError::UnknownLibrary { .. }
            | ValidationError::LibraryNotInInstalledApps { .. }
            | ValidationError::ExtendsMustBeFirst { .. }
            | ValidationError::MultipleExtends { .. } => {}
        }

        let mut diag = Diagnostic::new(source, path, code, &message, Severity::Error, span, "");

        if let ValidationError::UnbalancedStructure {
            closing_span: Some(cs),
            ..
        } = err
        {
            diag = diag.annotation(*cs, "", false);
        }

        for note in &notes {
            diag = diag.note(note);
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
        TagSpec {
            module: "example.templatetags.custom".into(),
            end_tag: None,
            intermediate_tags: Cow::Borrowed(&[]),
            opaque: false,
            extracted_rules: Some(TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
                ..TagRule::default()
            }),
        },
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
        spec.extracted_rules = Some(rule);
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

    let mut template_libraries = make_template_libraries(&tags, &filters, &libraries, &builtins);
    add_discovered_widgets_library(&mut template_libraries);
    template_libraries
}

fn add_discovered_widgets_library(template_libraries: &mut TemplateLibraries) {
    template_libraries.discovery_knowledge = Knowledge::Known;

    let name = LibraryName::parse("widgets").unwrap();
    let app = PyModuleName::parse("example.widgets").unwrap();
    let module = PyModuleName::parse("example.widgets.templatetags.widgets").unwrap();
    let origin = LibraryOrigin {
        app,
        module: module.clone(),
        path: Utf8PathBuf::from("/example/widgets/templatetags/widgets.py"),
    };
    let mut library = TemplateLibrary::new_discovered(name.clone(), origin);
    library.merge_symbol(template_symbol(
        TemplateSymbolKind::Tag,
        "widget_tag",
        "example.widgets.templatetags.widgets",
    ));
    library.merge_symbol(template_symbol(
        TemplateSymbolKind::Filter,
        "widget_filter",
        "example.widgets.templatetags.widgets",
    ));
    template_libraries
        .loadable
        .entry(name)
        .or_default()
        .push(library);
}

fn template_symbol(kind: TemplateSymbolKind, name: &str, module: &str) -> TemplateSymbol {
    TemplateSymbol {
        kind,
        name: TemplateSymbolName::parse(name).unwrap(),
        definition: SymbolDefinition::Module(PyModuleName::parse(module).unwrap()),
        doc: None,
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
