use divan::Bencher;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::BATCH_INNER_ITERS;

fn main() {
    divan::main();
}

#[divan::bench]
fn parse_all_templates(bencher: Bencher) {
    let fixtures = template_fixtures();
    bencher.bench_local(move || {
        let mut total_nodes = 0;
        for fixture in fixtures {
            let (nodes, _errors) = djls_templates::parse_template_impl(&fixture.source);
            total_nodes += nodes.len();
        }
        divan::black_box(total_nodes);
    });
}

struct IncrementalTemplate {
    file: djls_source::File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn parse_templates_incremental(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = Db::new();

    let mut templates: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            let _ = djls_templates::parse_template(&db, file);

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
        let mut total_nodes = 0;
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
                    total_nodes += nodelist.nodelist(&db).len();
                }
            }
        }
        divan::black_box(total_nodes);
    });
}
