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
use djls_project::Db as _;
use djls_semantic::block_overrides;
use djls_semantic::build_projectless_template_tree;
use djls_semantic::validate_template_file;
use djls_source::File;
use djls_source::FileError;
use djls_templates::TemplateParseResult;
use djls_templates::parse_template;

fn main() {
    divan::main();
}

// Keep the largest star in routine CI; opt into the full scaling sweep with:
// DJLS_BENCH_INHERITANCE_SCALING=1 cargo bench -p djls-bench --bench semantic -- block_overrides_warm_star
fn inheritance_sizes() -> &'static [usize] {
    if std::env::var("DJLS_BENCH_INHERITANCE_SCALING").as_deref() == Ok("1") {
        &[100, 500, 1000, 2000]
    } else {
        &[2000]
    }
}

#[divan::bench(args = inheritance_sizes())]
fn block_overrides_warm_star(bencher: Bencher, descendants: usize) {
    let mut db = require("prepare inheritance database", realistic_db());
    let base = require(
        "register inheritance base",
        db.file_with_contents(
            "/templates/base.html",
            "{% block content %}base{% endblock %}",
        ),
    );
    for index in 0..descendants {
        require(
            "register inheritance child",
            db.file_with_contents(
                format!("/templates/child-{index}.html"),
                "{% extends 'base.html' %}{% block content %}child{% endblock %}",
            ),
        );
    }
    let Some(project) = db.project() else {
        fail("inheritance benchmark requires a project");
    };
    assert_eq!(
        block_overrides(&db, project, base, "content").len(),
        descendants
    );

    bencher.bench_local(|| black_box(block_overrides(&db, project, base, "content")));
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
                let tree = build_projectless_template_tree(&db, nodelist);
                total_regions += tree.regions(&db).iter().count();
            }
            black_box(total_regions);
        });
}

/// Every unclosed block stays open to the end of the Template, so the tree builder holds
/// one frame per tag. Guards against per-tag work that grows with nesting depth.
#[divan::bench]
fn template_tree_deeply_nested(bencher: Bencher) {
    const DEPTH: usize = 10_000;
    bencher
        .with_inputs(|| {
            let mut db = require("prepare deeply nested template input", structure_db());
            let file = require(
                "register deeply nested template",
                db.file_with_contents("/templates/nested.html", &"{% if x %}\n".repeat(DEPTH)),
            );
            (db, file)
        })
        .bench_local_values(|(db, file)| {
            let TemplateParseResult::Parsed(nodelist) = parse_template(&db, file) else {
                fail("deeply nested benchmark input should parse as a template");
            };
            let tree = build_projectless_template_tree(&db, nodelist);
            black_box(tree.regions(&db).iter().count());
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
