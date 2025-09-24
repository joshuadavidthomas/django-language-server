use divan::Bencher;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;

fn main() {
    divan::main();
}

#[divan::bench(args = template_fixtures())]
fn parse_template(bencher: Bencher, fixture: &TemplateFixture) {
    let db = std::cell::RefCell::new(Db::new());

    let counter = std::cell::Cell::new(0_usize);

    bencher
        .with_inputs(|| {
            let i = counter.get();
            counter.set(i + 1);
            let path = format!("{}.bench{}", fixture.path.clone(), i);
            db.borrow_mut()
                .file_with_contents(path.into(), &fixture.source.clone())
        })
        .bench_local_values(|file| {
            let db = db.borrow();
            if let Some(nodelist) = djls_templates::parse_template(&*db, file) {
                divan::black_box(nodelist.nodelist(&*db).len());
            }
        });
}

#[divan::bench]
fn parse_all_templates(bencher: Bencher) {
    let fixtures = template_fixtures();
    let db = std::cell::RefCell::new(Db::new());

    let counter = std::cell::Cell::new(0_usize);

    bencher
        .with_inputs(|| {
            let i = counter.get();
            counter.set(i + 1);

            let mut db = db.borrow_mut();
            fixtures
                .iter()
                .map(|fixture| {
                    let path = format!("{}.bench{}", fixture.path, i);
                    db.file_with_contents(path.into(), &fixture.source)
                })
                .collect::<Vec<_>>()
        })
        .bench_local_values(|files| {
            let db = db.borrow();
            for file in files {
                if let Some(nodelist) = djls_templates::parse_template(&*db, file) {
                    divan::black_box(nodelist.nodelist(&*db).len());
                }
            }
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
