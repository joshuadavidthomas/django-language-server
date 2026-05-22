use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_project::load_env_file;
use djls_source::FileRootKind;
use salsa::Durability;

use crate::project::db::Db as ProjectDb;
use crate::project::python::Interpreter;
use crate::project::resolve::find_site_packages;
use crate::project::symbols::TemplateLibraries;

/// Template-directory introspection state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TemplateDirs {
    /// Template directories have not been loaded yet, or the last refresh failed.
    #[default]
    Unknown,
    /// Template directories were loaded successfully. The list may be empty.
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
}

impl Project {
    pub fn register_source_roots(self, db: &dyn ProjectDb) {
        register_source_roots(db, self.root(db), self.interpreter(db));
    }

    pub fn bootstrap(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let interpreter = Interpreter::discover(settings.venv_path());
        let resolved_django_settings_module = resolve_django_settings(root, settings);
        let env_vars = load_env_file(root, settings);

        register_source_roots(db, root, &interpreter);

        Project::builder(
            root.to_path_buf(),
            interpreter,
            resolved_django_settings_module,
            settings.pythonpath().to_vec(),
            env_vars,
            TemplateDirs::Unknown,
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
        )
        .durability(Durability::MEDIUM)
        .root_durability(Durability::HIGH)
        .new(db)
    }
}

fn register_source_roots(db: &dyn ProjectDb, root: &Utf8Path, interpreter: &Interpreter) {
    let source_files = db.files();
    source_files.try_add_root(root.to_path_buf(), FileRootKind::Project);
    if let Some(site_packages) = find_site_packages(interpreter, root) {
        source_files.try_add_root(site_packages, FileRootKind::LibrarySearchPath);
    }
}

pub(crate) fn resolve_django_settings(root: &Utf8Path, settings: &Settings) -> Option<String> {
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
