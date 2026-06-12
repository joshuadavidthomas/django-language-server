use std::collections::HashMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::Interpreter;
use djls_project::LibraryName;
use djls_project::Project;
use djls_project::PyModuleName;
use djls_project::SearchPaths;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrary;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolName;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Db as _;
use djls_templates::parse_template;

use crate::Corpus;
use crate::TestDatabase;
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

#[derive(serde::Deserialize)]
struct TemplateSymbolFixture {
    kind: TemplateSymbolKind,
    name: String,
    #[serde(default)]
    load_name: Option<String>,
    library_module: String,
    module: String,
    #[serde(default)]
    doc: Option<String>,
}

/// Build template-library facts from JSON fixture rows.
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
    let mut result = TemplateLibraries {
        knowledge: djls_project::StaticKnowledge::Known,
        ..TemplateLibraries::default()
    };

    for module_name in builtins {
        let Ok(module) = PyModuleName::parse(module_name) else {
            continue;
        };
        if !result
            .builtins
            .iter()
            .any(|library| library.module() == &module)
        {
            result.builtins.push(TemplateLibrary::new(module));
        }
    }

    for (load_name, module_name) in libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            continue;
        };
        let Ok(module) = PyModuleName::parse(module_name) else {
            continue;
        };
        result
            .loadable
            .insert(load_name, TemplateLibrary::new(module));
    }

    let symbols = tags.iter().chain(filters.iter()).cloned();
    for fixture in symbols
        .map(serde_json::from_value)
        .collect::<Result<Vec<TemplateSymbolFixture>, _>>()
        .unwrap()
    {
        let Ok(name) = TemplateSymbolName::parse(&fixture.name) else {
            continue;
        };
        let definition = PyModuleName::parse(&fixture.module)
            .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module);
        let symbol = TemplateSymbol {
            kind: fixture.kind,
            name,
            definition,
            doc: fixture.doc,
        };

        match fixture.load_name {
            None => {
                let Ok(module) = PyModuleName::parse(&fixture.library_module) else {
                    continue;
                };
                if let Some(library) = result
                    .builtins
                    .iter_mut()
                    .find(|library| library.module() == &module)
                {
                    library.merge_symbol(symbol);
                }
            }
            Some(load_name) => {
                let Ok(load_name) = LibraryName::parse(&load_name) else {
                    continue;
                };
                let Ok(module) = PyModuleName::parse(&fixture.library_module) else {
                    continue;
                };
                let library = result
                    .loadable
                    .entry(load_name)
                    .or_insert_with(|| TemplateLibrary::new(module.clone()));
                if library.module() == &module {
                    library.merge_symbol(symbol);
                }
            }
        }
    }

    result
}

pub fn make_template_libraries_tags_only(
    tags: &[serde_json::Value],
    libraries: &HashMap<String, String, impl std::hash::BuildHasher>,
    builtins: &[String],
) -> TemplateLibraries {
    make_template_libraries(tags, &[], libraries, builtins)
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
    pub fn pythonpath(mut self, path: impl Into<String>) -> Self {
        self.pythonpath.push(path.into());
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
        let result = djls_project::extract_rules(&source, &module_path);
        arities.merge_extraction_result(&result);
        specs.merge_extraction_results(&result);
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
pub fn build_entry_specs(
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

