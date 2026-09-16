use std::fmt::Write as _;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::Fixture;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::fail;
use djls_bench::python_fixtures;
use djls_bench::require;
use djls_bench::require_some;
use djls_project::InvalidModuleName;
use djls_project::Project;
use djls_project::PythonEnvironment;
use djls_project::PythonModuleName;
use djls_project::PythonSourceModule;
use djls_project::SearchPath;
use djls_project::SearchPaths;
use djls_project::testing::django_settings;
use djls_project::testing::settings_module_file;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileError;
use djls_testing::Corpus;
use djls_testing::OsTestDatabase;
use djls_testing::ProjectFixture;
use djls_testing::TestDatabase;
use djls_testing::extract_bundle;

struct ExtractionFile {
    file: File,
    module: PythonModuleName,
}

struct ExtractionInput {
    db: Db,
    files: Vec<ExtractionFile>,
}

#[derive(Debug, thiserror::Error)]
enum ExtractionSetupError {
    #[error("invalid extraction benchmark module name: {0}")]
    Module(#[from] InvalidModuleName),
    #[error("failed to register extraction fixture {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
}

fn main() {
    divan::main();
}

fn extraction_input(fixtures: &[Fixture]) -> Result<ExtractionInput, ExtractionSetupError> {
    let mut db = Db::new();
    let module = PythonModuleName::parse("bench.module")?;
    let mut files = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = db
            .file_with_contents(fixture.path.clone(), &fixture.source)
            .map_err(|source| ExtractionSetupError::Register {
                path: fixture.path.clone(),
                source,
            })?;
        files.push(ExtractionFile {
            file,
            module: module.clone(),
        });
    }

    Ok(ExtractionInput { db, files })
}

#[divan::bench]
fn tags(bencher: Bencher) {
    let fixtures = require("load Python extraction fixtures", python_fixtures());
    bencher
        .with_inputs(|| require("prepare tag extraction input", extraction_input(fixtures)))
        .bench_local_values(|input| {
            let mut extracted = 0;
            for extraction_file in input.files {
                let bundle =
                    extract_bundle(&input.db, extraction_file.file, extraction_file.module);
                extracted += bundle.tag_rules.len();
                extracted += bundle.filter_arities.len();
                extracted += bundle.block_specs.as_map().len();
                divan::black_box(bundle);
            }
            divan::black_box(extracted);
        });
}

/// Fresh settings parsing, evaluation, and projection with growing independent
/// conditional bindings. Source generation and database setup are not timed.
#[divan::bench(args = [8, 32, 64])]
fn settings_cold_branches(bencher: Bencher, branches: usize) {
    let mut source = String::from("INSTALLED_APPS = ['core']\n");
    for index in 0..branches {
        require(
            "write conditional settings fixture",
            writeln!(
                source,
                "if FLAG_{index}:\n    SETTING_{index} = 'enabled'\nelse:\n    SETTING_{index} = 'disabled'"
            ),
        );
    }
    bencher
        .with_inputs(|| {
            let mut db = TestDatabase::new();
            let project = require(
                "prepare cold conditional settings input",
                ProjectFixture::new("/corpus/repos/settings-project/src/project")
                    .django_settings_module("settings")
                    .file(
                        "/corpus/repos/settings-project/src/project/settings.py",
                        source.as_str(),
                    )
                    .install(&mut db),
            );
            (db, project)
        })
        .bench_local_values(|(db, project)| {
            divan::black_box(django_settings(&db, project));
        });
}

/// Fresh settings parsing, evaluation, and projection with a growing try body.
/// The inputs cover a common small body, an upper-end real-world-sized body, and an
/// explicit stress case. Source generation and database setup are not timed.
#[divan::bench(args = [2, 9, 64])]
fn settings_cold_try_prefixes(bencher: Bencher, statements: usize) {
    let mut source = String::from("INSTALLED_APPS = ['core']\ntry:\n");
    for index in 0..statements {
        require(
            "write try-prefix settings fixture",
            writeln!(source, "    SETTING_{index} = 'value-{index}'"),
        );
    }
    source.push_str("except Exception:\n    RECOVERED = True\n");
    bencher
        .with_inputs(|| {
            let mut db = TestDatabase::new();
            let project = require(
                "prepare cold try-prefix settings input",
                ProjectFixture::new("/corpus/repos/settings-project/src/project")
                    .django_settings_module("settings")
                    .file(
                        "/corpus/repos/settings-project/src/project/settings.py",
                        source.as_str(),
                    )
                    .install(&mut db),
            );
            (db, project)
        })
        .bench_local_values(|(db, project)| {
            divan::black_box(django_settings(&db, project));
        });
}

/// Real settings source and imports, with no installed dependencies and fresh
/// evaluation queries. Corpus metadata, search paths, and entry resolution are setup;
/// source reads, parsing, evaluation, and settings projection are timed.
#[divan::bench(args = ["healthchecks", "netbox", "pretix"], sample_count = 10)]
fn settings_cold_corpus(bencher: Bencher, name: &str) {
    let corpus = require("load settings corpus", Corpus::require());
    let declaration = require(
        "find settings corpus project",
        require(
            "load corpus project declarations",
            corpus.repo_settings_projects(),
        )
        .into_iter()
        .find(|project| project.repo_name == name)
        .ok_or("benchmark repository must declare settings metadata"),
    );
    let [settings_module] = declaration.django_settings_modules.as_slice() else {
        djls_bench::fail("settings corpus benchmark requires exactly one settings module");
    };
    let settings_module = require(
        "parse corpus settings module",
        PythonModuleName::parse(settings_module),
    );
    bencher
        .with_inputs(|| {
            let mut db = OsTestDatabase::with_disk_roots([declaration.checkout_root.clone()]);
            let python_environment =
                PythonEnvironment::Path(corpus.root().join("hermetic-no-venv"));
            let search_paths = SearchPaths::from_project_settings(
                db.file_system(),
                &declaration.project_root,
                &python_environment,
                &[],
            );
            search_paths.register_roots(&db);
            let project = Project::new(
                &db,
                declaration.project_root.clone(),
                search_paths,
                python_environment,
                Some(settings_module.clone()),
                Vec::new(),
                Vec::new(),
                djls_conf::Settings::default().tagspecs().clone(),
            );
            db.set_project(project);
            require(
                "resolve corpus settings entry",
                settings_module_file(&db, project).ok_or("settings module must resolve"),
            );
            (db, project)
        })
        .bench_local_values(|(db, project)| {
            divan::black_box(django_settings(&db, project));
        });
}

/// Real Pretix settings and imports with the latest pinned Django corpus checkout exposed as an
/// installed package. This preserves both repositories unchanged while making the dependency
/// source independent of the caller's `VIRTUAL_ENV`.
///
/// The workload is ignored because one cold evaluation currently takes about 90 seconds. Run it
/// with:
/// `cargo bench -p djls-bench --bench extraction -- --ignored settings_cold_pretix_with_django`.
#[divan::bench(sample_count = 1, sample_size = 1)]
#[ignore = "one cold evaluation currently takes about 90 seconds"]
fn settings_cold_pretix_with_django(bencher: Bencher) {
    let corpus = require("load settings corpus", Corpus::require());
    let declaration = require(
        "find Pretix settings corpus project",
        require(
            "load corpus project declarations",
            corpus.repo_settings_projects(),
        )
        .into_iter()
        .find(|project| project.repo_name == "pretix")
        .ok_or("benchmark corpus must declare Pretix settings metadata"),
    );
    let [settings_module] = declaration.django_settings_modules.as_slice() else {
        fail("Pretix settings corpus benchmark requires exactly one settings module");
    };
    let settings_module = require(
        "parse Pretix settings module",
        PythonModuleName::parse(settings_module),
    );
    let django_root = require_some(
        "find latest pinned Django corpus checkout",
        corpus.latest_package("django"),
    );
    let expected_django_init = django_root.join("django/__init__.py");

    bencher
        .with_inputs(|| {
            let mut db = OsTestDatabase::with_disk_roots([
                declaration.checkout_root.clone(),
                django_root.clone(),
            ]);
            let python_environment =
                PythonEnvironment::Path(corpus.root().join("hermetic-no-venv"));
            let mut paths = Vec::with_capacity(3);
            let src_root = declaration.project_root.join("src");
            if db.file_system().is_dir(&src_root)
                && !db.file_system().is_file(&src_root.join("__init__.py"))
            {
                paths.push(SearchPath::FirstParty(src_root));
            }
            paths.push(SearchPath::FirstParty(declaration.project_root.clone()));
            paths.push(SearchPath::SitePackages(django_root.clone()));
            let search_paths = SearchPaths::from_paths(paths);
            search_paths.register_roots(&db);
            let project = Project::new(
                &db,
                declaration.project_root.clone(),
                search_paths,
                python_environment,
                Some(settings_module.clone()),
                Vec::new(),
                Vec::new(),
                djls_conf::Settings::default().tagspecs().clone(),
            );
            db.set_project(project);
            require(
                "resolve Pretix settings entry",
                settings_module_file(&db, project).ok_or("Pretix settings module must resolve"),
            );
            let django_module = require(
                "resolve Django from the pinned corpus checkout",
                PythonSourceModule::resolve(
                    &db,
                    project,
                    require(
                        "parse Django module name",
                        PythonModuleName::parse("django"),
                    ),
                )
                .ok_or("Django module must resolve"),
            );
            if django_module.path() != expected_django_init {
                fail(format_args!(
                    "Django resolved to {}, expected {}",
                    django_module.path(),
                    expected_django_init
                ));
            }
            (db, project)
        })
        .bench_local_values(|(db, project)| {
            divan::black_box(django_settings(&db, project));
        });
}

#[divan::bench]
fn merge_tags(bencher: Bencher) {
    let fixtures = require("load Python extraction fixtures", python_fixtures());
    let input = require("prepare tag merge input", extraction_input(fixtures));
    let bundles: Vec<_> = input
        .files
        .iter()
        .map(|extraction_file| {
            extract_bundle(
                &input.db,
                extraction_file.file,
                extraction_file.module.clone(),
            )
        })
        .collect();

    bencher.bench_local(move || {
        let mut merged_rules = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            let mut specs = djls_semantic::TagSpecs::default();
            for bundle in &bundles {
                specs
                    .merge_block_specs(&bundle.block_specs)
                    .merge_tag_rules(&bundle.tag_rules);
            }
            merged_rules += specs.len();
        }
        divan::black_box(merged_rules);
    });
}
