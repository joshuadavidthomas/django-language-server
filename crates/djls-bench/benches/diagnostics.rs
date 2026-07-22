use camino::Utf8PathBuf;
use divan::Bencher;
use divan::black_box;
use divan::black_box_drop;
use djls::CheckedTemplate;
use djls::check_template;
use djls_bench::BenchmarkSetupError;
use djls_bench::DIAGNOSTICS_INNER_ITERS;
use djls_bench::DIAGNOSTICS_WARMUP_ITERS;
use djls_bench::Db;
use djls_bench::MANY_ERRORS_SOURCE;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::ValidationErrorFixture;
use djls_bench::prime;
use djls_bench::primed_realistic_db;
use djls_bench::require;
use djls_bench::require_some;
use djls_bench::synthetic_render_diagnostics;
use djls_bench::template_fixtures;
use djls_bench::validation_error_fixtures;
use djls_conf::DiagnosticsConfig;
use djls_ide::collect_diagnostics;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::FileError;
use djls_source::FileReadError;

fn main() {
    divan::main();
}

#[derive(Debug, thiserror::Error)]
enum DiagnosticsSetupError {
    #[error(transparent)]
    Database(#[from] BenchmarkSetupError),
    #[error("failed to register diagnostics fixture {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
    #[error("failed to check diagnostics fixture {path}: {source}")]
    Check {
        path: Utf8PathBuf,
        #[source]
        source: FileReadError,
    },
    #[error("diagnostics fixture {path} is not eligible for diagnostics")]
    DiagnosticsUnavailable { path: Utf8PathBuf },
    #[error("diagnostics fixture {path} produced no diagnostics")]
    MissingDiagnostics { path: Utf8PathBuf },
}

fn register_file(
    db: &mut Db,
    path: Utf8PathBuf,
    source: &str,
) -> Result<File, DiagnosticsSetupError> {
    db.file_with_contents(path.clone(), source)
        .map_err(|source| DiagnosticsSetupError::Register { path, source })
}

#[divan::bench]
fn render_synthetic(bencher: Bencher) {
    let plain = DiagnosticRenderer::plain();
    let styled = DiagnosticRenderer::styled();
    let diagnostics = require(
        "build synthetic rendering diagnostics",
        synthetic_render_diagnostics(),
    );

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            total += plain.render(&diagnostics[0]).len();
            total += plain.render(&diagnostics[1]).len();
            total += styled.render(&diagnostics[0]).len();
        }
        black_box(total);
    });
}

fn benchmark_diagnostics(db: &Db, file: File, context: &str) -> usize {
    require_some(context, collect_diagnostics(db, file)).len()
}

fn verify_expected_diagnostics(
    db: &Db,
    file: File,
    path: &Utf8PathBuf,
) -> Result<(), DiagnosticsSetupError> {
    let diagnostics = collect_diagnostics(db, file)
        .ok_or_else(|| DiagnosticsSetupError::DiagnosticsUnavailable { path: path.clone() })?;
    if diagnostics.is_empty() {
        return Err(DiagnosticsSetupError::MissingDiagnostics { path: path.clone() });
    }
    Ok(())
}

fn bench_cached_diagnostics(bencher: Bencher, db: Db, files: Vec<File>) {
    for &file in &files {
        prime(DIAGNOSTICS_WARMUP_ITERS, || {
            black_box(benchmark_diagnostics(
                &db,
                file,
                "prime cached diagnostics for a registered template",
            ));
        });
    }

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for &file in &files {
                total += benchmark_diagnostics(
                    &db,
                    file,
                    "collect cached diagnostics for a registered template",
                );
            }
        }
        black_box(total);
    });
}

#[divan::bench]
fn collect_cached_empty(bencher: Bencher) {
    const EMPTY_FILE_COUNT: usize = 6;

    let mut db = require(
        "initialize primed database for empty diagnostics",
        primed_realistic_db(),
    );
    let mut files = Vec::with_capacity(EMPTY_FILE_COUNT);
    for index in 0..EMPTY_FILE_COUNT {
        let path = Utf8PathBuf::from(format!("/templates/empty/{index}.html"));
        files.push(require(
            format_args!("register empty diagnostics fixture {path}"),
            register_file(&mut db, path.clone(), ""),
        ));
    }
    bench_cached_diagnostics(bencher, db, files);
}

#[divan::bench]
fn collect_cached_errors(bencher: Bencher) {
    let mut db = require(
        "initialize primed database for error diagnostics",
        primed_realistic_db(),
    );
    let fixtures = require(
        "load validation-error diagnostics fixtures",
        validation_error_fixtures(),
    );
    let mut files = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = require(
            format_args!("register diagnostics fixture {}", fixture.path),
            register_file(&mut db, fixture.path.clone(), &fixture.source),
        );
        require(
            format_args!("verify diagnostics fixture {}", fixture.path),
            verify_expected_diagnostics(&db, file, &fixture.path),
        );
        files.push(file);
    }
    bench_cached_diagnostics(bencher, db, files);
}

/// Preserve the previous realistic fixture aggregate as an explicitly mixed-output workload.
#[divan::bench]
fn collect_cached_realistic_end_to_end(bencher: Bencher) {
    let mut db = require(
        "initialize primed database for realistic diagnostics",
        primed_realistic_db(),
    );
    let fixtures = require("load realistic diagnostics fixtures", template_fixtures());
    let mut files = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        files.push(require(
            format_args!("register diagnostics fixture {}", fixture.path),
            register_file(&mut db, fixture.path.clone(), &fixture.source),
        ));
    }
    bench_cached_diagnostics(bencher, db, files);
}

struct IncrementalTemplate {
    file: File,
    original: String,
    modified: String,
    use_modified: bool,
}

#[divan::bench]
fn collect_incremental(bencher: Bencher) {
    let fixtures = require("load incremental diagnostics fixtures", template_fixtures());
    let mut db = require(
        "initialize primed database for incremental diagnostics",
        primed_realistic_db(),
    );

    let mut templates = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = require(
            format_args!("register incremental diagnostics fixture {}", fixture.path),
            register_file(&mut db, fixture.path.clone(), &fixture.source),
        );
        let original = fixture.source.clone();
        let modified = {
            let mut text = original.clone();
            text.push(' ');
            text
        };

        templates.push(IncrementalTemplate {
            file,
            original,
            modified,
            use_modified: true,
        });
    }

    for template in &templates {
        prime(DIAGNOSTICS_WARMUP_ITERS, || {
            black_box(benchmark_diagnostics(
                &db,
                template.file,
                "prime incremental diagnostics for a registered template",
            ));
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

                total += benchmark_diagnostics(
                    &db,
                    template.file,
                    "collect incremental diagnostics for a registered template",
                );
            }
        }
        black_box(total);
    });
}

fn validation_render_fixture(
    fixture: &ValidationErrorFixture,
) -> Result<CheckedTemplate, DiagnosticsSetupError> {
    let mut db = primed_realistic_db()?;
    let file = register_file(&mut db, fixture.path.clone(), &fixture.source)?;
    let checked = check_template(&db, file).map_err(|source| DiagnosticsSetupError::Check {
        path: fixture.path.clone(),
        source,
    })?;
    if !checked.has_diagnostics() {
        return Err(DiagnosticsSetupError::MissingDiagnostics {
            path: fixture.path.clone(),
        });
    }
    Ok(checked)
}

#[divan::bench]
fn render_validation_output(bencher: Bencher) {
    let validation_fixtures = require(
        "load validation rendering fixtures",
        validation_error_fixtures(),
    );
    let mut fixtures = Vec::with_capacity(validation_fixtures.len());
    for fixture in validation_fixtures {
        fixtures.push(require(
            format_args!("prepare validation rendering fixture {}", fixture.path),
            validation_render_fixture(fixture),
        ));
    }
    let config = DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for checked in &fixtures {
            for output in checked.render(&config, &renderer) {
                black_box_drop(output);
                rendered_count += 1;
            }
        }
        black_box(rendered_count);
    });
}

#[divan::bench]
fn render_validation_synthetic_output(bencher: Bencher) {
    let mut db = require(
        "initialize primed database for synthetic validation rendering",
        primed_realistic_db(),
    );
    let path = Utf8PathBuf::from("bench.html");
    let file = require(
        "register synthetic validation rendering fixture",
        register_file(&mut db, path.clone(), MANY_ERRORS_SOURCE),
    );
    let checked = require(
        "check synthetic validation rendering fixture",
        check_template(&db, file).map_err(|source| DiagnosticsSetupError::Check {
            path: path.clone(),
            source,
        }),
    );
    if !checked.has_diagnostics() {
        require::<(), _>(
            "verify synthetic validation rendering diagnostics",
            Err(DiagnosticsSetupError::MissingDiagnostics { path }),
        );
    }

    let config = DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for output in checked.render(&config, &renderer) {
                black_box_drop(output);
                rendered_count += 1;
            }
        }
        black_box(rendered_count);
    });
}
