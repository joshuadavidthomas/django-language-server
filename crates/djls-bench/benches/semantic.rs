use divan::Bencher;
use djls_bench::BATCH_INNER_ITERS;
use djls_bench::Db;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;

fn main() {
    divan::main();
}

fn template_files(db: &mut Db) -> Vec<djls_source::File> {
    template_fixtures()
        .iter()
        .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
        .collect()
}

#[divan::bench]
fn template_tree_cold(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = realistic_db();
        let files = template_files(&mut db);

        let mut total_regions = 0;
        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                let tree = djls_semantic::build_template_tree(&db, nodelist);
                total_regions += tree.regions(&db).iter().count();
            }
        }
        divan::black_box(total_regions);
    });
}

#[divan::bench]
fn validate_cold(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = realistic_db();
        let files = template_files(&mut db);

        let mut validated = 0;
        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                djls_semantic::validate_nodelist(&db, nodelist);
                validated += 1;
            }
        }
        divan::black_box(validated);
    });
}

struct IncrementalTemplate {
    file: djls_source::File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn validate_incremental(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();

    let mut templates: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

            if let Some(nodelist) = djls_templates::parse_template(&db, file) {
                djls_semantic::validate_nodelist(&db, nodelist);
            }

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

    let mut revision = 1_u64;

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

                db.set_file_contents(template.file, contents, revision);
                revision = revision.wrapping_add(1);

                if let Some(nodelist) = djls_templates::parse_template(&db, template.file) {
                    djls_semantic::validate_nodelist(&db, nodelist);
                    validated += 1;
                }
            }
        }
        divan::black_box(validated);
    });
}

#[divan::bench]
fn opaque_regions_cold(bencher: Bencher) {
    bencher.bench_local(move || {
        let mut db = realistic_db();
        let files = template_files(&mut db);

        let mut opaque_files = 0;
        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                let regions = djls_semantic::compute_opaque_regions(&db, nodelist);
                opaque_files += usize::from(!regions.is_empty());
            }
        }
        divan::black_box(opaque_files);
    });
}
