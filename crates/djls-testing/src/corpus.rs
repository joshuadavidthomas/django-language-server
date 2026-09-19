//! Corpus of real-world Django projects for grounding tests in reality.
//!
//! This crate is the **single source of truth** for real Python source code
//! and Django templates used across the test suite. It syncs pinned versions
//! of Django, popular third-party libraries, and open-source Django projects
//! as git repos, then provides helpers to enumerate and locate files within them.
//!
//! **All tests that analyze Python source (extraction rules, registrations,
//! filter arities, block specs) should use corpus files, not fabricated
//! snippets.** Template parser tests may use synthetic templates since
//! that's what users type, but extraction tests must be grounded in code
//! that real projects actually ship.
//!
//! # Usage
//!
//! ```no_run
//! use djls_testing::Corpus;
//!
//! let corpus = Corpus::require()?;
//! let django = corpus
//!     .latest_package("django")
//!     .ok_or_else(|| anyhow::anyhow!("Django is missing from the synced corpus"))?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! # Consumers
//!
//! - `djls-semantic` — golden tests: extract rules from real templatetag
//!   modules and snapshot the results
//! - `djls-server` — integration tests: parse real templates, validate
//!   against extracted rules, assert zero false positives

use anyhow::Context as _;
use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use ignore::WalkBuilder;

pub(crate) mod archive;
pub mod census;
mod environment;
mod lock;
mod manifest;
mod sync;

pub use lock::LockFilter;
pub use lock::Lockfile;
pub use lock::lock_corpus;
pub use manifest::Manifest;
pub use sync::clean_entries;
pub use sync::sync_corpus;

const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.corpus");
const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.toml");

/// A validated corpus root directory.
///
/// Constructed via [`Corpus::require`], which validates that the
/// directory exists. Once constructed, the root path is trusted for
/// the lifetime of the value.
pub struct Corpus {
    root: Utf8PathBuf,
    manifest_path: Utf8PathBuf,
    lockfile: lock::Lockfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusExtractionTarget {
    pub member: String,
    pub relative_path: Utf8PathBuf,
    pub path: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusSettingsProject {
    pub repo_name: String,
    pub checkout_root: Utf8PathBuf,
    pub project_root: Utf8PathBuf,
    pub django_settings_modules: Vec<String>,
}

fn lock_entry_matches_declaration(
    locked_repo: &lock::LockedRepo,
    declaration: &manifest::RepoSettingsProject<'_>,
) -> bool {
    locked_repo.name == declaration.repo_name
        && locked_repo.url == declaration.repo_url
        && declaration
            .repo_ref
            .is_none_or(|repo_ref| locked_repo.tag == repo_ref)
}

fn has_directory_component(path: &Utf8Path, names: &[&str]) -> bool {
    path.parent().is_some_and(|parent| {
        parent
            .components()
            .any(|component| names.contains(&component.as_str()))
    })
}

impl Corpus {
    /// Check whether the corpus directory exists.
    #[must_use]
    pub fn is_available() -> bool {
        Utf8Path::new(CORPUS_DIR).as_std_path().exists()
    }

    /// Get the default corpus after checking it against the lockfile.
    pub fn require() -> anyhow::Result<Self> {
        Self::require_from_manifest(Utf8Path::new(MANIFEST_PATH))
    }

    /// Get the corpus described by `manifest_path` after checking its lockfile.
    pub fn require_from_manifest(manifest_path: &Utf8Path) -> anyhow::Result<Self> {
        let manifest = Manifest::load(manifest_path)
            .with_context(|| format!("corpus manifest `{manifest_path}` is missing or invalid"))?;
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("corpus manifest `{manifest_path}` has no parent"))?;
        let root = manifest.corpus_root(manifest_dir);
        if !root.as_std_path().is_dir() {
            anyhow::bail!(
                "Corpus not synced at `{root}`. Run: cargo run -p djls-testing --bin corpus -- --manifest {manifest_path} sync"
            );
        }
        let lockfile_path = manifest_path.with_extension("lock");
        let lockfile = lock::Lockfile::load(&lockfile_path)
            .with_context(|| format!("corpus lockfile `{lockfile_path}` is missing or invalid"))?;
        let corpus = Self {
            root,
            manifest_path: manifest_path.to_owned(),
            lockfile,
        };
        sync::validate_synced_corpus(&corpus.lockfile, corpus.root())?;
        Ok(corpus)
    }

    /// The corpus root directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Locked corpus repositories in lockfile order, as names and directories.
    pub fn locked_repos(&self) -> impl Iterator<Item = (&str, Utf8PathBuf)> + '_ {
        self.lockfile
            .repos
            .iter()
            .map(|repo| (repo.name.as_str(), self.root.join("repos").join(&repo.name)))
    }

    fn locked_repo_dirs(&self) -> impl Iterator<Item = Utf8PathBuf> + '_ {
        self.locked_repos().map(|(_, directory)| directory)
    }

    pub fn repo_settings_projects(&self) -> anyhow::Result<Vec<CorpusSettingsProject>> {
        let manifest = Manifest::load(&self.manifest_path)?;
        manifest
            .repo_settings_projects()
            .into_iter()
            .map(|declaration| {
                let mut lock_matches = self
                    .lockfile
                    .repos
                    .iter()
                    .filter(|repo| repo.name == declaration.repo_name);
                let locked_repo = lock_matches.next().ok_or_else(|| {
                    anyhow::anyhow!(
                        "corpus repo `{}` has settings metadata but no lock entry; run `just corpus lock`",
                        declaration.repo_name
                    )
                })?;
                anyhow::ensure!(
                    lock_matches.next().is_none(),
                    "corpus repo `{}` has duplicate lock entries; run `just corpus lock`",
                    declaration.repo_name
                );
                anyhow::ensure!(
                    lock_entry_matches_declaration(locked_repo, &declaration),
                    "corpus repo `{}` identity differs between manifest.toml and manifest.lock; run `just corpus lock`",
                    declaration.repo_name
                );

                let checkout_root = self
                    .root()
                    .join("repos")
                    .join(declaration.repo_name);
                let project_root = declaration.relative_root.map_or_else(
                    || checkout_root.clone(),
                    |relative_root| checkout_root.join(relative_root),
                );
                if !project_root.as_std_path().is_dir() {
                    anyhow::bail!(
                        "corpus repo `{}` project root `{}` does not exist",
                        declaration.repo_name,
                        declaration
                            .relative_root
                            .map_or(".", Utf8Path::as_str)
                    );
                }
                Ok(CorpusSettingsProject {
                    repo_name: declaration.repo_name.to_string(),
                    checkout_root,
                    project_root,
                    django_settings_modules: declaration
                        .django_settings_modules
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                })
            })
            .collect()
    }

    /// Latest locked and synced version directory for a package under `repos/`.
    ///
    /// Handles both single-entry names (e.g. `repos/django-allauth/`)
    /// and multi-version names (e.g. `repos/django-6.0/`).
    #[must_use]
    pub fn latest_package(&self, name: &str) -> Option<Utf8PathBuf> {
        // Single-version: repos/{name}/. Only lockfile entries are eligible so
        // stale or manually synced directories cannot change corpus behavior.
        if let Some((_, exact)) = self.locked_repos().find(|(locked, directory)| {
            *locked == name && directory.join(".complete.json").as_std_path().exists()
        }) {
            return Some(exact);
        }

        // Multi-version: repos/{name}-{version}/ — find highest version
        let prefix = format!("{name}-");
        let mut best: Option<(Vec<u32>, Utf8PathBuf)> = None;
        for (locked, path) in self.locked_repos() {
            let Some(version_str) = locked.strip_prefix(&prefix) else {
                continue;
            };
            // The suffix after "{name}-" must start with a digit to be a
            // version, otherwise it's a different package (e.g. "django-cms"
            // should not match prefix "django-").
            if !version_str.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }

            if !path.join(".complete.json").as_std_path().exists() {
                continue;
            }
            let version: Option<Vec<u32>> = version_str
                .split('.')
                .map(|part| {
                    if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                        return None;
                    }
                    part.parse::<u32>().ok()
                })
                .collect();

            let Some(version) = version else {
                continue;
            };

            let should_replace = match &best {
                None => true,
                Some((best_version, _)) => version > *best_version,
            };

            if should_replace {
                best = Some((version, path));
            }
        }

        best.map(|(_, path)| path)
    }

    pub fn extraction_target_members(&self) -> anyhow::Result<Vec<CorpusExtractionTarget>> {
        let mut targets = Vec::new();
        for repo in &self.lockfile.repos {
            let member_root = self.root.join("repos").join(&repo.name);
            for path in Self::extraction_targets_in(&member_root) {
                let relative_path = path.strip_prefix(&member_root).with_context(|| {
                    format!(
                        "extraction target `{path}` escaped locked member `{}`",
                        repo.name
                    )
                })?;
                targets.push(CorpusExtractionTarget {
                    member: repo.name.clone(),
                    relative_path: relative_path
                        .components()
                        .map(|component| component.as_str())
                        .collect::<Vec<_>>()
                        .join("/")
                        .into(),
                    path,
                });
            }
        }
        targets.sort_by(|left, right| {
            (&left.member, &left.relative_path).cmp(&(&right.member, &right.relative_path))
        });
        Ok(targets)
    }

    /// Extraction target files under a specific directory.
    ///
    /// Matches `**/templatetags/**/*.py` (excluding `__init__.py`)
    /// and `**/template/{defaulttags,defaultfilters,loader_tags}.py`.
    #[must_use]
    pub(crate) fn extraction_targets_in(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();

        for entry in WalkBuilder::new(dir.as_std_path())
            .standard_filters(false)
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };

            if has_directory_component(relative, &["__pycache__"]) {
                continue;
            }

            let is_py = path.extension().is_some_and(|ext| ext == "py");
            let is_core_template_module = has_directory_component(relative, &["template"])
                && matches!(
                    path.file_name(),
                    Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
                );

            if is_py
                && path.file_name() != Some("__init__.py")
                && (has_directory_component(relative, &["templatetags"]) || is_core_template_module)
            {
                files.push(path.to_owned());
            }
        }

        files.sort();
        files
    }

    /// All model files in the entire corpus.
    #[must_use]
    pub fn model_files(&self) -> Vec<Utf8PathBuf> {
        let mut files = self
            .locked_repo_dirs()
            .flat_map(|dir| self.model_files_in(&dir))
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    /// Model files under a specific directory.
    ///
    /// Matches any `models.py` file. Excludes files inside `__pycache__`,
    /// `docs/`, `tests/`, and `test/` directories.
    #[must_use]
    pub fn model_files_in(&self, dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();

        for entry in WalkBuilder::new(dir.as_std_path())
            .standard_filters(false)
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };

            if path.file_name() == Some("models.py")
                && !has_directory_component(relative, &["__pycache__", "docs", "tests", "test"])
            {
                files.push(path.to_owned());
            }
        }

        files.sort();
        files
    }

    /// Template files under a specific directory.
    ///
    /// Matches any file under a `templates/` directory. Excludes files
    /// inside `docs/`, `tests/`, `jinja2/`, and `static/` directories.
    #[must_use]
    pub fn templates_in(&self, dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();

        for entry in WalkBuilder::new(dir.as_std_path())
            .standard_filters(false)
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };

            if has_directory_component(relative, &["templates"])
                && !has_directory_component(
                    relative,
                    &["__pycache__", "docs", "tests", "jinja2", "static"],
                )
            {
                files.push(path.to_owned());
            }
        }

        files.sort();
        files
    }
}

/// Derive a dotted Python module name from a file path within the corpus.
///
/// Handles the corpus layout where entries live under `repos/`:
/// - `.corpus/repos/{name}/{python_code}...`
///
/// Falls back to a heuristic for non-corpus paths (looks for version-like
/// path components).
///
/// # Examples
///
/// ```
/// # use camino::Utf8Path;
/// # use djls_testing::module_name_from_file;
/// let path = Utf8Path::new(".corpus/repos/django-6.0/django/template/defaulttags.py");
/// assert_eq!(module_name_from_file(path), "django.template.defaulttags");
///
/// let path = Utf8Path::new(".corpus/repos/sentry/sentry/templatetags/sentry_helpers.py");
/// assert_eq!(module_name_from_file(path), "sentry.templatetags.sentry_helpers");
///
/// let path = Utf8Path::new(".corpus/repos/django-allauth/allauth/templatetags/allauth.py");
/// assert_eq!(module_name_from_file(path), "allauth.templatetags.allauth");
/// ```
#[must_use]
pub fn module_name_from_file(file: &Utf8Path) -> String {
    let components: Vec<&str> = file
        .components()
        .filter_map(|c| match c {
            Utf8Component::Normal(s) => Some(s),
            Utf8Component::Prefix(_)
            | Utf8Component::RootDir
            | Utf8Component::CurDir
            | Utf8Component::ParentDir => None,
        })
        .collect();

    // Layout: "repos" followed by {dir_name}/{python_code}...
    // Skip 2 components after marker (the marker itself + the directory name).
    let start = if let Some(pos) = components.iter().position(|c| *c == "repos") {
        pos + 2
    } else {
        // Fallback for non-corpus paths: look for version-like directory
        // (starts with digit, contains '.', not a .py file)
        let mut fallback = 0;
        for (i, component) in components.iter().enumerate() {
            if component.chars().next().is_some_and(|c| c.is_ascii_digit())
                && component.contains('.')
                && !Utf8Path::new(component)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
            {
                fallback = i + 1;
            }
        }
        fallback
    };

    let slice: &[&str] = components.get(start..).unwrap_or(&[]);

    slice
        .iter()
        .map(|s| s.strip_suffix(".py").unwrap_or(s))
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::Corpus;
    use super::lock_entry_matches_declaration;
    use crate::corpus::lock::LockedRepo;
    use crate::corpus::lock::Lockfile;
    use crate::corpus::manifest::RepoSettingsProject;

    #[test]
    fn lock_entry_must_match_the_declared_repo_ref() {
        let locked_repo = LockedRepo {
            name: "example".to_string(),
            url: "https://example.com/repo.git".to_string(),
            tag: "main".to_string(),
            git_ref: "0123456789abcdef".to_string(),
        };
        let matching = RepoSettingsProject {
            repo_name: "example",
            repo_url: "https://example.com/repo.git",
            repo_ref: Some("main"),
            relative_root: None,
            django_settings_modules: vec!["project.settings"],
        };
        let stale = RepoSettingsProject {
            repo_ref: Some("release"),
            ..matching.clone()
        };

        assert!(lock_entry_matches_declaration(&locked_repo, &matching));
        assert!(!lock_entry_matches_declaration(&locked_repo, &stale));
    }

    #[test]
    fn whole_corpus_inventories_only_include_locked_repos() {
        let tempdir = tempfile::tempdir().expect("temporary corpus root should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary corpus path should be UTF-8");
        let registered = root.join("repos/djangopackages.org");
        let backup = root.join("repos/djangopackages.org.bak");

        let registered_tags = registered.join("package/templatetags/package_tags.py");
        let registered_models = registered.join("package/models.py");
        let backup_tags = backup.join("package/templatetags/package_tags.py");
        let backup_models = backup.join("package/models.py");
        for file in [
            &registered_tags,
            &registered_models,
            &backup_tags,
            &backup_models,
        ] {
            std::fs::create_dir_all(
                file.parent()
                    .expect("corpus source file should have a parent")
                    .as_std_path(),
            )
            .expect("corpus source directory should be created");
            std::fs::write(file.as_std_path(), "").expect("corpus source file should be created");
        }

        let corpus = Corpus {
            manifest_path: root.join("manifest.toml"),
            root,
            lockfile: Lockfile {
                repos: vec![LockedRepo {
                    name: "djangopackages.org".to_string(),
                    url: "https://example.com/djangopackages.org.git".to_string(),
                    tag: "main".to_string(),
                    git_ref: "0123456789abcdef".to_string(),
                }],
            },
        };

        assert_eq!(
            corpus.locked_repos().collect::<Vec<_>>(),
            vec![("djangopackages.org", registered.clone())]
        );
        let targets = corpus
            .extraction_target_members()
            .expect("locked extraction targets should have relative identities");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].member, "djangopackages.org");
        assert_eq!(
            targets[0].relative_path.as_str(),
            "package/templatetags/package_tags.py"
        );
        assert_eq!(targets[0].path, registered_tags);
        assert_eq!(corpus.model_files(), vec![registered_models]);
    }

    #[test]
    fn latest_package_ignores_completed_repositories_missing_from_the_lockfile() {
        let tempdir = tempfile::tempdir().expect("temporary corpus root should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary corpus path should be UTF-8");
        for name in ["django-5.2", "django-6.1", "django-99.0", "django"] {
            let directory = root.join("repos").join(name);
            std::fs::create_dir_all(directory.as_std_path())
                .expect("test corpus directory should be created");
            std::fs::write(directory.join(".complete.json").as_std_path(), "{}")
                .expect("test completion marker should be written");
        }
        let corpus = Corpus {
            manifest_path: root.join("manifest.toml"),
            root: root.clone(),
            lockfile: Lockfile {
                repos: ["django-5.2", "django-6.1"]
                    .into_iter()
                    .map(|name| LockedRepo {
                        name: name.to_string(),
                        url: format!("https://example.com/{name}.git"),
                        tag: name.to_string(),
                        git_ref: "0123456789abcdef".to_string(),
                    })
                    .collect(),
            },
        };

        assert_eq!(
            corpus.latest_package("django"),
            Some(root.join("repos/django-6.1"))
        );
    }

    #[test]
    fn extraction_selects_native_template_directory_components() {
        let tempdir = tempfile::tempdir().expect("temporary corpus root should be created");
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary corpus path should be UTF-8");
        let template_dir = root.join("django").join("template");
        let similar_dir = root.join("django").join("other_template");
        for directory in [&template_dir, &similar_dir] {
            std::fs::create_dir_all(directory).expect("source directory should be created");
        }
        let selected = template_dir.join("defaulttags.py");
        for file in [
            &selected,
            &template_dir.join("unrelated.py"),
            &similar_dir.join("defaulttags.py"),
        ] {
            std::fs::write(file, "").expect("source file should be created");
        }

        assert_eq!(Corpus::extraction_targets_in(&root), vec![selected]);
    }

    #[test]
    fn corpus_selectors_use_relative_directory_components() {
        let tempdir = tempfile::tempdir().expect("temporary corpus root should be created");
        let temp_root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf())
            .expect("temporary corpus path should be UTF-8");
        let root = temp_root.join("tests/docs/corpus");
        for relative in [
            "project/models.py",
            "project/docs/models.py",
            "project/templates/index.html",
            "project/docs/templates/index.html",
            "project/tests/templates/index.html",
            "project/assets/templates",
        ] {
            let file = root.join(relative);
            std::fs::create_dir_all(
                file.parent()
                    .expect("corpus source file should have a parent"),
            )
            .expect("corpus source directory should be created");
            std::fs::write(file, "").expect("corpus source file should be created");
        }

        let corpus = Corpus {
            root: root.clone(),
            manifest_path: root.join("manifest.toml"),
            lockfile: Lockfile::default(),
        };

        assert_eq!(
            corpus.model_files_in(&root),
            vec![root.join("project/models.py")]
        );
        assert_eq!(
            corpus.templates_in(&root),
            vec![root.join("project/templates/index.html")]
        );
    }

    #[test]
    fn corpus_exposes_real_repo_settings_projects() {
        // This checks checked-in metadata, not downloaded repository contents.
        let manifest_path = Utf8PathBuf::from(super::MANIFEST_PATH);
        let manifest =
            super::Manifest::load(&manifest_path).expect("default corpus manifest should load");
        let projects = manifest
            .repo_settings_projects()
            .into_iter()
            .map(|project| {
                (
                    project.repo_name.to_string(),
                    project
                        .relative_root
                        .unwrap_or(camino::Utf8Path::new("."))
                        .to_string(),
                    project
                        .django_settings_modules
                        .into_iter()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            projects,
            vec![
                (
                    "archivebox".to_string(),
                    ".".to_string(),
                    vec!["archivebox.core.settings".to_string()],
                ),
                (
                    "django-allauth".to_string(),
                    ".".to_string(),
                    vec!["tests.projects.account_only.settings".to_string()],
                ),
                (
                    "healthchecks".to_string(),
                    ".".to_string(),
                    vec!["hc.settings".to_string()],
                ),
                (
                    "inventree".to_string(),
                    "src/backend/InvenTree".to_string(),
                    vec!["InvenTree.settings".to_string()],
                ),
                (
                    "netbox".to_string(),
                    "netbox".to_string(),
                    vec!["netbox.settings".to_string()],
                ),
                (
                    "pretix".to_string(),
                    ".".to_string(),
                    vec!["pretix.settings".to_string()],
                ),
                (
                    "sentry".to_string(),
                    ".".to_string(),
                    vec!["sentry.conf.server".to_string()],
                ),
            ]
        );
    }
}
