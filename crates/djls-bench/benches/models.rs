use std::io;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::FixtureLoadError;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::corpus_or_skip;
use djls_bench::model_benchmark_module_name;
use djls_bench::model_fixtures;
use djls_bench::require;
use djls_bench::require_some;
use djls_conf::Settings;
use djls_project::InvalidModuleName;
use djls_project::ModelGraph;
use djls_project::ModelId;
use djls_project::Project;
use djls_project::PythonModuleName;
use djls_project::testing::extract_model_graph;
use djls_project::testing::resolve_model_graph_from_modules;
use djls_source::File;
use djls_source::FileError;

fn main() {
    divan::main();
}

#[derive(Debug, thiserror::Error)]
enum ModelSetupError {
    #[error(transparent)]
    Fixtures(#[from] FixtureLoadError),
    #[error("invalid model benchmark module name {value:?}: {source}")]
    Module {
        value: String,
        #[source]
        source: InvalidModuleName,
    },
    #[error("failed to register model benchmark source {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
    #[error("model benchmark fixture {label:?} is missing")]
    MissingFixture { label: &'static str },
    #[error("resolved model graph is missing {module}.{name}")]
    MissingModel {
        module: &'static str,
        name: &'static str,
    },
    #[error("resolved model graph has no forward relation {model}.{field}")]
    MissingForwardRelation {
        model: &'static str,
        field: &'static str,
    },
    #[error("resolved model graph has no reverse relation {model}.{relation}")]
    MissingReverseRelation {
        model: &'static str,
        relation: &'static str,
    },
    #[error("benchmark corpus is invalid or stale: {message}")]
    InvalidCorpus { message: String },
    #[error("Django package is missing from the synchronized benchmark corpus at {root}")]
    MissingDjangoPackage { root: Utf8PathBuf },
    #[error("failed to read corpus model source {path}: {source}")]
    ReadCorpusModel {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no model files were found in the synchronized {selection} corpus")]
    EmptyCorpus { selection: &'static str },
}

fn parse_module(value: &str) -> Result<PythonModuleName, ModelSetupError> {
    PythonModuleName::parse(value).map_err(|source| ModelSetupError::Module {
        value: value.to_string(),
        source,
    })
}

fn register_model_source(
    db: &mut Db,
    path: Utf8PathBuf,
    source: &str,
) -> Result<File, ModelSetupError> {
    db.file_with_contents(path.clone(), source)
        .map_err(|source| ModelSetupError::Register { path, source })
}

struct ModelExtractionInput {
    db: Db,
    files: Vec<(File, PythonModuleName)>,
}

fn fixture_extraction_input(prefix: &str) -> Result<ModelExtractionInput, ModelSetupError> {
    let fixtures = model_fixtures()?;
    let mut db = Db::new();
    let mut files = Vec::with_capacity(fixtures.len());
    for (index, fixture) in fixtures.iter().enumerate() {
        let path = Utf8PathBuf::from(format!("/bench/models/{prefix}/{index}.py"));
        let file = register_model_source(&mut db, path, &fixture.source)?;
        let module = parse_module(&format!("bench.models.fixture_{index}"))?;
        files.push((file, module));
    }
    Ok(ModelExtractionInput { db, files })
}

// Batch extraction: all fixtures in one iteration

#[divan::bench]
fn extract(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            require(
                "prepare model extraction benchmark input",
                fixture_extraction_input("extract"),
            )
        })
        .bench_local_values(|input| {
            let mut extracted_models = 0;
            for (file, module) in input.files {
                let graph = extract_model_graph(&input.db, file, module);
                extracted_models += graph.len();
                divan::black_box(graph);
            }
            divan::black_box(extracted_models);
        });
}

// Project assembly: reuse cached per-file extraction and rebuild inheritance.

#[divan::bench]
fn assemble(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            require(
                "prepare model assembly benchmark input",
                fixture_extraction_input("assemble"),
            )
        })
        .bench_local_values(|input| {
            let project =
                Project::initial(&input.db, Utf8Path::new("/bench"), &Settings::default());
            let mut assembled_models = 0;
            for _ in 0..REPEATED_INNER_ITERS {
                let graph = resolve_model_graph_from_modules(
                    &input.db,
                    project,
                    input.files.iter().cloned(),
                );
                assembled_models += graph.len();
                divan::black_box(graph);
            }
            divan::black_box(assembled_models);
        });
}

// Resolution: forward and field-based relation lookups on a populated graph

fn build_auth_graph() -> Result<ModelGraph, ModelSetupError> {
    let fixtures = model_fixtures()?;
    let auth = fixtures
        .iter()
        .find(|fixture| fixture.label == "medium_auth.py")
        .ok_or(ModelSetupError::MissingFixture {
            label: "medium_auth.py",
        })?;
    let mut db = Db::new();
    let project = Project::initial(&db, Utf8Path::new("/bench"), &Settings::default());
    let auth_file = register_model_source(
        &mut db,
        Utf8PathBuf::from("/bench/django/contrib/auth/models.py"),
        &auth.source,
    )?;
    let content_type_file = register_model_source(
        &mut db,
        Utf8PathBuf::from("/bench/django/contrib/contenttypes/models.py"),
        "from django.db import models\n\nclass ContentType(models.Model):\n    pass\n",
    )?;

    Ok(resolve_model_graph_from_modules(
        &db,
        project,
        [
            (auth_file, parse_module("django.contrib.auth.models")?),
            (
                content_type_file,
                parse_module("django.contrib.contenttypes.models")?,
            ),
        ],
    ))
}

fn auth_graph() -> Result<&'static ModelGraph, &'static ModelSetupError> {
    static GRAPH: OnceLock<Result<ModelGraph, ModelSetupError>> = OnceLock::new();
    match GRAPH.get_or_init(build_auth_graph) {
        Ok(graph) => Ok(graph),
        Err(error) => Err(error),
    }
}

fn model_id<'a>(
    graph: &'a ModelGraph,
    name: &'static str,
    module: &'static str,
) -> Result<&'a ModelId, ModelSetupError> {
    let id = graph
        .models_named(name)
        .map(|(id, _model)| id)
        .find(|id| id.module_name().as_str() == module)
        .ok_or(ModelSetupError::MissingModel { module, name })?;
    if graph.get_by_id(id).is_none() {
        return Err(ModelSetupError::MissingModel { module, name });
    }
    Ok(id)
}

#[divan::bench]
fn resolve_relations(bencher: Bencher) {
    let graph = require("prepare auth model graph", auth_graph());
    let permission = require(
        "find Permission in auth model graph",
        model_id(graph, "Permission", "django.contrib.auth.models"),
    );
    let group = require(
        "find Group in auth model graph",
        model_id(graph, "Group", "django.contrib.auth.models"),
    );
    let lookup_queries = [("auth", "Permission"), ("auth", "Group"), ("auth", "User")];
    let forward_queries = [(permission, "content_type"), (group, "permissions")];
    let relation_queries = [(permission, "group_set")];

    for &(model, field) in &forward_queries {
        require_some(
            ModelSetupError::MissingForwardRelation {
                model: model.name(),
                field,
            },
            graph.resolve_forward(model, field),
        );
    }
    for &(model, relation) in &relation_queries {
        require_some(
            ModelSetupError::MissingReverseRelation {
                model: model.name(),
                relation,
            },
            graph.resolve_relation(model, relation),
        );
    }

    bencher.bench_local(|| {
        let mut resolved = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            for (app_label, name) in lookup_queries {
                resolved += usize::from(graph.lookup(app_label, name).is_some());
            }
            for (model, field) in forward_queries {
                resolved += usize::from(graph.resolve_forward(model, field).is_some());
            }
            for (model, relation) in relation_queries {
                resolved += usize::from(graph.resolve_relation(model, relation).is_some());
            }
        }
        divan::black_box(resolved);
    });
}

// Corpus-scale: extract all models.py from Django, then from the full corpus

struct CorpusModels {
    files: Vec<(String, PythonModuleName)>,
}

fn load_corpus_models_inner(
    selection: &'static str,
    get_paths: impl FnOnce(&djls_testing::Corpus) -> Result<Vec<Utf8PathBuf>, ModelSetupError>,
) -> Result<Option<CorpusModels>, ModelSetupError> {
    if !djls_testing::Corpus::is_available() {
        return Ok(None);
    }

    let corpus =
        djls_testing::Corpus::require().map_err(|error| ModelSetupError::InvalidCorpus {
            message: error.to_string(),
        })?;
    let mut paths = get_paths(&corpus)?;
    paths.sort();
    if paths.is_empty() {
        return Err(ModelSetupError::EmptyCorpus { selection });
    }

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let source = std::fs::read_to_string(path.as_std_path()).map_err(|source| {
            ModelSetupError::ReadCorpusModel {
                path: path.clone(),
                source,
            }
        })?;
        let module_name = model_benchmark_module_name(&path);
        let module_name = parse_module(&module_name)?;
        files.push((source, module_name));
    }

    Ok(Some(CorpusModels { files }))
}

fn load_django_models() -> Result<Option<&'static CorpusModels>, &'static ModelSetupError> {
    static CORPUS: OnceLock<Result<Option<CorpusModels>, ModelSetupError>> = OnceLock::new();
    match CORPUS.get_or_init(|| {
        load_corpus_models_inner("Django", |corpus| {
            let django_dir = corpus.latest_package("django").ok_or_else(|| {
                ModelSetupError::MissingDjangoPackage {
                    root: corpus.root().to_path_buf(),
                }
            })?;
            Ok(corpus.model_files_in(&django_dir))
        })
    }) {
        Ok(corpus) => Ok(corpus.as_ref()),
        Err(error) => Err(error),
    }
}

fn load_all_corpus_models() -> Result<Option<&'static CorpusModels>, &'static ModelSetupError> {
    static CORPUS: OnceLock<Result<Option<CorpusModels>, ModelSetupError>> = OnceLock::new();
    match CORPUS.get_or_init(|| {
        load_corpus_models_inner("full", |corpus| Ok(corpus.model_files_in(corpus.root())))
    }) {
        Ok(corpus) => Ok(corpus.as_ref()),
        Err(error) => Err(error),
    }
}

fn corpus_extraction_input(corpus: &CorpusModels) -> Result<ModelExtractionInput, ModelSetupError> {
    let mut db = Db::new();
    let mut files = Vec::with_capacity(corpus.files.len());
    for (index, (source, module_name)) in corpus.files.iter().enumerate() {
        let path = Utf8PathBuf::from(format!("/bench/models/corpus/{index}.py"));
        let file = register_model_source(&mut db, path, source)?;
        files.push((file, module_name.clone()));
    }
    Ok(ModelExtractionInput { db, files })
}

fn bench_corpus(
    bencher: Bencher,
    corpus: Result<Option<&'static CorpusModels>, &'static ModelSetupError>,
    selection: &'static str,
) {
    let Some(corpus) = corpus_or_skip(format_args!("{selection} model benchmark"), corpus) else {
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .with_inputs(move || {
            require(
                format_args!("register {selection} corpus model inputs"),
                corpus_extraction_input(corpus),
            )
        })
        .bench_local_values(|input| {
            let project =
                Project::initial(&input.db, Utf8Path::new("/bench"), &Settings::default());
            let graph = resolve_model_graph_from_modules(&input.db, project, input.files);
            divan::black_box(graph);
        });
}

// Typed inheritance resolves the corpus as one project; these are assembly
// workloads rather than the former per-file extraction-and-merge workloads.
#[divan::bench]
fn corpus_django_project_assembly(bencher: Bencher) {
    bench_corpus(bencher, load_django_models(), "Django");
}

#[divan::bench]
fn corpus_all_project_assembly(bencher: Bencher) {
    bench_corpus(bencher, load_all_corpus_models(), "full");
}
