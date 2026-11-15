//! Django differential oracle: run real Django on a project and report where DJLS disagrees.
//!
//! `tools/django_oracle.py` runs in the project's own interpreter, imports each Template
//! Library, and compiles generated probe templates. This module builds the same Project the
//! way the corpus tests do, asks DJLS the same questions, and compares the answers.
//!
//! A project without a configured settings module, or an explicitly synthetic run, gets
//! generated settings listing the libraries under `TEMPLATES[0]["OPTIONS"]["libraries"]`.
//! A configured project that cannot start fails instead of falling back to synthetic settings.

mod compare;
mod django;
mod report;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use anyhow::Context as _;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as _;
use djls_project::PythonEnvironment;
use djls_project::template_resolution;
pub use report::Finding;
pub use report::FindingClass;
pub use report::OracleReport;
pub use report::render_markdown;

use crate::corpus::Corpus;
use crate::db::OsTestDatabase;
use crate::fixtures::ProjectFixture;

const SYNTHETIC_SETTINGS_MODULE: &str = "djls_oracle_settings";

/// One project to check, configured the way a user configures DJLS.
#[derive(Debug, Clone)]
pub struct OracleTarget {
    pub name: String,
    pub project_root: Utf8PathBuf,
    /// Directories DJLS may read from disk: the checkout and anything outside it it imports.
    pub disk_roots: Vec<Utf8PathBuf>,
    /// The virtual environment DJLS inspects statically.
    pub environment_root: Utf8PathBuf,
    /// The interpreter that runs Django. It must see the same packages as `environment_root`.
    pub python: Utf8PathBuf,
    /// Extra Python search path entries, as the `pythonpath` setting.
    pub source_roots: Vec<Utf8PathBuf>,
    pub settings_module: Option<String>,
    /// Environment variables for the project. Django runs with them and DJLS gets them as its
    /// `env_file` entries. `{scratch}` expands to the run's temporary directory.
    pub env: Vec<(String, String)>,
    /// Apps synthetic settings install before each library's own app, such as dependencies.
    pub apps: Vec<String>,
    /// Template Library sources to check even when no settings module installs them.
    pub library_files: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone)]
pub struct OracleOptions {
    pub include_django: bool,
    /// Use generated settings even when the target has a settings module.
    pub synthetic: bool,
    pub timeout: Duration,
    pub backend: Option<String>,
    pub seeds: Option<Utf8PathBuf>,
}

impl Default for OracleOptions {
    fn default() -> Self {
        Self {
            include_django: false,
            synthetic: false,
            timeout: Duration::from_mins(10),
            backend: None,
            seeds: None,
        }
    }
}

/// Which settings both sides were given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsMode {
    /// The project's own settings module.
    Project,
    /// A generated settings module that installs the libraries under test.
    Synthetic,
}

/// Run Django and DJLS on one target and compare them.
#[allow(clippy::too_many_lines)] // One linear sequence of Django runs and DJLS databases.
pub fn run_oracle(target: &OracleTarget, options: &OracleOptions) -> anyhow::Result<OracleReport> {
    let scratch = tempfile::tempdir().context("failed to create oracle scratch directory")?;
    let scratch_root = Utf8PathBuf::from_path_buf(scratch.path().to_path_buf())
        .map_err(|path| anyhow::anyhow!("scratch path is not UTF-8: {}", path.display()))?;
    let mut notes = Vec::new();
    let expanded = OracleTarget {
        env: target
            .env
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.replace("{scratch}", scratch_root.as_str()),
                )
            })
            .collect(),
        ..target.clone()
    };
    let target = &expanded;

    // Derive candidate import names independently of DJLS's Python resolver.
    let discovery_db = oracle_database(target, target.settings_module.as_deref(), None, &[])?;
    let project = discovery_db
        .project()
        .context("oracle database has no project")?;
    let mut requested = BTreeMap::new();
    for path in &target.library_files {
        let module = target
            .source_roots
            .iter()
            .chain(std::iter::once(&target.project_root))
            .find_map(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.with_extension("").as_str().replace('/', "."));
        match module {
            Some(module) => {
                let stem = module.rsplit('.').next().unwrap_or_default().to_string();
                let mut load_name = stem.clone();
                let mut suffix = 2;
                while requested.contains_key(&load_name) {
                    load_name = format!("{stem}_{suffix}");
                    suffix += 1;
                }
                requested.insert(load_name, module);
            }
            None => anyhow::bail!("candidate {path} is outside every Python root"),
        }
    }
    let template_names: Vec<String> = template_resolution(&discovery_db, project)
        .template_names(&discovery_db)
        .map(|name| name.name(&discovery_db).clone())
        .collect();

    let mut mode = SettingsMode::Synthetic;
    let mut answers = None;
    if let Some(settings_module) = target
        .settings_module
        .as_ref()
        .filter(|_| !options.synthetic)
    {
        // With project settings, Django discovers libraries itself.
        let request = django::Request {
            settings_module: settings_module.clone(),
            sys_path: python_path(target, None),
            library_modules: BTreeMap::new(),
            template_names: template_names.clone(),
            include_django: options.include_django,
            backend: options.backend.clone(),
            ..django::Request::default()
        };
        let project_answers = django::run(target, &request, options.timeout)?;
        if let Some(error) = &project_answers.setup_error {
            anyhow::bail!("Django project setup failed for {settings_module}: {error}");
        }
        mode = SettingsMode::Project;
        answers = Some(project_answers);
    }

    // Django is set up with an empty library list and receives the candidates in the request,
    // so one module that is not a library cannot break its default engine. DJLS then reads
    // settings that list only the modules Django imported as libraries: a module DJLS cannot
    // read as a library would otherwise open the catalog and hide unknown-name diagnostics.
    // Installing each library's app lets libraries that import models load. If the project's
    // apps cannot all start together, retry with the configured apps only.
    let candidate_apps: Vec<String> = target
        .apps
        .iter()
        .cloned()
        .chain(requested.values().filter_map(|module| {
            module
                .split_once(".templatetags.")
                .map(|(app, _)| app.to_string())
        }))
        .fold(Vec::new(), |mut apps, app| {
            if !apps.contains(&app) {
                apps.push(app);
            }
            apps
        });
    let django_settings_dir = scratch_root.join("django-settings");
    let mut apps = candidate_apps.clone();
    let mut answers = match answers {
        Some(answers) => answers,
        None => loop {
            write_settings(&django_settings_dir, &BTreeMap::new(), &apps)?;
            let request = django::Request {
                settings_module: SYNTHETIC_SETTINGS_MODULE.to_string(),
                sys_path: python_path(target, Some(&django_settings_dir)),
                library_modules: requested.clone(),
                template_names: Vec::new(),
                include_django: options.include_django,
                backend: options.backend.clone(),
                ..django::Request::default()
            };
            let answers = django::run(target, &request, options.timeout)?;
            match &answers.setup_error {
                None => break answers,
                Some(error) if apps.len() > target.apps.len() => {
                    notes.push(format!(
                        "Django could not start with every library's app installed; retried with configured apps only. {}",
                        error.lines().last().unwrap_or_default()
                    ));
                    apps.clone_from(&target.apps);
                }
                Some(error) => anyhow::bail!(
                    "Django could not set up synthetic settings for {}: {error}",
                    target.name
                ),
            }
        },
    };
    if answers
        .templates
        .as_ref()
        .is_some_and(|templates| templates.truncated)
    {
        notes.push("Template resolution compared only the first 5000 Template Names.".to_string());
    }
    let (valid, invalid): (BTreeMap<_, _>, BTreeMap<_, _>) = requested
        .iter()
        .map(|(load_name, module)| (load_name.clone(), module.clone()))
        .partition(|(_, module)| !answers.library_errors.contains_key(module));
    let mut findings = Vec::new();
    let not_libraries: BTreeMap<String, String> = invalid
        .into_iter()
        .filter(|(_, module)| {
            answers.library_errors.get(module).map(|error| error.kind)
                == Some(django::LibraryErrorKind::NotALibrary)
        })
        .collect();
    if !not_libraries.is_empty() {
        let dir = scratch_root.join("not-libraries");
        write_settings(&dir, &not_libraries, &apps)?;
        let db = oracle_database(target, Some(SYNTHETIC_SETTINGS_MODULE), Some(&dir), &[])?;
        let project = db.project().context("oracle database has no project")?;
        findings.extend(compare::compare_not_libraries(
            &db,
            project,
            &answers,
            &not_libraries,
        ));
    }
    findings.extend(compare::import_failures(&answers, &requested));

    let mut probes = compare::probe_files(&answers, &scratch_root.join("probes"));
    if let Some(seeds) = &options.seeds {
        probes.extend(compare::seed_files(seeds)?);
    }
    // A new Python process replays every candidate with unmodified Django.
    let replay_request = django::Request {
        settings_module: if mode == SettingsMode::Project {
            target
                .settings_module
                .clone()
                .context("project settings missing")?
        } else {
            SYNTHETIC_SETTINGS_MODULE.to_string()
        },
        sys_path: python_path(
            target,
            (mode == SettingsMode::Synthetic).then_some(django_settings_dir.as_path()),
        ),
        library_modules: if mode == SettingsMode::Synthetic {
            requested.clone()
        } else {
            BTreeMap::new()
        },
        backend: options.backend.clone(),
        replay_templates: Some(probes.iter().map(|probe| probe.template.clone()).collect()),
        ..django::Request::default()
    };
    let native = django::run(target, &replay_request, options.timeout)?;
    anyhow::ensure!(
        native.setup_error.is_none(),
        "native replay setup failed: {:?}",
        native.setup_error
    );
    for probe in &mut probes {
        let verdict = native
            .native_probes
            .get(&probe.template)
            .context("missing native replay verdict")?;
        probe.django = verdict.django;
        probe.error.clone_from(&verdict.error);
    }
    for library in &mut answers.libraries {
        for (name, tag) in &mut library.tags {
            let mut differences = Vec::new();
            for probe in &mut tag.probes {
                let verdict = native
                    .native_probes
                    .get(&probe.template)
                    .context("missing tag verdict")?;
                if probe.django != verdict.django {
                    differences.push(format!(
                        "{}: instrumented {:?}; native {:?}: {}",
                        probe.template,
                        probe.django,
                        verdict.django,
                        verdict.error.as_deref().unwrap_or("compiled")
                    ));
                }
                *probe = verdict.clone();
            }
            if !differences.is_empty() {
                tag.min_compiled_bits = None;
                tag.block = None;
                let mut finding = report::Finding::new(
                    report::FindingClass::Unchecked,
                    report::Family::ReferenceIntegrity,
                    Some(&library.module),
                    Some(name),
                    "instrumented and native compilation differ",
                    "argument and block facts withheld",
                    "Parser instrumentation changes this compile function's behavior; native verdicts still drive diagnostic comparisons.",
                );
                write!(finding.key, ":load={:?}", library.load_name)?;
                finding.evidence = differences;
                findings.push(finding);
            }
        }
    }
    let djls_settings_dir = scratch_root.join("djls-settings");
    let (settings_module, synthetic) = match mode {
        SettingsMode::Project => (target.settings_module.as_deref(), None),
        SettingsMode::Synthetic => {
            write_settings(&djls_settings_dir, &valid, &apps)?;
            (
                Some(SYNTHETIC_SETTINGS_MODULE),
                Some(djls_settings_dir.as_path()),
            )
        }
    };
    let db = oracle_database(target, settings_module, synthetic, &probes)?;
    let project = db.project().context("oracle database has no project")?;
    let mut comparison = compare::compare(&db, project, &answers, mode, &probes)?;
    comparison.findings.extend(findings);
    for (load_name, module) in &requested {
        comparison
            .coverage
            .subjects
            .insert(format!("candidate:{load_name}:{module}"));
    }
    for note in &notes {
        comparison.findings.push(report::Finding::new(
            report::FindingClass::Unchecked,
            report::Family::LibraryLoad,
            None,
            Some("setup-coverage"),
            note,
            "not compared in the original setup",
            "The reduced setup or bounded exploration needs explicit accounting.",
        ));
    }

    let mut report = OracleReport::new(
        target.name.clone(),
        mode,
        answers.django_version.clone(),
        notes,
        comparison,
    );
    report.backend = answers
        .backend
        .context("Django did not identify its backend")?;
    report.python_version = answers.python_version;
    // These replacements affect paths only, never the compiler's verdict or static facts.
    let replacements = [
        (scratch_root.as_str(), "${SCRATCH}"),
        (target.environment_root.as_str(), "${ENVIRONMENT}"),
        (target.project_root.as_str(), "${PROJECT}"),
        (env!("CARGO_MANIFEST_DIR"), "${TESTING}"),
    ];
    let mut json = serde_json::to_string(&report)?;
    for (path, placeholder) in replacements {
        json = json.replace(path, placeholder);
    }
    Ok(serde_json::from_str(&json)?)
}

fn write_settings(
    dir: &Utf8Path,
    libraries: &BTreeMap<String, String>,
    apps: &[String],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join(format!("{SYNTHETIC_SETTINGS_MODULE}.py")),
        synthetic_settings(libraries, apps),
    )?;
    Ok(())
}

fn synthetic_settings(libraries: &BTreeMap<String, String>, apps: &[String]) -> String {
    let installed = apps
        .iter()
        .map(|app| format!("    {app:?},\n"))
        .collect::<Vec<_>>()
        .concat();
    let entries = libraries
        .iter()
        .map(|(load_name, module)| format!("            {load_name:?}: {module:?},\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        r#"# Generated by the DJLS Django oracle. Django and DJLS both read this file.
SECRET_KEY = "djls-oracle"
STATIC_URL = "/static/"
INSTALLED_APPS = [
    "django.contrib.auth",
    "django.contrib.contenttypes",
    "django.contrib.sessions",
    "django.contrib.messages",
    "django.contrib.staticfiles",
{installed}]
TEMPLATES = [
    {{
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": False,
        "OPTIONS": {{
            "libraries": {{
{entries}            }},
        }},
    }},
]
"#
    )
}

fn python_path(target: &OracleTarget, synthetic_dir: Option<&Utf8Path>) -> Vec<String> {
    synthetic_dir
        .into_iter()
        .map(Utf8Path::to_path_buf)
        .chain(std::iter::once(target.project_root.clone()))
        .chain(target.source_roots.iter().cloned())
        .map(String::from)
        .collect()
}

fn oracle_database(
    target: &OracleTarget,
    settings_module: Option<&str>,
    synthetic_dir: Option<&Utf8Path>,
    probes: &[compare::ProbeCase],
) -> anyhow::Result<OsTestDatabase> {
    let mut roots = target.disk_roots.clone();
    roots.push(target.environment_root.clone());
    roots.extend(synthetic_dir.map(Utf8Path::to_path_buf));
    let mut db = OsTestDatabase::with_disk_roots(roots);
    let mut fixture = ProjectFixture::new(target.project_root.clone())
        .python_environment(PythonEnvironment::Path(target.environment_root.clone()))
        .without_bundled_django();
    if let Some(settings_module) = settings_module {
        fixture = fixture.django_settings_module(settings_module);
    }
    if let Some(dir) = synthetic_dir {
        fixture = fixture.pythonpath(dir);
    }
    for root in &target.source_roots {
        fixture = fixture.pythonpath(root.clone());
    }
    for (key, value) in &target.env {
        fixture = fixture.env_var(key.clone(), value.clone());
    }
    // Probe templates are installed before the first query so adding thousands of them does not
    // invalidate the catalog once per file.
    for probe in probes {
        fixture = fixture.file(probe.path.clone(), probe.template.clone());
    }
    fixture.install(&mut db)?;
    Ok(db)
}

/// Build the target for a corpus repository from its manifest entry and ready environment.
pub fn corpus_target(corpus: &Corpus, name: &str) -> anyhow::Result<OracleTarget> {
    let project = corpus.environment_project(name)?;
    let library_files = corpus
        .extraction_target_members()?
        .into_iter()
        .filter(|target| target.member == name)
        .map(|target| target.path)
        .collect();
    let python = project.environment_root.join("bin/python");
    let (apps, env) = django::oracle_repo_config(name)?;
    Ok(OracleTarget {
        name: name.to_string(),
        project_root: project.project_root,
        disk_roots: vec![project.checkout_root],
        environment_root: project.environment_root,
        python,
        source_roots: project.source_roots,
        settings_module: project.django_settings_module,
        apps,
        env,
        library_files,
    })
}

/// Find Template Library sources under `root`: Python files in `templatetags` packages.
#[must_use]
pub fn discover_library_files(root: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "node_modules" | "site-packages" | "__pycache__"
            )
        })
        .build();
    for entry in walker.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.into_path()) else {
            continue;
        };
        let in_templatetags = path
            .parent()
            .and_then(Utf8Path::file_name)
            .is_some_and(|name| name == "templatetags");
        if in_templatetags
            && path.extension() == Some("py")
            && path.file_name() != Some("__init__.py")
        {
            files.push(path);
        }
    }
    files.sort();
    files
}
