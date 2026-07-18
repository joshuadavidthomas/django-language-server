use divan::Bencher;
use djls_bench::DIAGNOSTICS_INNER_ITERS;
use djls_bench::DIAGNOSTICS_WARMUP_ITERS;
use djls_bench::MANY_ERRORS_SOURCE;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::ValidationErrorFixture;
use djls_bench::prime;
use djls_bench::primed_realistic_db;
use djls_bench::synthetic_render_diagnostics;
use djls_bench::template_fixtures;
use djls_bench::validation_error_fixtures;
use djls_source::DiagnosticRenderer;

fn main() {
    divan::main();
}

#[divan::bench]
fn render_synthetic(bencher: Bencher) {
    let plain = DiagnosticRenderer::plain();
    let styled = DiagnosticRenderer::styled();
    let diagnostics = synthetic_render_diagnostics();

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            total += plain.render(&diagnostics[0]).len();
            total += plain.render(&diagnostics[1]).len();
            total += styled.render(&diagnostics[0]).len();
        }
        divan::black_box(total);
    });
}

fn bench_cached_diagnostics(bencher: Bencher, db: djls_bench::Db, files: Vec<djls_source::File>) {
    for &file in &files {
        prime(DIAGNOSTICS_WARMUP_ITERS, || {
            let _ = djls_ide::collect_diagnostics(&db, file);
        });
    }

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for &file in &files {
                total += djls_ide::collect_diagnostics(&db, file)
                    .expect("template fixture should be eligible for diagnostics")
                    .len();
            }
        }
        divan::black_box(total);
    });
}

#[divan::bench]
fn collect_cached_empty(bencher: Bencher) {
    const EMPTY_FILE_COUNT: usize = 6;

    let mut db = primed_realistic_db();
    let files: Vec<_> = (0..EMPTY_FILE_COUNT)
        .map(|index| db.file_with_contents(format!("/templates/empty/{index}.html"), ""))
        .collect();
    bench_cached_diagnostics(bencher, db, files);
}

#[divan::bench]
fn collect_cached_errors(bencher: Bencher) {
    let mut db = primed_realistic_db();
    let files: Vec<_> = validation_error_fixtures()
        .iter()
        .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
        .collect();
    bench_cached_diagnostics(bencher, db, files);
}

/// Preserve the previous realistic fixture aggregate as an explicitly mixed-output workload.
#[divan::bench]
fn collect_cached_realistic_end_to_end(bencher: Bencher) {
    let mut db = primed_realistic_db();
    let files: Vec<_> = template_fixtures()
        .iter()
        .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
        .collect();
    bench_cached_diagnostics(bencher, db, files);
}

struct IncrementalTemplate {
    file: djls_source::File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn collect_incremental(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = primed_realistic_db();

    let mut templates: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
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

    for template in &templates {
        prime(DIAGNOSTICS_WARMUP_ITERS, || {
            let _ = djls_ide::collect_diagnostics(&db, template.file);
        });
    }

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for template in &mut templates {
                let contents = if template.use_modified {
                    &template.modified
                } else {
                    &template.original
                };
                template.use_modified = !template.use_modified;

                db.set_file_contents(template.file, contents);

                total += djls_ide::collect_diagnostics(&db, template.file)
                    .expect("template fixture should be eligible for diagnostics")
                    .len();
            }
        }
        divan::black_box(total);
    });
}

fn validation_render_fixture(fixture: &ValidationErrorFixture) -> djls::CheckedTemplate {
    let mut db = primed_realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let checked = djls::check_template(&db, file).expect("benchmark file should be readable");
    assert!(
        checked.has_diagnostics(),
        "validation error rendering fixture '{}' produced no diagnostics",
        fixture.label,
    );
    checked
}

#[divan::bench]
fn render_validation_output(bencher: Bencher) {
    let fixtures: Vec<_> = validation_error_fixtures()
        .iter()
        .map(validation_render_fixture)
        .collect();
    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for checked in &fixtures {
            for output in checked.render(&config, &renderer) {
                divan::black_box_drop(output);
                rendered_count += 1;
            }
        }
        divan::black_box(rendered_count);
    });
}

#[divan::bench]
fn render_validation_synthetic_output(bencher: Bencher) {
    let mut db = primed_realistic_db();
    let file = db.file_with_contents("bench.html", MANY_ERRORS_SOURCE);

    let checked = djls::check_template(&db, file).expect("benchmark file should be readable");
    assert!(
        checked.has_diagnostics(),
        "synthetic validation error benchmark produced no diagnostics",
    );

    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for output in checked.render(&config, &renderer) {
                divan::black_box_drop(output);
                rendered_count += 1;
            }
        }
        divan::black_box(rendered_count);
    });
}
