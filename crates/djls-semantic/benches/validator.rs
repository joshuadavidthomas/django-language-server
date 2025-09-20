use std::collections::HashMap;
use std::io;
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
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::{
    self,
};
use djls_source::File;

#[path = "../../djls-templates/benches/fixtures/mod.rs"]
mod fixtures;

#[salsa::db]
#[derive(Clone)]
struct BenchDb {
    storage: salsa::Storage<Self>,
    sources: Arc<Mutex<HashMap<Utf8PathBuf, String>>>,
    tag_specs: djls_semantic::TagSpecs,
}

impl BenchDb {
    fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            sources: Arc::new(Mutex::new(HashMap::new())),
            tag_specs: djls_semantic::django_builtin_specs(),
        }
    }

    fn file_with_contents(&mut self, path: Utf8PathBuf, contents: &str) -> File {
        self.sources
            .lock()
            .expect("sources lock poisoned")
            .insert(path.clone(), contents.to_string());
        File::new(self, path, 0)
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

#[salsa::db]
impl djls_semantic::Db for BenchDb {
    fn tag_specs(&self) -> djls_semantic::TagSpecs {
        self.tag_specs.clone()
    }
}

fn bench_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("validate_nodelist");
    group.sample_size(60);
    group.measurement_time(Duration::from_secs(8));

    for fixture in fixtures::validation_fixtures() {
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
                    if let Some(nodelist) = djls_templates::parse_template(&db, file) {
                        djls_semantic::validate_nodelist(&db, nodelist);
                        let errors = djls_semantic::validate_nodelist::accumulated::<
                            ValidationErrorAccumulator,
                        >(&db, nodelist);
                        black_box(errors);
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_validation);
criterion_main!(benches);
