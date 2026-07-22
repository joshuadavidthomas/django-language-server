use camino::Utf8PathBuf;
use divan::Bencher;
use divan::black_box;
use djls_bench::BATCH_INNER_ITERS;
use djls_bench::BenchmarkSetupError;
use djls_bench::Db;
use djls_bench::FixtureLoadError;
use djls_bench::fail;
use djls_bench::primed_realistic_db;
use djls_bench::realistic_db;
use djls_bench::require;
use djls_bench::structure_db;
use djls_bench::template_fixtures;
use djls_semantic::build_template_tree_for_file;
use djls_semantic::compute_opaque_regions;
use djls_semantic::validate_template_file;
use djls_source::File;
use djls_source::FileError;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;

fn main() {
    divan::main();
}

#[derive(Debug, thiserror::Error)]
enum SemanticSetupError {
    #[error(transparent)]
    Fixtures(#[from] FixtureLoadError),
    #[error(transparent)]
    Database(#[from] BenchmarkSetupError),
    #[error("failed to register semantic fixture {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
}

fn template_files(db: &mut Db) -> Result<Vec<File>, SemanticSetupError> {
    let fixtures = template_fixtures()?;
    let mut files = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = db
            .file_with_contents(fixture.path.clone(), &fixture.source)
            .map_err(|source| SemanticSetupError::Register {
                path: fixture.path.clone(),
                source,
            })?;
        files.push(file);
    }
    Ok(files)
}

fn structure_input() -> Result<(Db, Vec<File>), SemanticSetupError> {
    let mut db = structure_db()?;
    let files = template_files(&mut db)?;
    Ok((db, files))
}

fn realistic_input() -> Result<(Db, Vec<File>), SemanticSetupError> {
    let mut db = realistic_db()?;
    let files = template_files(&mut db)?;
    Ok((db, files))
}

fn primed_realistic_input() -> Result<(Db, Vec<File>), SemanticSetupError> {
    let mut db = primed_realistic_db()?;
    let files = template_files(&mut db)?;
    Ok((db, files))
}

#[divan::bench]
fn template_tree_cold(bencher: Bencher) {
    bencher
        .with_inputs(|| require("prepare cold template-tree input", structure_input()))
        .bench_local_values(|(db, files)| {
            let mut total_regions = 0;
            for file in files {
                let nodelist = match parse_template(&db, file) {
                    TemplateParseResult::Parsed(nodelist) => nodelist,
                    TemplateParseResult::NotTemplate => {
                        fail("template-tree benchmark input is not a template");
                    }
                    TemplateParseResult::Unreadable(error) => {
                        fail(format_args!("parse template-tree benchmark input: {error}"));
                    }
                };
                let tree = build_template_tree_for_file(&db, file, nodelist);
                total_regions += tree.regions(&db).iter().count();
            }
            black_box(total_regions);
        });
}

#[divan::bench]
fn validate_cold_project(bencher: Bencher) {
    bencher
        .with_inputs(|| require("prepare cold-Project validation input", realistic_input()))
        .bench_local_values(|(db, files)| {
            let mut validated = 0;
            for file in files {
                validate_template_file(&db, file);
                validated += 1;
            }
            black_box(validated);
        });
}

#[divan::bench]
fn validate_primed_project_cold_templates(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            require(
                "prepare primed-Project validation input",
                primed_realistic_input(),
            )
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
    let fixtures = require("load incremental semantic fixtures", template_fixtures());
    let (mut db, files) = require("prepare incremental validation input", realistic_input());

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
    bencher
        .with_inputs(|| require("prepare cold opaque-region input", structure_input()))
        .bench_local_values(|(db, files)| {
            let mut opaque_files = 0;
            for file in files {
                let nodelist = match parse_template(&db, file) {
                    TemplateParseResult::Parsed(nodelist) => nodelist,
                    TemplateParseResult::NotTemplate => {
                        fail("opaque-region benchmark input is not a template");
                    }
                    TemplateParseResult::Unreadable(error) => {
                        fail(format_args!("parse opaque-region benchmark input: {error}"));
                    }
                };
                let regions = compute_opaque_regions(&db, file, nodelist);
                opaque_files += usize::from(!regions.is_empty());
            }
            black_box(opaque_files);
        });
}
