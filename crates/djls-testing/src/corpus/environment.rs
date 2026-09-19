//! Explicit, per-repository Python environments. Source sync alone is not setup.

use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::process::Command;
use std::time::SystemTime;

use anyhow::Context as _;
use anyhow::ensure;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::PythonEnvironment;
use djls_project::PythonModuleName;
use djls_project::PythonSourceModule;
use djls_project::SearchPath;
use serde::Deserialize;
use serde::Serialize;

use crate::corpus::Corpus;
use crate::corpus::manifest::Manifest;
use crate::db::OsTestDatabase;
use crate::fixtures::ProjectFixture;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Environment {
    /// Compatibility exception when upstream declarations cannot select a usable interpreter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    python: Option<String>,
    /// Checkout-relative roots added to the project's normal search paths.
    #[serde(default = "default_source_roots")]
    source_roots: Vec<Utf8PathBuf>,
    dependencies: Dependencies,
    #[serde(default)]
    metadata_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Dependencies {
    /// Use the upstream lock directly, including its default dependency groups.
    UvLock,
    PoetryLock,
    /// Install upstream declarations without maintaining another dependency lock.
    Requirements {
        inputs: Vec<Utf8PathBuf>,
        /// Requirements declared outside pip inputs, such as a selected tox factor.
        #[serde(default)]
        supplemental: Vec<String>,
        /// Upstream wheels with packaging defects may need a source build.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        no_binary: Vec<String>,
    },
    Deferred {
        reason: String,
    },
}

fn default_source_roots() -> Vec<Utf8PathBuf> {
    vec![Utf8PathBuf::from(".")]
}

impl Environment {
    fn python_request(&self, checkout: &Utf8Path) -> anyhow::Result<String> {
        if let Some(python) = &self.python {
            return Ok(python.clone());
        }
        let version_file = checkout.join(".python-version");
        if version_file.exists() {
            let source = std::fs::read_to_string(&version_file)?;
            // pyenv permits multiple versions; the first is the preferred interpreter.
            return source
                .lines()
                .map(|line| line.split('#').next().unwrap_or_default().trim())
                .find(|line| !line.is_empty())
                .map(str::to_owned)
                .with_context(|| format!("no Python version in `{version_file}`"));
        }
        let pyproject = checkout.join("pyproject.toml");
        if pyproject.exists() {
            let source = std::fs::read_to_string(&pyproject)?;
            let metadata: toml::Value = toml::from_str(&source)?;
            // uv pip honors this target even with --python, so its environment
            // must match rather than receiving wheels for a different interpreter.
            let pip_target = metadata
                .get("tool")
                .and_then(|tool| tool.get("uv"))
                .and_then(|uv| uv.get("pip"))
                .and_then(|pip| pip.get("python-version"))
                .filter(|_| matches!(self.dependencies, Dependencies::Requirements { .. }));
            if let Some(requirement) = pip_target.or_else(|| {
                metadata
                    .get("project")
                    .and_then(|project| project.get("requires-python"))
            }) {
                return requirement
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
                    .with_context(|| format!("invalid Python request in `{pyproject}`"));
            }
        }
        Ok("3.12".to_string())
    }

    pub(crate) fn validate(&self, name: &str) -> anyhow::Result<()> {
        ensure!(
            self.python
                .as_ref()
                .is_none_or(|python| !python.trim().is_empty())
                && !self.source_roots.is_empty(),
            "corpus environment `{name}` needs Python and source roots"
        );
        let inputs = match &self.dependencies {
            Dependencies::UvLock | Dependencies::PoetryLock | Dependencies::Deferred { .. } => {
                &[][..]
            }
            Dependencies::Requirements { inputs, .. } => {
                ensure!(
                    !inputs.is_empty(),
                    "environment `{name}` needs dependency inputs"
                );
                inputs.as_slice()
            }
        };
        if let Dependencies::Deferred { reason } = &self.dependencies {
            ensure!(
                !reason.trim().is_empty(),
                "environment `{name}` needs a deferral reason"
            );
        }
        for path in self.source_roots.iter().chain(inputs) {
            ensure!(
                !path.as_str().is_empty()
                    && !path.is_absolute()
                    && !path.as_str().contains(['\\', ':'])
                    && path.components().all(|component| matches!(
                        component,
                        camino::Utf8Component::Normal(_) | camino::Utf8Component::CurDir
                    )),
                "environment `{name}` path `{path}` must stay within its checkout"
            );
        }
        Ok(())
    }
}

const ENVIRONMENT_POLICY_REVISION: &str = "3";

fn run(command: &mut Command) -> anyhow::Result<()> {
    let status = command
        .status()
        .context("failed to start environment setup command; install the project tools first")?;
    ensure!(
        status.success(),
        "environment setup command failed: {status}"
    );
    Ok(())
}

fn with_metadata(mut command: Command, version: Option<&str>) -> Command {
    if let Some(version) = version {
        command.env("SETUPTOOLS_SCM_PRETEND_VERSION", version);
    }
    command
}

fn command_output(command: &mut Command) -> anyhow::Result<String> {
    let output = command
        .output()
        .context("failed to start environment diagnostic command")?;
    ensure!(
        output.status.success(),
        "environment diagnostic command failed: {}",
        output.status
    );
    String::from_utf8(output.stdout).context("environment diagnostic output was not UTF-8")
}

fn write_provenance(
    root: &Utf8Path,
    python: &Utf8Path,
    revision: &str,
    environment: &Environment,
    python_request: &str,
) -> anyhow::Result<()> {
    let runtime: serde_json::Value = serde_json::from_str(&command_output(
        Command::new(python).args(["-c", "import json,platform,sys; print(json.dumps({'python': sys.version, 'platform': platform.platform()}))"]),
    )?)?;
    let packages: serde_json::Value = serde_json::from_str(&command_output(
        Command::new("uv")
            .args(["pip", "list", "--format", "json", "--python"])
            .arg(python),
    )?)?;
    // Read PEP 610 records directly: importlib.metadata and assignment expressions
    // are unavailable in the historical Python environments in this corpus.
    let direct_urls: serde_json::Value = serde_json::from_str(&command_output(
        Command::new(python).args([
            "-c",
            "import glob,json,os,sysconfig\nrecords = {}\nfor root in set([sysconfig.get_path('purelib'), sysconfig.get_path('platlib')]):\n for path in glob.glob(os.path.join(root, '*.dist-info', 'direct_url.json')):\n  with open(path) as source:\n   records[os.path.basename(os.path.dirname(path))] = json.load(source)\nprint(json.dumps(records))",
        ]),
    )?)?;
    let installer = command_output(Command::new("uv").arg("--version"))?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs();
    let provenance = serde_json::json!({
        "source_revision": revision,
        "recipe": environment,
        "python_request": python_request,
        "python": runtime["python"],
        "platform": runtime["platform"],
        "installer": installer.trim(),
        "timestamp_unix_seconds": timestamp,
        "installed_packages": packages,
        "direct_urls": direct_urls,
    });
    std::fs::write(
        root.join("provenance.json"),
        serde_json::to_vec_pretty(&provenance)?,
    )?;
    Ok(())
}

impl Corpus {
    fn environment_recipe(&self, name: &str) -> anyhow::Result<(Environment, String)> {
        let repo = self
            .lockfile
            .repos
            .iter()
            .find(|repo| repo.name == name)
            .with_context(|| format!("unknown corpus repository `{name}`"))?;
        let manifest = Manifest::load(&self.manifest_path)?;
        let environment = manifest
            .repos
            .iter()
            .find(|repo| repo.name == name)
            .and_then(|repo| repo.environment.clone())
            .with_context(|| format!("corpus repository `{name}` has no environment recipe"))?;
        let checkout = self.root.join("repos").join(name);
        for root in &environment.source_roots {
            ensure!(
                checkout.join(root).is_dir(),
                "corpus `{name}` source root `{root}` is missing"
            );
        }
        Ok((environment, repo.git_ref.clone()))
    }

    pub fn environment_deferral(&self, name: &str) -> anyhow::Result<Option<String>> {
        let (environment, _) = self.environment_recipe(name)?;
        Ok(match environment.dependencies {
            Dependencies::Deferred { reason } => Some(reason),
            Dependencies::UvLock | Dependencies::PoetryLock | Dependencies::Requirements { .. } => {
                None
            }
        })
    }

    fn environment_stamp(&self, name: &str) -> anyhow::Result<String> {
        let (environment, revision) = self.environment_recipe(name)?;
        let checkout = self.root.join("repos").join(name);
        let lock = match &environment.dependencies {
            Dependencies::UvLock => Some(checkout.join("uv.lock")),
            Dependencies::PoetryLock => Some(checkout.join("poetry.lock")),
            Dependencies::Requirements { .. } | Dependencies::Deferred { .. } => None,
        };
        // This is only a disposable local cache key, not dependency integrity data.
        // uv owns lock parsing, compatibility checks, and artifact verification.
        let mut stamp = DefaultHasher::new();
        revision.hash(&mut stamp);
        toml::to_string(&environment)?.hash(&mut stamp);
        environment.python_request(&checkout)?.hash(&mut stamp);
        ENVIRONMENT_POLICY_REVISION.hash(&mut stamp);
        if let Some(path) = lock {
            std::fs::read(&path)
                .with_context(|| format!("missing upstream dependency lock `{path}` for `{name}`"))?
                .hash(&mut stamp);
        }
        Ok(format!("{:016x}\n", stamp.finish()))
    }

    /// Prepare missing/stale environments; refresh explicitly re-resolves dependencies.
    pub fn sync_environment(&self, name: &str, refresh: bool) -> anyhow::Result<()> {
        let (environment, revision) = self.environment_recipe(name)?;
        if let Dependencies::Deferred { reason } = &environment.dependencies {
            anyhow::bail!("corpus environment `{name}` is deferred: {reason}");
        }
        if !refresh && self.environment_database(name).is_ok() {
            tracing::info!(%name, "environment already ready");
            return Ok(());
        }
        let stamp = self.environment_stamp(name)?;
        let root = self.root.join("environments").join(name);
        let ready = root.join(".ready");
        if ready.exists() {
            std::fs::remove_file(&ready)?;
        }
        let checkout = self.root.join("repos").join(name);
        let python_request = environment.python_request(&checkout)?;
        // tools/corpus-python.sh supplies EOL interpreters absent from uv downloads.
        if cfg!(target_os = "linux") && matches!(python_request.as_str(), "3.6" | "3.7") {
            run(Command::new("bash")
                .arg(Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/corpus-python.sh"))
                .arg(self.root.join("interpreters")))?;
        }
        let local_python = self
            .root
            .join("interpreters")
            .join(&python_request)
            .join("bin")
            .join(format!("python{python_request}"));
        let interpreter = if local_python.is_file() {
            local_python.as_str()
        } else {
            &python_request
        };
        // This directory contains only disposable, corpus-managed environments.
        run(Command::new("uv")
            .args(["venv", "--no-project", "--clear", "--python", interpreter])
            .arg(&root))?;
        let python = root.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        let metadata_version = environment.metadata_version.as_deref();
        match &environment.dependencies {
            Dependencies::UvLock => run(with_metadata(Command::new("uv"), metadata_version)
                .current_dir(&checkout)
                .env("UV_PROJECT_ENVIRONMENT", &root)
                .args([
                    "sync",
                    "--frozen",
                    "--no-install-project",
                    "--python",
                    interpreter,
                ]))?,
            Dependencies::PoetryLock => run(with_metadata(Command::new("uv"), metadata_version)
                .current_dir(&checkout)
                .env("VIRTUAL_ENV", &root)
                .env("POETRY_VIRTUALENVS_CREATE", "false")
                .args([
                    "tool",
                    "run",
                    "--from",
                    "poetry==2.2.1",
                    "poetry",
                    "sync",
                    "--no-root",
                    "--no-interaction",
                ]))?,
            Dependencies::Requirements {
                inputs,
                supplemental,
                no_binary,
            } => {
                let mut command = with_metadata(Command::new("uv"), metadata_version);
                command
                    .current_dir(&checkout)
                    .args(["pip", "install", "--refresh", "--python"])
                    .arg(&python);
                for input in inputs {
                    ensure!(
                        checkout.join(input).is_file(),
                        "corpus `{name}` dependency input `{input}` is missing"
                    );
                    command.arg("-r").arg(input);
                }
                command.args(supplemental);
                for package in no_binary {
                    command.arg("--no-binary").arg(package);
                }
                run(&mut command)?;
            }
            Dependencies::Deferred { reason } => {
                anyhow::bail!("corpus environment `{name}` is deferred: {reason}");
            }
        }
        run(Command::new("uv")
            .args(["pip", "check", "--python"])
            .arg(&python))?;
        write_provenance(&root, &python, &revision, &environment, &python_request)?;
        std::fs::write(ready, stamp)?;
        Ok(())
    }

    /// A missing, partial, or stale setup is an error, never an empty environment.
    fn require_environment(&self, name: &str) -> anyhow::Result<Utf8PathBuf> {
        let stamp = self.environment_stamp(name)?;
        let root = self.root.join("environments").join(name);
        let ready = std::fs::read_to_string(root.join(".ready"))
            .with_context(|| format!("corpus environment `{name}` is not ready; run `just corpus environment sync {name}`"))?;
        ensure!(
            ready == stamp
                && root.join("pyvenv.cfg").is_file()
                && root
                    .join(if cfg!(windows) {
                        "Scripts/python.exe"
                    } else {
                        "bin/python"
                    })
                    .is_file(),
            "corpus environment `{name}` is stale; run `just corpus environment sync {name}`"
        );
        Ok(root)
    }

    /// Use the checkout and its own environment, with no ambient Python fallback.
    pub fn environment_database(&self, name: &str) -> anyhow::Result<OsTestDatabase> {
        if let Some(reason) = self.environment_deferral(name)? {
            anyhow::bail!("corpus environment `{name}` is deferred: {reason}");
        }
        let root = self.require_environment(name)?;
        let (environment, _) = self.environment_recipe(name)?;
        let checkout = self.root.join("repos").join(name);
        let mut db = OsTestDatabase::with_disk_roots([checkout.clone(), root.clone()]);
        let manifest = Manifest::load(&self.manifest_path)?;
        let declaration = manifest
            .repo_settings_projects()
            .into_iter()
            .find(|project| project.repo_name == name);
        let project_root = declaration
            .as_ref()
            .and_then(|project| project.relative_root)
            .map_or_else(|| checkout.clone(), |relative| checkout.join(relative));
        let mut fixture =
            ProjectFixture::new(project_root).python_environment(PythonEnvironment::Path(root));
        if let Some(declaration) = declaration {
            ensure!(
                declaration.django_settings_modules.len() == 1,
                "corpus environment `{name}` needs an explicit settings selection for its multiple projects"
            );
            fixture = fixture.django_settings_module(declaration.django_settings_modules[0]);
        }
        for source_root in environment.source_roots {
            fixture = fixture.pythonpath(checkout.join(source_root));
        }
        let project = fixture.install(&mut db)?;
        ensure!(
            project
                .search_paths(&db)
                .iter()
                .any(|path| matches!(path, SearchPath::SitePackages(_))),
            "corpus environment `{name}` has no discoverable site-packages; run `just corpus environment sync {name}`"
        );
        ensure!(
            PythonSourceModule::resolve(&db, project, PythonModuleName::parse("django")?,)
                .is_some(),
            "corpus environment `{name}` cannot resolve Django; review its upstream dependency inputs"
        );
        Ok(db)
    }
}

#[cfg(test)]
mod tests {
    use djls_project::Db as _;

    use super::*;
    use crate::corpus::lock::LockedRepo;
    use crate::corpus::lock::Lockfile;

    fn environment_fixture() -> (tempfile::TempDir, Corpus) {
        let directory = tempfile::tempdir().expect("temporary corpus");
        let base = Utf8PathBuf::from_path_buf(directory.path().to_owned()).expect("UTF-8 path");
        let corpus = Corpus {
            root: base.join(".corpus"),
            manifest_path: base.join("manifest.toml"),
            lockfile: Lockfile {
                repos: vec![LockedRepo {
                    name: "example".into(),
                    url: "https://example.com/example.git".into(),
                    tag: "v1".into(),
                    git_ref: "locked-revision".into(),
                }],
            },
        };
        std::fs::create_dir_all(corpus.root.join("repos/example/src")).expect("source root");
        std::fs::write(&corpus.manifest_path, "[corpus]\nroot_dir = '.corpus'\n[[repo]]\nname = 'example'\nurl = 'https://example.com/example.git'\nenvironment = { python = '3.12.14', source_roots = ['src'], dependencies = { kind = 'uv_lock' } }\n")
            .expect("manifest");
        let lock_path = corpus.root.join("repos/example/uv.lock");
        std::fs::write(lock_path, "version = 1\nrequires-python = '>=3.12'\n")
            .expect("upstream lock fixture");
        let root = corpus.root.join("environments/example");
        let executable = root.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        std::fs::create_dir_all(executable.parent().expect("parent"))
            .expect("environment directory");
        std::fs::write(executable, "").expect("interpreter placeholder");
        std::fs::write(root.join("pyvenv.cfg"), "version = 3.12.14\n").expect("venv metadata");
        std::fs::write(
            root.join(".ready"),
            corpus.environment_stamp("example").expect("stamp"),
        )
        .expect("ready state");
        (directory, corpus)
    }

    #[test]
    fn native_lock_bytes_participate_in_stamp() {
        let (directory, corpus) = environment_fixture();
        for (kind, filename) in [("uv_lock", "uv.lock"), ("poetry_lock", "poetry.lock")] {
            std::fs::write(&corpus.manifest_path, format!(
                "[corpus]\nroot_dir = '.corpus'\n[[repo]]\nname = 'example'\nurl = 'https://example.com/example.git'\nenvironment = {{ python = '3.12.14', source_roots = ['src'], dependencies = {{ kind = '{kind}' }} }}\n"
            )).expect("native lock recipe");
            let lock = corpus.root.join("repos/example").join(filename);
            assert_eq!(lock.file_name(), Some(filename));
            std::fs::write(&lock, "upstream lock contents\n").expect("lock fixture");
            let before = corpus.environment_stamp("example").expect("stamp");
            std::fs::write(&lock, "changed opaque lock contents\n").expect("lock fixture");
            assert_ne!(corpus.environment_stamp("example").expect("stamp"), before);
            std::fs::remove_file(lock).expect("remove upstream lock");
            assert!(corpus.environment_stamp("example").is_err());
        }
        assert!(!directory.path().join("environment-locks").exists());
    }

    #[test]
    fn readiness_rejects_missing_partial_and_stale_environments() {
        let (_directory, corpus) = environment_fixture();
        let root = corpus
            .require_environment("example")
            .expect("matching ready environment");
        assert!(corpus.require_environment("unknown").is_err());
        let ready_path = root.join(".ready");
        let ready = std::fs::read_to_string(&ready_path).expect("ready state");
        std::fs::remove_file(&ready_path).expect("simulate failed provisioning");
        assert!(
            corpus
                .require_environment("example")
                .expect_err("partial provisioning must fail")
                .to_string()
                .contains("not ready")
        );
        std::fs::write(&ready_path, "older lock contents").expect("stale state");
        assert!(
            corpus
                .require_environment("example")
                .expect_err("stale provisioning must fail")
                .to_string()
                .contains("stale")
        );
        std::fs::write(&ready_path, ready).expect("restore state");
        std::fs::remove_file(root.join("pyvenv.cfg")).expect("remove environment metadata");
        assert!(corpus.require_environment("example").is_err());
    }

    #[test]
    fn changing_recipe_source_revision_or_lock_requires_resync() {
        let (directory, mut corpus) = environment_fixture();
        corpus.lockfile.repos[0].git_ref = "new-revision".into();
        assert!(
            corpus
                .require_environment("example")
                .expect_err("new source revision must require sync")
                .to_string()
                .contains("is stale")
        );
        corpus.lockfile.repos[0].git_ref = "locked-revision".into();
        let path = directory.path().join("manifest.toml");
        let source = std::fs::read_to_string(&path).expect("recipe");
        std::fs::write(&path, source.replace("3.12.14", "3.13.15")).expect("change interpreter");
        assert!(
            corpus
                .require_environment("example")
                .expect_err("new recipe must require sync")
                .to_string()
                .contains("is stale")
        );
        std::fs::write(path, source).expect("restore recipe");
        let lock = corpus.root.join("repos/example/uv.lock");
        std::fs::write(&lock, "version = 1\nrequires-python = '>=3.13'\n")
            .expect("change upstream lock");
        assert!(
            corpus
                .require_environment("example")
                .expect_err("changed lock must require sync")
                .to_string()
                .contains("is stale")
        );
        std::fs::remove_file(lock).expect("remove lock");
        assert!(
            corpus
                .require_environment("example")
                .expect_err("missing lock must fail")
                .to_string()
                .contains("missing upstream dependency lock")
        );
    }

    #[test]
    fn database_resolves_checkout_and_its_own_dependencies() {
        let (_directory, corpus) = environment_fixture();
        let root = corpus
            .require_environment("example")
            .expect("ready environment");
        let site_packages = root.join(if cfg!(windows) {
            "Lib/site-packages"
        } else {
            "lib/python3.12/site-packages"
        });
        std::fs::create_dir_all(&site_packages).expect("site-packages");
        let dependency = site_packages.join("dependency.py");
        let local = corpus.root.join("repos/example/src/local.py");
        std::fs::write(&dependency, "").expect("dependency source");
        std::fs::write(&local, "").expect("project source");
        // A neighboring environment must never leak into this one.
        let neighbor = corpus
            .root
            .join("environments/other/lib/python3.12/site-packages");
        std::fs::create_dir_all(&neighbor).expect("other environment");
        std::fs::write(neighbor.join("other_dependency.py"), "").expect("other source");
        std::fs::write(neighbor.join("django.py"), "").expect("other Django source");
        assert!(
            corpus
                .environment_database("example")
                .err()
                .expect("an environment without Django must fail")
                .to_string()
                .contains("cannot resolve Django")
        );
        std::fs::write(site_packages.join("django.py"), "").expect("own Django source");
        let db = corpus
            .environment_database("example")
            .expect("project database");
        let project = db.project().expect("installed project");
        for (name, path) in [("local", local), ("dependency", dependency)] {
            let module = PythonSourceModule::resolve(
                &db,
                project,
                PythonModuleName::parse(name).expect("valid name"),
            )
            .expect("module resolves");
            assert_eq!(module.file().path(&db), &path);
        }
        assert!(
            PythonSourceModule::resolve(
                &db,
                project,
                PythonModuleName::parse("other_dependency").expect("valid name")
            )
            .is_none()
        );
        // Django's own checkout must work without a second installed copy.
        std::fs::rename(
            site_packages.join("django.py"),
            corpus.root.join("repos/example/src/django.py"),
        )
        .expect("move Django into checkout");
        assert!(corpus.environment_database("example").is_ok());
        // The placeholder interpreter cannot run. Reusing this complete fixture
        // must not invoke uv, rewrite metadata, or remove installed source.
        let ready = std::fs::read(root.join(".ready")).expect("readiness stamp");
        let metadata = std::fs::read(root.join("pyvenv.cfg")).expect("venv metadata");
        corpus
            .sync_environment("example", false)
            .expect("reuse ready environment");
        assert_eq!(
            std::fs::read(root.join(".ready")).expect("stamp remains"),
            ready
        );
        assert_eq!(
            std::fs::read(root.join("pyvenv.cfg")).expect("metadata remains"),
            metadata
        );
        assert!(site_packages.join("dependency.py").is_file());
    }

    #[test]
    fn default_environment_recipes_parse() {
        let environment: Environment = toml::from_str(
            "dependencies = { kind = 'requirements', inputs = ['requirements.txt'] }",
        )
        .expect("valid recipe");
        assert_eq!(environment.python, None);
        assert_eq!(environment.source_roots, [Utf8PathBuf::from(".")]);
    }

    #[test]
    fn python_selection_follows_upstream_unless_explicitly_overridden() {
        let (_directory, corpus) = environment_fixture();
        let (mut environment, _) = corpus.environment_recipe("example").expect("recipe");
        environment.python = None;
        let checkout = corpus.root.join("repos/example");
        assert_eq!(
            environment.python_request(&checkout).expect("fallback"),
            "3.12"
        );
        std::fs::write(
            checkout.join("pyproject.toml"),
            "[project]\nrequires-python = '>=3.13,<3.14'\n",
        )
        .expect("metadata");
        assert_eq!(
            environment
                .python_request(&checkout)
                .expect("metadata request"),
            ">=3.13,<3.14"
        );
        environment.dependencies = Dependencies::Requirements {
            inputs: vec!["pyproject.toml".into()],
            supplemental: Vec::new(),
            no_binary: Vec::new(),
        };
        std::fs::write(
            checkout.join("pyproject.toml"),
            "[project]\nrequires-python = '>=3.11'\n[tool.uv.pip]\npython-version = '3.11'\n",
        )
        .expect("pip target");
        assert_eq!(
            environment.python_request(&checkout).expect("pip target"),
            "3.11"
        );
        std::fs::write(
            checkout.join(".python-version"),
            "# preferred runtime\n\n3.13.1 # primary\n3.12.7\n",
        )
        .expect("version file");
        assert_eq!(
            environment
                .python_request(&checkout)
                .expect("preferred version"),
            "3.13.1"
        );
        environment.python = Some("3.11".into());
        assert_eq!(
            environment.python_request(&checkout).expect("override"),
            "3.11"
        );
        environment.python = None;
        std::fs::write(checkout.join(".python-version"), "# no version\n")
            .expect("empty version file");
        assert!(environment.python_request(&checkout).is_err());
        std::fs::remove_file(checkout.join(".python-version")).expect("remove version file");
        std::fs::write(
            checkout.join("pyproject.toml"),
            "[project]\nrequires-python = 313\n",
        )
        .expect("invalid metadata");
        assert!(environment.python_request(&checkout).is_err());
    }

    #[test]
    fn upstream_python_selection_changes_invalidate_readiness() {
        let (_directory, corpus) = environment_fixture();
        let source = std::fs::read_to_string(&corpus.manifest_path).expect("manifest");
        std::fs::write(
            &corpus.manifest_path,
            source.replace("python = '3.12.14', ", ""),
        )
        .expect("remove override");
        let ready = corpus.root.join("environments/example/.ready");
        std::fs::write(
            &ready,
            corpus.environment_stamp("example").expect("fallback stamp"),
        )
        .expect("ready state");
        assert!(corpus.require_environment("example").is_ok());
        let checkout = corpus.root.join("repos/example");
        std::fs::write(
            checkout.join("pyproject.toml"),
            "[project]\nrequires-python = '>=3.13'\n",
        )
        .expect("metadata");
        assert!(corpus.require_environment("example").is_err());
        std::fs::write(
            &ready,
            corpus.environment_stamp("example").expect("metadata stamp"),
        )
        .expect("ready state");
        assert!(corpus.require_environment("example").is_ok());
        std::fs::write(checkout.join(".python-version"), "3.13.1\n").expect("preferred version");
        assert!(corpus.require_environment("example").is_err());
    }

    #[test]
    fn environment_recipes_require_explicit_inputs_and_contained_paths() {
        for (roots, inputs, valid) in [
            ("['src']", "['pyproject.toml']", true),
            ("['.']", "['requirements/base.txt']", true),
            ("[]", "['requirements.txt']", false),
            ("['.']", "[]", false),
            ("['../outside']", "['requirements.txt']", false),
            ("['.']", "['/absolute']", false),
        ] {
            let recipe: Environment = toml::from_str(&format!("python = '3.12'\nsource_roots = {roots}\ndependencies = {{ kind = 'requirements', inputs = {inputs} }}\n")).expect("decode recipe");
            assert_eq!(
                recipe.validate("example").is_ok(),
                valid,
                "{roots}: {inputs}"
            );
        }
    }

    #[test]
    fn deferred_environment_is_visible_and_unusable() {
        let (_directory, corpus) = environment_fixture();
        let source = std::fs::read_to_string(&corpus.manifest_path).expect("manifest");
        std::fs::write(
            &corpus.manifest_path,
            source.replace(
                "{ kind = 'uv_lock' }",
                "{ kind = 'deferred', reason = 'unsupported build' }",
            ),
        )
        .expect("manifest");
        assert_eq!(
            corpus
                .environment_deferral("example")
                .expect("deferral")
                .as_deref(),
            Some("unsupported build")
        );
        assert!(corpus.environment_database("example").is_err());
    }
}
