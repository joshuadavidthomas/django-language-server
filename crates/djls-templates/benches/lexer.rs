mod support;

use divan::Bencher;
use support::db::Db;
use support::fixtures::template_fixtures;
use support::fixtures::TemplateFixture;

fn main() {
    divan::main();
}

#[divan::bench(args = template_fixtures())]
fn lex_template_fixture(fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let tokens = djls_templates::lex_template(&db, file);
    divan::black_box(tokens.stream(&db).len());
}

#[divan::bench]
fn lex_all_templates(bencher: Bencher) {
    let fixtures = template_fixtures();
    bencher.bench_local(|| {
        let mut db = Db::new();
        for fixture in fixtures {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            let tokens = djls_templates::lex_template(&db, file);
            divan::black_box(tokens.stream(&db).len());
        }
    });
}

#[divan::bench(args = template_fixtures())]
fn lex_template_incremental(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = Db::new();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    // Prime caches with the baseline source so the benchmark measures the incremental path.
    let _ = djls_templates::lex_template(&db, file);

    let original = fixture.source.clone();
    let modified = {
        let mut text = original.clone();
        text.push(' ');
        text
    };

    let mut revision = 1_u64;
    let mut use_modified = true;

    bencher.bench_local(move || {
        let contents = if use_modified { &modified } else { &original };
        use_modified = !use_modified;

        db.set_file_contents(file, contents, revision);
        revision = revision.wrapping_add(1);

        let tokens = djls_templates::lex_template(&db, file);
        divan::black_box(tokens.stream(&db).len());
    });
}
