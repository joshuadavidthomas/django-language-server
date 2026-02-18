use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::model_fixtures;
use djls_bench::ModelFixture;
use djls_python::ModelGraph;

fn main() {
    divan::main();
}

// Per-fixture extraction: parse one models.py → ModelGraph

#[divan::bench(args = model_fixtures())]
fn extract_model_graph(fixture: &ModelFixture) {
    divan::black_box(djls_python::extract_model_graph(
        &fixture.source,
        "bench.models",
    ));
}

// Batch extraction: all fixtures in one iteration

#[divan::bench]
fn extract_all_models(bencher: Bencher) {
    let fixtures = model_fixtures();
    bencher.bench_local(move || {
        for fixture in fixtures {
            divan::black_box(djls_python::extract_model_graph(
                &fixture.source,
                "bench.models",
            ));
        }
    });
}

// Merge: extract graphs then merge them (the hot path in compute_model_graph)

#[divan::bench]
fn merge_graphs(bencher: Bencher) {
    let fixtures = model_fixtures();
    let graphs: Vec<ModelGraph> = fixtures
        .iter()
        .map(|f| djls_python::extract_model_graph(&f.source, "bench.models"))
        .collect();

    bencher.bench_local(move || {
        let mut merged = ModelGraph::new();
        for graph in &graphs {
            merged.merge(graph.clone());
        }
        divan::black_box(merged);
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
        djls_python::extract_model_graph(&auth.source, "django.contrib.auth.models")
    })
}

#[divan::bench]
fn resolve_forward(bencher: Bencher) {
    let graph = auth_graph();
    bencher.bench_local(|| {
        divan::black_box(graph.resolve_forward("Permission", "content_type"));
    });
}

#[divan::bench]
fn resolve_reverse(bencher: Bencher) {
    let graph = auth_graph();
    bencher.bench_local(|| {
        let reverses: Vec<_> = graph.resolve_reverse("Group").collect();
        divan::black_box(reverses);
    });
}

#[divan::bench]
fn resolve_relation(bencher: Bencher) {
    let graph = auth_graph();
    // Use a reverse-only lookup (Group has no forward "user_set" field, but
    // User's inherited M2M to Group has related_name="user_set") so this
    // exercises the forward-miss → reverse-scan fallthrough in resolve_relation.
    bencher.bench_local(|| {
        divan::black_box(graph.resolve_relation("Group", "user_set"));
    });
}

// Corpus-scale: extract all models.py from Django, then from the full corpus

struct CorpusModels {
    files: Vec<(String, String)>, // (source, module_path)
}

fn load_corpus_models_inner(
    get_paths: impl FnOnce(&djls_corpus::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusModels> {
    if !djls_corpus::Corpus::is_available() {
        return None;
    }

    let corpus = djls_corpus::Corpus::require();
    let paths = get_paths(&corpus)?;

    let files: Vec<(String, String)> = paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            let module_path = djls_corpus::module_path_from_file(&path);
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
        eprintln!("corpus not synced, skipping");
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .bench_local(move || {
            let mut merged = ModelGraph::new();
            for (source, module_path) in &corpus.files {
                let graph = djls_python::extract_model_graph(source, module_path);
                merged.merge(graph);
            }
            divan::black_box(merged);
        });
}

#[divan::bench]
fn extract_corpus_django(bencher: Bencher) {
    bench_corpus(bencher, load_django_models());
}

#[divan::bench]
fn extract_corpus_all(bencher: Bencher) {
    bench_corpus(bencher, load_all_corpus_models());
}
