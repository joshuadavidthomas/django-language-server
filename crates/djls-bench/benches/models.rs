use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::model_fixtures;
use djls_project::ModelGraph;
use djls_project::ModelId;
use djls_project::PythonModuleName;
use djls_project::testing::extract_model_graph;

fn main() {
    divan::main();
}

fn module_name(path: &str) -> PythonModuleName {
    PythonModuleName::parse(path).unwrap()
}

fn model_graph_from_source(
    db: &mut Db,
    path: impl Into<Utf8PathBuf>,
    source: &str,
    module_name: PythonModuleName,
) -> ModelGraph {
    let file = db.file_with_contents(path, source);
    extract_model_graph(db, file, module_name).clone()
}

// Batch extraction: all fixtures in one iteration

#[divan::bench]
fn extract(bencher: Bencher) {
    let fixtures = model_fixtures();
    bencher.bench_local(move || {
        let mut db = Db::new();
        for (index, fixture) in fixtures.iter().enumerate() {
            divan::black_box(model_graph_from_source(
                &mut db,
                format!("/bench/models/extract/{index}.py"),
                &fixture.source,
                module_name("bench.models"),
            ));
        }
    });
}

// Merge: extract graphs then merge them (the hot path in compute_model_graph)

#[divan::bench]
fn merge(bencher: Bencher) {
    let fixtures = model_fixtures();
    let mut db = Db::new();
    let graphs: Vec<ModelGraph> = fixtures
        .iter()
        .enumerate()
        .map(|(index, fixture)| {
            model_graph_from_source(
                &mut db,
                format!("/bench/models/merge/{index}.py"),
                &fixture.source,
                module_name("bench.models"),
            )
        })
        .collect();

    bencher.bench_local(move || {
        let mut merged_models = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            let mut merged = ModelGraph::new();
            for graph in &graphs {
                merged.merge(graph.clone());
            }
            merged_models += merged.len();
        }
        divan::black_box(merged_models);
    });
}

// Resolution: forward and field-based relation lookups on a populated graph

fn auth_graph() -> &'static ModelGraph {
    static GRAPH: OnceLock<ModelGraph> = OnceLock::new();
    GRAPH.get_or_init(|| {
        let fixtures = model_fixtures();
        let auth = fixtures
            .iter()
            .find(|f| f.label == "medium_auth.py")
            .expect("medium_auth fixture missing");
        let mut db = Db::new();
        model_graph_from_source(
            &mut db,
            "/bench/models/auth.py",
            &auth.source,
            module_name("django.contrib.auth.models"),
        )
    })
}

fn model_id<'a>(graph: &'a ModelGraph, name: &'a str, module_name: &str) -> &'a ModelId {
    let (id, _model) = graph.models_named(name).next().expect("model should exist");
    assert_eq!(id.name(), name);
    assert_eq!(id.module_name().as_str(), module_name);
    assert!(graph.get_by_id(id).is_some());
    id
}

#[divan::bench]
fn resolve_relations(bencher: Bencher) {
    let graph = auth_graph();
    let permission = model_id(graph, "Permission", "django.contrib.auth.models");
    let group = model_id(graph, "Group", "django.contrib.auth.models");
    let user = model_id(graph, "User", "django.contrib.auth.models");

    let lookup_queries = [("auth", "Permission"), ("auth", "Group"), ("auth", "User")];
    let forward_queries = [
        (permission, "content_type"),
        (group, "permissions"),
        (user, "groups"),
    ];
    let relation_queries = [
        (group, "user_set"),
        (permission, "group_set"),
        (permission, "user_set"),
    ];

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
    get_paths: impl FnOnce(&djls_testing::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusModels> {
    if !djls_testing::Corpus::is_available() {
        return None;
    }

    let corpus = djls_testing::Corpus::require();
    let mut paths = get_paths(&corpus)?;
    paths.sort();

    let files: Vec<(String, PythonModuleName)> = paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            let module_name = djls_testing::module_name_from_file(&path);
            let module_name = PythonModuleName::parse(&module_name).ok()?;
            Some((source, module_name))
        })
        .collect();

    if files.is_empty() {
        return None;
    }

    Some(CorpusModels { files })
}

fn load_django_models() -> Option<&'static CorpusModels> {
    static CORPUS: OnceLock<Option<CorpusModels>> = OnceLock::new();
    CORPUS
        .get_or_init(|| {
            load_corpus_models_inner(|corpus| {
                let django_dir = corpus.latest_package("django")?;
                Some(corpus.model_files_in(&django_dir))
            })
        })
        .as_ref()
}

fn load_all_corpus_models() -> Option<&'static CorpusModels> {
    static CORPUS: OnceLock<Option<CorpusModels>> = OnceLock::new();
    CORPUS
        .get_or_init(|| {
            load_corpus_models_inner(|corpus| Some(corpus.model_files_in(corpus.root())))
        })
        .as_ref()
}

fn bench_corpus(bencher: Bencher, corpus: Option<&'static CorpusModels>) {
    let Some(corpus) = corpus else {
        assert!(
            std::env::var_os("CI").is_none(),
            "corpus not synced; run `just corpus sync` before benchmarks",
        );
        eprintln!("corpus not synced, skipping");
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .bench_local(move || {
            let mut db = Db::new();
            let mut merged = ModelGraph::new();
            for (index, (source, module_name)) in corpus.files.iter().enumerate() {
                let graph = model_graph_from_source(
                    &mut db,
                    format!("/bench/models/corpus/{index}.py"),
                    source,
                    module_name.clone(),
                );
                merged.merge(graph);
            }
            divan::black_box(merged);
        });
}

#[divan::bench]
fn corpus_django(bencher: Bencher) {
    bench_corpus(bencher, load_django_models());
}

#[divan::bench]
fn corpus_all(bencher: Bencher) {
    bench_corpus(bencher, load_all_corpus_models());
}
