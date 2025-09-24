use divan::Bencher;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;

fn main() {
    divan::main();
}

#[divan::bench(args = template_fixtures())]
fn parse_template(bencher: Bencher, fixture: &TemplateFixture) {
    bencher
        .with_inputs(|| {
            let mut db = Db::new();
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            (db, file)
        })
        .bench_local_values(|(db, file)| {
            if let Some(nodelist) = djls_templates::parse_template(&db, file) {
                divan::black_box(nodelist.nodelist(&db).len());
            }
            (db, file)
        });
}

#[divan::bench]
fn parse_all_templates(bencher: Bencher) {
    let fixtures = template_fixtures();

    bencher
        .with_inputs(|| {
            let mut db = Db::new();
            let mut files = Vec::with_capacity(fixtures.len());

            for fixture in fixtures {
                let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
                files.push(file);
            }

            (db, files)
        })
        .bench_local_values(|(db, files)| {
            for file in &files {
                if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                    divan::black_box(nodelist.nodelist(&db).len());
                }
            }
            (db, files)
        });
}

#[divan::bench(args = template_fixtures())]
fn parse_template_incremental(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    // Prime caches with the baseline source so the benchmark measures the incremental path.
    let _ = djls_templates::parse_template(&db, file);

    let original = fixture.source.clone();
    let modified = {
        let mut text = original.clone();
        text.push(' ');
        text
    };

    // Flip between original/modified sources so each iteration simulates an edit or revert
    // against the same cached parse, forcing Salsa down its incremental path.
    let mut revision = 1_u64;
    let mut use_modified = true;

    bencher.bench_local(move || {
        let contents = if use_modified { &modified } else { &original };
        use_modified = !use_modified;

        db.set_file_contents(file, contents, revision);
        revision = revision.wrapping_add(1);

        if let Some(nodelist) = djls_templates::parse_template(&db, file) {
            divan::black_box(nodelist.nodelist(&db).len());
        }
    });
}
