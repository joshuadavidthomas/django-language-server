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

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use ignore::WalkBuilder;

pub(crate) mod archive;
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
const LOCKFILE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.lock");
const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.toml");

/// A validated corpus root directory.
///
/// Constructed via [`Corpus::require`], which validates that the
/// directory exists. Once constructed, the root path is trusted for
/// the lifetime of the value.
pub struct Corpus {
    lockfile: lock::Lockfile,
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

impl Corpus {
    /// Check whether the corpus directory exists.
    #[must_use]
    pub fn is_available() -> bool {
        Utf8Path::new(CORPUS_DIR).as_std_path().exists()
    }

    /// Get the corpus after checking it against the lockfile.
    pub fn require() -> anyhow::Result<Self> {
        if !Self::is_available() {
            anyhow::bail!("Corpus not synced. Run: cargo run -p djls-testing --bin corpus -- sync");
        }
        let lockfile = lock::Lockfile::load(Utf8Path::new(LOCKFILE_PATH)).map_err(|error| {
            anyhow::anyhow!("Corpus lockfile missing or invalid. Run: just corpus lock: {error}")
        })?;
        let corpus = Self { lockfile };
        sync::validate_synced_corpus(&corpus.lockfile, corpus.root())?;
        Ok(corpus)
    }

    /// The corpus root directory.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        Utf8Path::new(CORPUS_DIR)
    }

    pub fn repo_settings_projects(&self) -> anyhow::Result<Vec<CorpusSettingsProject>> {
        let manifest = Manifest::load(Utf8Path::new(MANIFEST_PATH))?;
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

    /// Derive the corpus entry directory for a path under the corpus root.
    ///
    /// Corpus entries are direct children of `repos/`.
    /// For example:
    /// - `{root}/repos/django-6.0/...` -> `{root}/repos/django-6.0`
    /// - `{root}/repos/sentry/...` -> `{root}/repos/sentry`
    #[must_use]
    pub fn entry_dir_for_path(&self, path: &Utf8Path) -> Option<Utf8PathBuf> {
        let relative = path.strip_prefix(self.root()).ok()?;

        let mut components = relative.components();
        let category = components.next()?;
        let entry = components.next()?;

        if category.as_str() == "repos" {
            Some(self.root().join(category.as_str()).join(entry.as_str()))
        } else {
            None
        }
    }

    /// Whether an entry directory represents a Django package.
    ///
    /// True for:
    /// - `repos/django`
    /// - `repos/django-<version>`
    #[must_use]
    pub(crate) fn is_django_entry(entry_dir: &Utf8Path) -> bool {
        let Some(entry_name) = entry_dir.file_name() else {
            return false;
        };

        let is_repos = entry_dir
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|cat| cat == "repos");

        if !is_repos {
            return false;
        }

        entry_name == "django"
            || (entry_name.starts_with("django-")
                && entry_name["django-".len()..].starts_with(|c: char| c.is_ascii_digit()))
    }

    /// Latest synced version directory for a package under `repos/`.
    ///
    /// Handles both single-entry names (e.g. `repos/django-allauth/`)
    /// and multi-version names (e.g. `repos/django-6.0/`).
    #[must_use]
    pub fn latest_package(&self, name: &str) -> Option<Utf8PathBuf> {
        let repos_dir = self.root().join("repos");

        // Single-version: repos/{name}/
        let exact = repos_dir.join(name);
        if exact.join(".complete.json").as_std_path().exists() {
            return Some(exact);
        }

        // Multi-version: repos/{name}-{version}/ — find highest version
        let prefix = format!("{name}-");
        let Ok(entries) = std::fs::read_dir(repos_dir.as_std_path()) else {
            return None;
        };

        let mut best: Option<(Vec<u32>, Utf8PathBuf)> = None;
        for entry in entries.filter_map(Result::ok) {
            let Some(dir_name) = entry.file_name().to_str().map(String::from) else {
                continue;
            };
            if !dir_name.starts_with(&prefix) {
                continue;
            }

            let version_str = &dir_name[prefix.len()..];
            // The suffix after "{name}-" must start with a digit to be a
            // version, otherwise it's a different package (e.g. "django-cms"
            // should not match prefix "django-").
            if !version_str.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }

            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
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

    /// All extraction target files in the entire corpus.
    #[must_use]
    pub fn extraction_targets(&self) -> Vec<Utf8PathBuf> {
        Self::extraction_targets_in(self.root())
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
            let path_str = path.as_str();

            if path_str.contains("__pycache__") {
                continue;
            }

            let is_py = path.extension().is_some_and(|ext| ext == "py");
            let is_core_template_module = path_str.contains("/template/")
                && matches!(
                    path.file_name(),
                    Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
                );

            if is_py
                && path.file_name() != Some("__init__.py")
                && (path_str.contains("/templatetags/") || is_core_template_module)
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
        self.model_files_in(self.root())
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
            let path_str = path.as_str();

            if path.file_name() == Some("models.py")
                && !path_str.contains("__pycache__")
                && !path_str.contains("/docs/")
                && !path_str.contains("/tests/")
                && !path_str.contains("/test/")
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
            let path_str = path.as_str();

            if path_str.contains("/templates/")
                && !path_str.contains("__pycache__")
                && !path_str.contains("/docs/")
                && !path_str.contains("/tests/")
                && !path_str.contains("/jinja2/")
                && !path_str.contains("/static/")
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
    use super::Corpus;
    use super::lock_entry_matches_declaration;
    use crate::corpus::lock::LockedRepo;
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
    fn corpus_exposes_real_repo_settings_projects() {
        let corpus = Corpus::require().expect("synced corpus should be available");
        let projects = corpus
            .repo_settings_projects()
            .expect("default corpus manifest should load")
            .into_iter()
            .map(|project| {
                let relative_root = if project.project_root == project.checkout_root {
                    ".".to_string()
                } else {
                    project
                        .project_root
                        .strip_prefix(&project.checkout_root)
                        .expect("project root should stay within its checkout")
                        .to_string()
                };
                (
                    project.repo_name,
                    relative_root,
                    project.django_settings_modules,
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
