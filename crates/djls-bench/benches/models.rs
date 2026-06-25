use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::model_fixtures;
use djls_project::ModelGraph;
use djls_project::PythonModulePath;

fn main() {
    divan::main();
}

fn module_path(path: &str) -> PythonModulePath {
    PythonModulePath::parse(path).unwrap()
}

fn model_graph_from_source(
    db: &mut Db,
    path: impl Into<Utf8PathBuf>,
    source: &str,
    module_path: PythonModulePath,
) -> ModelGraph {
    let file = db.file_with_contents(path, source);
    djls_project::extract_model_graph(db, file, module_path).clone()
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
                module_path("bench.models"),
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
                module_path("bench.models"),
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

// Resolution: forward, reverse, and combined lookups on a populated graph

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
            module_path("django.contrib.auth.models"),
        )
    })
}

#[divan::bench]
fn resolve_relations(bencher: Bencher) {
    let graph = auth_graph();
    let forward_queries = [
        ("Permission", "content_type"),
        ("Group", "permissions"),
        ("User", "groups"),
    ];
    let reverse_queries = ["Group", "Permission", "ContentType"];
    let relation_queries = [
        ("Group", "user_set"),
        ("Permission", "group_set"),
        ("Permission", "user_set"),
    ];

    bencher.bench_local(|| {
        let mut resolved = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            for (model, field) in forward_queries {
                resolved += usize::from(graph.resolve_forward(model, field).is_some());
            }
            for model in reverse_queries {
                resolved += graph.resolve_reverse(model).count();
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
    files: Vec<(String, PythonModulePath)>,
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

    let files: Vec<(String, PythonModulePath)> = paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            let module_path = djls_testing::module_path_from_file(&path);
            let module_path = PythonModulePath::parse(&module_path).ok()?;
            Some((source, module_path))
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
            for (index, (source, module_path)) in corpus.files.iter().enumerate() {
                let graph = model_graph_from_source(
                    &mut db,
                    format!("/bench/models/corpus/{index}.py"),
                    source,
                    module_path.clone(),
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
