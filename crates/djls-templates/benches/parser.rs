use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use camino::Utf8Path;
use camino::Utf8PathBuf;
#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::criterion_group;
#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::criterion_main;
#[cfg(feature = "codspeed")]
use codspeed_criterion_compat::Criterion;
use criterion::black_box;
#[cfg(not(feature = "codspeed"))]
use criterion::criterion_group;
#[cfg(not(feature = "codspeed"))]
use criterion::criterion_main;
use criterion::BatchSize;
#[cfg(not(feature = "codspeed"))]
use criterion::Criterion;
use djls_source::File;
use djls_templates::Lexer;
use djls_templates::{
    self,
};

mod fixtures;

#[salsa::db]
#[derive(Clone)]
struct BenchDb {
    storage: salsa::Storage<Self>,
    sources: Arc<Mutex<HashMap<Utf8PathBuf, String>>>,
}

impl BenchDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            sources: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn file_with_contents(&mut self, path: Utf8PathBuf, contents: &str) -> File {
        self.sources
            .lock()
            .expect("sources lock poisoned")
            .insert(path.clone(), contents.to_string());
        File::new(self, path, 0)
    }

    fn set_file_contents(&mut self, file: File, contents: &str, revision: u64) {
        let path = file.path(self);
        self.sources
            .lock()
            .expect("sources lock poisoned")
            .insert(path.clone(), contents.to_string());
        file.set_revision(self, revision);
    }
}

#[salsa::db]
impl salsa::Database for BenchDb {}

#[salsa::db]
impl djls_source::Db for BenchDb {
    fn read_file_source(&self, path: &Utf8Path) -> io::Result<String> {
        let sources = self.sources.lock().expect("sources lock poisoned");
        Ok(sources.get(path).cloned().unwrap_or_default())
    }
}

#[salsa::db]
impl djls_templates::Db for BenchDb {}

fn bench_lexing(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexer_tokenize");
    group.sample_size(60);
    group.measurement_time(Duration::from_secs(8));

    for fixture in fixtures::lex_parse_fixtures() {
        let name = fixture.name.clone();
        let contents = fixture.contents.clone();

        group.bench_function(name, move |b| {
            let contents = contents.clone();

            b.iter_batched(
                BenchDb::new,
                |db| {
                    black_box(Lexer::new(&db, &contents).tokenize());
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_parse_template(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_template");
    group.sample_size(60);
    group.measurement_time(Duration::from_secs(8));

    for fixture in fixtures::lex_parse_fixtures() {
        let name = fixture.name.clone();
        let contents = fixture.contents.clone();
        let path = fixture.file_path();

        group.bench_function(name, move |b| {
            let contents = contents.clone();
            let path = path.clone();

            b.iter_batched(
                move || {
                    let mut db = BenchDb::new();
                    let file = db.file_with_contents(path.clone(), &contents);
                    (db, file)
                },
                |(db, file)| {
                    let nodelist = djls_templates::parse_template(&db, file);
                    black_box(nodelist);
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_incremental_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_template_incremental");
    group.sample_size(40);
    group.measurement_time(Duration::from_secs(6));

    for fixture in fixtures::lex_parse_fixtures().into_iter().take(3) {
        let path = fixture.file_path();
        let contents = fixture.contents.clone();
        let alternate = format!(
            "{}\n{{% comment %}}bench-toggle{{% endcomment %}}",
            contents
        );

        // Cached retrieval benchmark
        let mut warm_db = BenchDb::new();
        let warm_file = warm_db.file_with_contents(path.clone(), contents.as_str());
        let _ = djls_templates::parse_template(&warm_db, warm_file);
        let cached_name = format!("{} (cached)", fixture.name);
        group.bench_function(cached_name, {
            let warm_db = warm_db;
            move |b| {
                b.iter(|| {
                    black_box(djls_templates::parse_template(&warm_db, warm_file));
                });
            }
        });

        // Incremental edit benchmark
        let db = Rc::new(RefCell::new(BenchDb::new()));
        let file = {
            let mut db_mut = db.borrow_mut();
            db_mut.file_with_contents(path.clone(), contents.as_str())
        };
        let incremental_name = format!("{} (edit)", fixture.name);
        group.bench_function(incremental_name, move |b| {
            let db = Rc::clone(&db);
            let base = contents.clone();
            let alt = alternate.clone();
            let mut revision = 1u64;
            let mut toggle = false;

            b.iter(|| {
                let mut db_mut = db.borrow_mut();
                let text = if toggle { base.as_str() } else { alt.as_str() };
                toggle = !toggle;
                db_mut.set_file_contents(file, text, revision);
                revision = revision.wrapping_add(1);
                let result = djls_templates::parse_template(&*db_mut, file);
                black_box(result);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_lexing,
    bench_parse_template,
    bench_incremental_parse
);
criterion_main!(benches);
