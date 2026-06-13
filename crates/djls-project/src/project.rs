use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_source::File;
use djls_source::FileSystem;
use salsa::Durability;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::names::ModulePath;
use crate::python::Interpreter;
use crate::resolve::SearchPaths;

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    module_path: ModulePath,
    path: Utf8PathBuf,
    file: File,
}

impl PythonModule {
    pub(crate) fn new(module_path: ModulePath, path: Utf8PathBuf, file: File) -> Self {
        Self {
            module_path,
            path,
            file,
        }
    }

    pub fn module_path(&self) -> &ModulePath {
        &self.module_path
    }

    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    pub fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
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
    /// Python module-resolution paths for this project.
    #[returns(ref)]
    pub search_paths: SearchPaths,
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
    /// Manual TagSpecs configuration from TOML (fallback for extraction gaps)
    #[returns(ref)]
    pub tagspecs: TagSpecDef,
}

impl Project {
    pub(crate) fn touch_search_path_roots(self, db: &dyn ProjectDb) {
        for search_path in self.search_paths(db).iter() {
            if let Some(root) = db.files().root(db, search_path.path()) {
                let _ = root.revision(db);
            } else {
                tracing::warn!(
                    "Search path has no registered source root: {}",
                    search_path.path()
                );
            }
        }
    }

    pub fn initial(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let search_paths = SearchPaths::root_only(root);
        let interpreter = settings.venv_path().map_or(Interpreter::Auto, |path| {
            Interpreter::VenvPath(path.to_string())
        });
        let django_settings_module = settings.django_settings_module().map(String::from);
        let pythonpath = settings.pythonpath().to_vec();
        let env_vars = Vec::new();
        let tagspecs = settings.tagspecs().clone();

        search_paths.register_roots(db);
        Project::builder(
            root.to_path_buf(),
            search_paths,
            interpreter,
            django_settings_module,
            pythonpath,
            env_vars,
            tagspecs,
        )
        .durability(Durability::MEDIUM)
        .root_durability(Durability::HIGH)
        .new(db)
    }

    pub fn bootstrap(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let interpreter = Interpreter::discover(settings.venv_path());
        let django_settings_module = resolve_django_settings(db.file_system(), root, settings);
        let env_vars = load_env_file(db.file_system(), root, settings);
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            root,
            &interpreter,
            settings.pythonpath(),
        );
        let pythonpath = settings.pythonpath().to_vec();
        let tagspecs = settings.tagspecs().clone();

        search_paths.register_roots(db);
        Project::builder(
            root.to_path_buf(),
            search_paths,
            interpreter,
            django_settings_module,
            pythonpath,
            env_vars,
            tagspecs,
        )
        .durability(Durability::MEDIUM)
        .root_durability(Durability::HIGH)
        .new(db)
    }

    /// Reload settings-derived project fields on this stable Salsa input.
    pub fn reload_from_settings(self, db: &mut dyn ProjectDb, settings: &Settings) {
        let root = self.root(db).clone();
        let interpreter = Interpreter::discover(settings.venv_path());
        let django_settings_module = resolve_django_settings(db.file_system(), &root, settings);
        let env_vars = load_env_file(db.file_system(), &root, settings);
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            &root,
            &interpreter,
            settings.pythonpath(),
        );
        let pythonpath = settings.pythonpath().to_vec();
        let tagspecs = settings.tagspecs().clone();

        if self.search_paths(db) != &search_paths {
            search_paths.register_roots(db);
            self.set_search_paths(db).to(search_paths);
        }

        if self.interpreter(db) != &interpreter {
            self.set_interpreter(db).to(interpreter);
        }

        if self.django_settings_module(db) != &django_settings_module {
            self.set_django_settings_module(db)
                .to(django_settings_module);
        }

        if self.pythonpath(db) != &pythonpath {
            self.set_pythonpath(db).to(pythonpath);
        }

        if self.env_vars(db) != &env_vars {
            self.set_env_vars(db).to(env_vars);
        }

        if self.tagspecs(db) != &tagspecs {
            self.set_tagspecs(db).to(tagspecs);
        }
    }
}

pub fn load_env_file(
    fs: &dyn FileSystem,
    root: &Utf8Path,
    settings: &Settings,
) -> Vec<(String, String)> {
    let env_path = match settings.env_file() {
        Some(path) => root.join(path),
        None => root.join(".env"),
    };

    if !fs.exists(&env_path) {
        if settings.env_file().is_some() {
            tracing::warn!("Configured env_file not found: {}", env_path);
        } else {
            tracing::debug!("No .env file found at {}", env_path);
        }
        return Vec::new();
    }

    let content = match fs.read_to_string(&env_path) {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!("Failed to read env file {}: {}", env_path, err);
            return Vec::new();
        }
    };

    let mut vars = Vec::new();
    for item in dotenvy::from_read_iter(content.as_bytes()) {
        match item {
            Ok((key, value)) => {
                tracing::debug!("Loaded env var from file: {}", key);
                vars.push((key, value));
            }
            Err(err) => {
                tracing::warn!("Failed to parse env file entry: {}", err);
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

pub(crate) fn resolve_django_settings(
    fs: &dyn FileSystem,
    root: &Utf8Path,
    settings: &Settings,
) -> Option<String> {
    settings
        .django_settings_module()
        .map(String::from)
        .or_else(|| {
            std::env::var("DJANGO_SETTINGS_MODULE")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| auto_detect_settings_module(fs, root))
}

fn auto_detect_settings_module(fs: &dyn FileSystem, root: &Utf8Path) -> Option<String> {
    if !fs.exists(&root.join("manage.py")) {
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

        if fs.exists(&path) {
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
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

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
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0], ("MY_VAR".to_string(), "hello".to_string()));
        }

        #[test]
        fn returns_empty_when_no_env_file() {
            let dir = tempdir().unwrap();
            let root = Utf8Path::from_path(dir.path()).unwrap();

            let settings = Settings::default();
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

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
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

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
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

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
            let vars = load_env_file(&djls_source::OsFileSystem, root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(
                vars[0],
                ("SECRET".to_string(), "my secret value".to_string())
            );
            assert_eq!(vars[1], ("OTHER".to_string(), "single quoted".to_string()));
        }
    }
}
