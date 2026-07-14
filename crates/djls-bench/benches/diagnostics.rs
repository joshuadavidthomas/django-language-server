use divan::Bencher;
use djls_bench::DIAGNOSTICS_INNER_ITERS;
use djls_bench::DIAGNOSTICS_WARMUP_ITERS;
use djls_bench::DiagnosticDigest;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::ValidationErrorFixture;
use djls_bench::prime;
use djls_bench::primed_realistic_db;
use djls_bench::template_fixtures;
use djls_bench::validation_error_fixtures;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::Severity;
use djls_source::Span;

fn main() {
    divan::main();
}

fn span_of(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .expect("span_of: needle not found in source");
    Span::saturating_from_bounds_usize(start, start + needle.len())
}

static SINGLE_SPAN_SOURCE: &str = "{% block content %}\n<p>Hello</p>\n{% endblock %}\n";

static MULTI_SPAN_SOURCE: &str = "{% block sidebar %}\n<nav>Links</nav>\n{% endblock content %}\n";

static MANY_ERRORS_SOURCE: &str = concat!(
    "{% if and x %}oops{% endif %}\n",
    "{{ name|title:\"arg\" }}\n",
    "{{ text|truncatewords }}\n",
    "{% trans \"hello\" %}\n",
    "{% unknown_tag %}\n",
    "{% for %}empty{% endfor %}\n",
    "{{ value|bogus }}\n",
    "{% block a %}{% endblock b %}\n",
);

fn single_span_diagnostic() -> Diagnostic<'static> {
    Diagnostic::new(
        SINGLE_SPAN_SOURCE,
        "templates/page.html",
        "S100",
        "Unclosed tag: block",
        Severity::Error,
        span_of(SINGLE_SPAN_SOURCE, "{% block content %}"),
        "this block tag is never closed",
    )
}

fn multi_span_diagnostic() -> Diagnostic<'static> {
    Diagnostic::new(
        MULTI_SPAN_SOURCE,
        "templates/layout.html",
        "S103",
        "'content' does not match 'sidebar'",
        Severity::Error,
        span_of(MULTI_SPAN_SOURCE, "{% endblock content %}"),
        "closing tag says 'content'",
    )
    .annotation(
        span_of(MULTI_SPAN_SOURCE, "{% block sidebar %}"),
        "opening tag is 'sidebar'",
        false,
    )
}

#[divan::bench]
fn render_synthetic(bencher: Bencher) {
    let plain = DiagnosticRenderer::plain();
    let styled = DiagnosticRenderer::styled();
    let diagnostics = [single_span_diagnostic(), multi_span_diagnostic()];

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

fn assert_diagnostic_contract(
    db: &djls_bench::Db,
    files: &[djls_source::File],
    expected: &DiagnosticDigest,
) {
    let mut check_digest = DiagnosticDigest::default();
    let mut collected_count = 0;

    for &file in files {
        check_digest.merge(&djls_bench::check_file(db, file).diagnostic_digest());
        collected_count += djls_ide::collect_diagnostics(db, file)
            .expect("template fixture should be eligible for diagnostics")
            .len();
    }

    assert_eq!(&check_digest, expected);
    assert_eq!(collected_count, expected.total());
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
    let expected = DiagnosticDigest::default();
    assert_diagnostic_contract(&db, &files, &expected);

    bench_cached_diagnostics(bencher, db, files);
}

#[divan::bench]
fn collect_cached_errors(bencher: Bencher) {
    let mut db = primed_realistic_db();
    let files: Vec<_> = validation_error_fixtures()
        .iter()
        .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
        .collect();
    // Canonical Django roles intentionally expose structural and `if` expression errors that the
    // former synthetic builtin identities misclassified; fixture cardinality is unchanged.
    let expected = DiagnosticDigest::from_counts(
        0,
        873,
        [
            ("S103", 127),
            ("S108", 126),
            ("S111", 123),
            ("S114", 123),
            ("S115", 128),
            ("S116", 124),
            ("S117", 122),
        ],
    );
    assert_diagnostic_contract(&db, &files, &expected);

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
    // Canonical builtin identities intentionally eliminate the synthetic unknown/unloaded output.
    let expected = DiagnosticDigest::from_counts(0, 20, [("S111", 20)]);
    assert_diagnostic_contract(&db, &files, &expected);

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

struct ValidationRenderFixture<'a> {
    source: &'a str,
    path: &'a str,
    check: djls_bench::CheckResult,
}

fn validation_render_fixture(fixture: &ValidationErrorFixture) -> ValidationRenderFixture<'_> {
    let mut db = primed_realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let check = djls_bench::check_file(&db, file);
    assert!(
        !check.validation_errors.is_empty(),
        "validation error rendering fixture '{}' produced no validation errors",
        fixture.label,
    );

    ValidationRenderFixture {
        source: &fixture.source,
        path: fixture.path.as_str(),
        check,
    }
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
        for fixture in &fixtures {
            for error in &fixture.check.validation_errors {
                if let Some(output) = djls_bench::render_validation_error(
                    fixture.source,
                    fixture.path,
                    error,
                    &config,
                    &renderer,
                ) {
                    divan::black_box_drop(output);
                    rendered_count += 1;
                }
            }
        }
        divan::black_box(rendered_count);
    });
}

#[divan::bench]
fn render_validation_synthetic_output(bencher: Bencher) {
    let mut db = primed_realistic_db();
    let file = db.file_with_contents("/templates/bench.html", MANY_ERRORS_SOURCE);

    let check = djls_bench::check_file(&db, file);
    assert!(
        !check.validation_errors.is_empty(),
        "synthetic validation error benchmark produced no validation errors",
    );

    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for error in &check.validation_errors {
                if let Some(output) = djls_bench::render_validation_error(
                    MANY_ERRORS_SOURCE,
                    "bench.html",
                    error,
                    &config,
                    &renderer,
                ) {
                    divan::black_box_drop(output);
                    rendered_count += 1;
                }
            }
        }
        divan::black_box(rendered_count);
    });
}
