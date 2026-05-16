use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_source::File;
use djls_source::FileRootKind;
use rustc_hash::FxHashMap;
use salsa::Durability;

use crate::project::db::Db as ProjectDb;
use crate::project::python::Interpreter;
use crate::project::resolve::find_site_packages;
use crate::project::symbols::TemplateLibraries;
use crate::python::BlockSpecs;
use crate::python::FilterArityMap;
use crate::python::ModelGraph;
use crate::python::ModulePath;
use crate::python::TagRuleMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TemplateDirs {
    #[default]
    Unknown,
    Known(Vec<Utf8PathBuf>),
}

impl TemplateDirs {
    #[must_use]
    pub fn as_known(&self) -> Option<&[Utf8PathBuf]> {
        match self {
            Self::Unknown => None,
            Self::Known(dirs) => Some(dirs),
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProjectTemplateFiles(Vec<ProjectTemplateFile>);

impl ProjectTemplateFiles {
    pub(crate) fn from_ordered(templates: Vec<ProjectTemplateFile>) -> Self {
        Self(templates)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ProjectTemplateFile> {
        self.0.iter()
    }
}

impl fmt::Debug for ProjectTemplateFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProjectTemplateFiles")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProjectPythonModules(Vec<ProjectPythonModule>);

impl ProjectPythonModules {
    pub(crate) fn new(mut modules: Vec<ProjectPythonModule>) -> Self {
        modules.sort_by(|a, b| {
            a.module_path
                .cmp(&b.module_path)
                .then_with(|| a.path.cmp(&b.path))
        });
        Self(modules)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ProjectPythonModule> {
        self.0.iter()
    }
}

impl fmt::Debug for ProjectPythonModules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProjectPythonModules")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectTemplateFile {
    name: String,
    path: Utf8PathBuf,
    file: File,
}

impl ProjectTemplateFile {
    pub(crate) fn new(name: String, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for ProjectTemplateFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectTemplateFile")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectPythonModule {
    module_path: ModulePath,
    path: Utf8PathBuf,
    file: File,
}

impl ProjectPythonModule {
    pub(crate) fn new(module_path: ModulePath, path: Utf8PathBuf, file: File) -> Self {
        Self {
            module_path,
            path,
            file,
        }
    }

    pub(crate) fn module_path(&self) -> &ModulePath {
        &self.module_path
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for ProjectPythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectPythonModule")
            .field("module_path", &self.module_path)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Complete project configuration as a Salsa input.
///
/// This represents the core identity of a project: where it is (root path),
/// which Python environment to use (interpreter), Django-specific configuration,
/// and external data sources that drive semantic analysis.
///
/// Tracked queries in `djls-server` convert extraction results into semantic
/// types (`TagSpecs`).
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    /// The project root path
    #[returns(ref)]
    pub root: Utf8PathBuf,
    /// Interpreter specification for Python environment discovery
    #[returns(ref)]
    pub interpreter: Interpreter,
    /// Django settings module (e.g., "myproject.settings")
    #[returns(ref)]
    pub django_settings_module: Option<String>,
    /// Additional Python import paths (PYTHONPATH entries)
    #[returns(ref)]
    pub pythonpath: Vec<String>,
    /// Extra environment variables for project introspection, loaded from an
    /// env file (e.g. `.env`). Each entry is a `(key, value)` pair.
    #[returns(ref)]
    pub env_vars: Vec<(String, String)>,
    /// Template directories reported by project introspection.
    #[returns(ref)]
    pub template_dirs: TemplateDirs,
    /// Manual TagSpecs configuration from TOML (fallback for extraction gaps)
    #[returns(ref)]
    pub tagspecs: TagSpecDef,
    /// Template libraries and symbols for this project.
    ///
    /// This value always exists to support progressive enhancement:
    /// - Discovered libraries are populated by scanning `sys.path`.
    /// - Installed libraries/symbols are populated by project introspection.
    ///
    /// The semantic layer combines this with `{% load %}` scope computed from templates.
    #[returns(ref)]
    pub template_libraries: TemplateLibraries,
    /// First-party template files discovered for this project.
    #[returns(ref)]
    pub(crate) template_files: ProjectTemplateFiles,
    /// First-party model modules discovered for this project.
    #[returns(ref)]
    pub(crate) model_modules: ProjectPythonModules,
    /// First-party templatetag modules discovered for this project.
    #[returns(ref)]
    pub(crate) templatetag_modules: ProjectPythonModules,
    /// Extracted tag rules from external modules (site-packages), keyed by
    /// registration module path (e.g., `"django.templatetags.i18n"`).
    /// Populated by `refresh_external_data`. Workspace files use tracked queries.
    #[returns(ref)]
    pub(crate) extracted_external_tag_rules: FxHashMap<String, TagRuleMap>,
    /// Extracted filter arities from external modules (site-packages), keyed by
    /// registration module path. Populated by `refresh_external_data`.
    #[returns(ref)]
    pub(crate) extracted_external_filter_arities: FxHashMap<String, FilterArityMap>,
    /// Extracted block specs from external modules (site-packages), keyed by
    /// registration module path. Populated by `refresh_external_data`.
    #[returns(ref)]
    pub(crate) extracted_external_block_specs: FxHashMap<String, BlockSpecs>,
    /// Model graphs from external packages (site-packages), keyed by module
    /// path (e.g., `"django.contrib.auth.models"`). Populated by scanning
    /// the venv's site-packages directory. Workspace `models.py` files use
    /// tracked queries via `collect_workspace_models` instead.
    #[returns(ref)]
    pub(crate) extracted_external_models: FxHashMap<ModulePath, ModelGraph>,
}

impl Project {
    pub fn bootstrap(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let interpreter = Interpreter::discover(settings.venv_path());
        let resolved_django_settings_module = resolve_django_settings(root, settings);
        let env_vars = load_env_file(root, settings);

        let source_files = db.files();
        source_files.try_add_root(root.to_path_buf(), FileRootKind::Project);
        if let Some(site_packages) = find_site_packages(&interpreter, root) {
            source_files.try_add_root(site_packages, FileRootKind::LibrarySearchPath);
        }

        Project::builder(
            root.to_path_buf(),
            interpreter,
            resolved_django_settings_module,
            settings.pythonpath().to_vec(),
            env_vars,
            TemplateDirs::Unknown,
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
            ProjectTemplateFiles::default(),
            ProjectPythonModules::default(),
            ProjectPythonModules::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
        )
        .durability(Durability::MEDIUM)
        .root_durability(Durability::HIGH)
        .template_files_durability(Durability::LOW)
        .model_modules_durability(Durability::LOW)
        .templatetag_modules_durability(Durability::LOW)
        .extracted_external_tag_rules_durability(Durability::HIGH)
        .extracted_external_filter_arities_durability(Durability::HIGH)
        .extracted_external_block_specs_durability(Durability::HIGH)
        .extracted_external_models_durability(Durability::HIGH)
        .new(db)
    }
}

pub fn load_env_file(root: &Utf8Path, settings: &Settings) -> Vec<(String, String)> {
    let env_path = match settings.env_file() {
        Some(path) => root.join(path),
        None => root.join(".env"),
    };

    if !env_path.exists() {
        if settings.env_file().is_some() {
            tracing::warn!("Configured env_file not found: {}", env_path);
        } else {
            tracing::debug!("No .env file found at {}", env_path);
        }
        return Vec::new();
    }

    match dotenvy::from_path_iter(env_path.as_std_path()) {
        Ok(iter) => {
            let mut vars = Vec::new();
            for item in iter {
                match item {
                    Ok((key, value)) => {
                        tracing::debug!("Loaded env var from file: {}", key);
                        vars.push((key, value));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse env file entry: {}", e);
                    }
                }
            }
            if !vars.is_empty() {
                tracing::info!(
                    "Loaded {} environment variable(s) from env file",
                    vars.len()
                );
            }
            vars
        }
        Err(e) => {
            tracing::warn!("Failed to read env file {}: {}", env_path, e);
            Vec::new()
        }
    }
}

fn resolve_django_settings(root: &Utf8Path, settings: &Settings) -> Option<String> {
    settings
        .django_settings_module()
        .map(String::from)
        .or_else(|| {
            std::env::var("DJANGO_SETTINGS_MODULE")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| auto_detect_settings_module(root))
}

fn auto_detect_settings_module(root: &Utf8Path) -> Option<String> {
    if !root.join("manage.py").exists() {
        tracing::debug!("No manage.py found, skipping Django settings auto-detection");
        return None;
    }

    for candidate in &["settings", "config.settings", "project.settings"] {
        let parts: Vec<&str> = candidate.split('.').collect();
        let mut path = root.to_path_buf();
        for part in &parts[..parts.len() - 1] {
            path = path.join(part);
        }
        if let Some(last) = parts.last() {
            path = path.join(format!("{last}.py"));
        }

        if path.exists() {
            tracing::info!("Auto-detected Django settings module: {}", candidate);
            return Some((*candidate).to_string());
        }
    }

    tracing::warn!("manage.py found but could not auto-detect Django settings module");
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino::Utf8Path;
    use djls_conf::Settings;
    use tempfile::tempdir;

    use super::*;

    mod env_file {
        use super::*;

        #[test]
        fn loads_default_dot_env() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(
                dir.path().join(".env"),
                "DJANGO_SECRET_KEY=test-secret\nDATABASE_URL=postgres://localhost/db\n",
            )
            .unwrap();

            let settings = Settings::default();
            let vars = load_env_file(root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(
                vars[0],
                ("DJANGO_SECRET_KEY".to_string(), "test-secret".to_string())
            );
            assert_eq!(
                vars[1],
                (
                    "DATABASE_URL".to_string(),
                    "postgres://localhost/db".to_string()
                )
            );
        }

        #[test]
        fn loads_configured_env_file_path() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(dir.path().join(".env.local"), "MY_VAR=hello\n").unwrap();
            fs::write(dir.path().join("djls.toml"), r#"env_file = ".env.local""#).unwrap();

            let settings = Settings::new(root, None).unwrap();
            let vars = load_env_file(root, &settings);

            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0], ("MY_VAR".to_string(), "hello".to_string()));
        }

        #[test]
        fn returns_empty_when_no_env_file() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();

            let settings = Settings::default();
            let vars = load_env_file(root, &settings);

            assert!(vars.is_empty());
        }

        #[test]
        fn returns_empty_when_configured_file_missing() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(
                dir.path().join("djls.toml"),
                r#"env_file = ".env.nonexistent""#,
            )
            .unwrap();

            let settings = Settings::new(root, None).unwrap();
            let vars = load_env_file(root, &settings);

            assert!(vars.is_empty());
        }

        #[test]
        fn handles_comments_and_blank_lines() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(
                dir.path().join(".env"),
                "# This is a comment\n\nDJANGO_SECRET_KEY=secret\n\n# Another comment\nDEBUG=true\n",
            )
            .unwrap();

            let settings = Settings::default();
            let vars = load_env_file(root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(vars[0].0, "DJANGO_SECRET_KEY");
            assert_eq!(vars[1].0, "DEBUG");
        }

        #[test]
        fn handles_quoted_values() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();
            fs::write(
                dir.path().join(".env"),
                "SECRET=\"my secret value\"\nOTHER='single quoted'\n",
            )
            .unwrap();

            let settings = Settings::default();
            let vars = load_env_file(root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(
                vars[0],
                ("SECRET".to_string(), "my secret value".to_string())
            );
            assert_eq!(vars[1], ("OTHER".to_string(), "single quoted".to_string()));
        }
    }
}
