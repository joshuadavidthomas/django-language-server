use divan::Bencher;
use divan::black_box;
use djls_bench::BATCH_INNER_ITERS;
use djls_bench::Db;
use djls_bench::primed_realistic_db;
use djls_bench::realistic_db;
use djls_bench::structure_db;
use djls_bench::template_fixtures;
use djls_semantic::build_template_tree_for_file;
use djls_semantic::compute_opaque_regions;
use djls_semantic::validate_template_file;
use djls_source::File;
use djls_templates::parse_template;

fn main() {
    divan::main();
}

fn template_files(db: &mut Db) -> Vec<File> {
    template_fixtures()
        .iter()
        .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
        .collect()
}

#[divan::bench]
fn template_tree_cold(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = structure_db();
        let files = template_files(&mut db);

        let mut total_regions = 0;
        for file in &files {
            let nodelist = parse_template(&db, *file).expect("benchmark template should parse");
            let tree = build_template_tree_for_file(&db, *file, nodelist);
            total_regions += tree.regions(&db).iter().count();
        }
        black_box(total_regions);
    });
}

#[divan::bench]
fn validate_cold_project(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = realistic_db();
        let files = template_files(&mut db);

        let mut validated = 0;
        for file in &files {
            validate_template_file(&db, *file);
            validated += 1;
        }
        black_box(validated);
    });
}

#[divan::bench]
fn validate_primed_project_cold_templates(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            let mut db = primed_realistic_db();
            let files = template_files(&mut db);
            (db, files)
        })
        .bench_local_values(|(db, files)| {
            let mut validated = 0;
            for file in files {
                validate_template_file(&db, file);
                validated += 1;
            }
            black_box(validated);
        });
}

struct IncrementalTemplate {
    file: File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn validate_incremental(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();
    let files = template_files(&mut db);

    for &file in &files {
        validate_template_file(&db, file);
    }

    let mut templates: Vec<_> = fixtures
        .iter()
        .zip(files)
        .map(|(fixture, file)| {
            let original = fixture.source.clone();
            let modified = {
                let mut text = original.clone();
                text.push(' ');
                text
            };

            IncrementalTemplate {
                file,
                original,
                modified,
                use_modified: true,
            }
        })
        .collect();

    bencher.bench_local(move || {
        let mut validated = 0;
        for _ in 0..BATCH_INNER_ITERS {
            for template in &mut templates {
                let contents = if template.use_modified {
                    &template.modified
                } else {
                    &template.original
                };
                template.use_modified = !template.use_modified;

                db.set_file_contents(template.file, contents);

                validate_template_file(&db, template.file);
                validated += 1;
            }
        }
        black_box(validated);
    });
}

#[divan::bench]
fn opaque_regions_cold(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = structure_db();
        let files = template_files(&mut db);

        let mut opaque_files = 0;
        for file in &files {
            let nodelist = parse_template(&db, *file).expect("benchmark template should parse");
            let regions = compute_opaque_regions(&db, *file, nodelist);
            opaque_files += usize::from(!regions.is_empty());
        }
        black_box(opaque_files);
    });
}
