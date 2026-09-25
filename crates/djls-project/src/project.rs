use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DjangoVersion;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_source::FileSystem;
use ruff_python_ast::Expr;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;
use ruff_python_parser::parse_module;
use salsa::Durability;
use salsa::Setter;

use crate::ast::ExprExt;
use crate::db::Db as ProjectDb;
use crate::python::PythonEnvironment;
use crate::python::PythonModuleName;
use crate::python::SearchPaths;

/// Complete project configuration as a Salsa input.
///
/// This represents the core identity of a project: where it is (root path),
/// which Python environment to inspect, Django-specific configuration,
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
    /// Python executable, environment root, or prefix selection for import discovery.
    #[returns(ref)]
    pub python_environment: PythonEnvironment,
    /// Django settings module name (e.g., "myproject.settings")
    #[returns(ref)]
    pub django_settings_module: Option<PythonModuleName>,
    /// Explicit feature-line override for bundled source discovery.
    #[returns(copy)]
    pub django_version: Option<DjangoVersion>,
    /// Additional Python import paths (PYTHONPATH entries)
    #[returns(ref)]
    pub pythonpath: Vec<Utf8PathBuf>,
    /// Extra environment variables for project introspection, loaded from an
    /// env file (e.g. `.env`). Each entry is a `(key, value)` pair.
    #[returns(ref)]
    pub env_vars: Vec<(String, String)>,
    /// Manual TagSpecs configuration from TOML (fallback for extraction gaps)
    #[returns(ref)]
    pub tagspecs: TagSpecDef,
}

impl Project {
    /// Returns the last value for `key` from the project's env file.
    ///
    /// `load_env_file` preserves duplicate entries in source order, while
    /// dotenv semantics give the last entry precedence.
    pub(crate) fn env_var<'db>(self, db: &'db dyn ProjectDb, key: &str) -> Option<&'db str> {
        self.env_vars(db)
            .iter()
            .rev()
            .find_map(|(candidate, value)| {
                let matches = if cfg!(windows) {
                    candidate.eq_ignore_ascii_case(key)
                } else {
                    candidate == key
                };
                matches.then_some(value.as_str())
            })
    }

    pub(crate) fn touch_search_path_roots(self, db: &dyn ProjectDb) {
        for search_path in self.search_paths(db).iter() {
            if let Some(root) = db.files().root(db, search_path.path()) {
                let _ = root.revision(db);
            } else {
                tracing::warn!(
                    search_path_kind = search_path.kind_name(),
                    "Search path has no registered source root"
                );
                tracing::debug!(
                    path = %search_path.path(),
                    "Search path without registered source root"
                );
            }
        }
    }

    pub fn initial(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let search_paths = SearchPaths::root_only(root);
        let python_environment = PythonEnvironment::discover(settings.venv_path());
        let django_settings_module = settings
            .django_settings_module()
            .and_then(|module_name| PythonModuleName::parse(module_name).ok());
        let pythonpath = settings.pythonpath().to_vec();
        let env_vars = Vec::new();
        let tagspecs = settings.tagspecs().clone();

        search_paths.register_roots(db);
        Project::builder(
            root.to_path_buf(),
            search_paths,
            python_environment,
            django_settings_module,
            settings.django_version(),
            pythonpath,
            env_vars,
            tagspecs,
        )
        .durability(Durability::MEDIUM)
        .root_durability(Durability::HIGH)
        .new(db)
    }

    pub fn bootstrap(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let process_settings_module = std::env::var("DJANGO_SETTINGS_MODULE").ok();
        let python_environment = PythonEnvironment::discover(settings.venv_path());
        let django_settings_module = django_settings_module_name(
            db.file_system(),
            root,
            settings,
            process_settings_module.as_deref(),
        );
        let env_vars = load_env_file(db.file_system(), root, settings);
        let search_paths = SearchPaths::from_project_settings(
            db.file_system(),
            root,
            &python_environment,
            settings.pythonpath(),
        );
        let pythonpath = settings.pythonpath().to_vec();
        let tagspecs = settings.tagspecs().clone();

        search_paths.register_roots(db);
        Project::builder(
            root.to_path_buf(),
            search_paths,
            python_environment,
            django_settings_module,
            settings.django_version(),
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
        let python_environment = PythonEnvironment::discover(settings.venv_path());
        let process_settings_module = std::env::var("DJANGO_SETTINGS_MODULE").ok();
        let django_settings_module = django_settings_module_name(
            db.file_system(),
            &root,
            settings,
            process_settings_module.as_deref(),
        );
        let env_vars = load_env_file(db.file_system(), &root, settings);
        let pythonpath = settings.pythonpath().to_vec();
        let tagspecs = settings.tagspecs().clone();

        if self.python_environment(db) != &python_environment {
            self.set_python_environment(db).to(python_environment);
        }

        if self.django_settings_module(db) != &django_settings_module {
            self.set_django_settings_module(db)
                .to(django_settings_module);
        }

        if self.django_version(db) != settings.django_version() {
            self.set_django_version(db).to(settings.django_version());
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

fn load_env_file(
    fs: &dyn FileSystem,
    root: &Utf8Path,
    settings: &Settings,
) -> Vec<(String, String)> {
    let env_path = match settings.env_file() {
        Some(path) => root.join(path),
        None => root.join(".env"),
    };

    if !fs.is_file(&env_path) {
        if settings.env_file().is_some() {
            if fs.exists(&env_path) {
                tracing::warn!(reason = "not_file", "Configured env file is unavailable");
            } else {
                tracing::warn!(reason = "not_found", "Configured env file is unavailable");
            }
            tracing::debug!(path = %env_path, "Configured env file path");
        } else {
            tracing::debug!(path = %env_path, "No default env file found");
        }
        return Vec::new();
    }

    let content = match fs.read_to_string(&env_path) {
        Ok(content) => content,
        Err(error) => {
            tracing::warn!(error_kind = ?error.kind(), "Failed to read env file");
            tracing::debug!(path = %env_path, %error, "Env file read error");
            return Vec::new();
        }
    };

    let mut vars = Vec::new();
    let mut parse_error_count = 0usize;
    for item in dotenvy::from_read_iter(content.as_bytes()) {
        match item {
            Ok((key, value)) => {
                vars.push((key, value));
            }
            // dotenvy parse errors embed the offending line, which may hold a secret value.
            Err(_) => {
                parse_error_count += 1;
            }
        }
    }
    if parse_error_count > 0 {
        tracing::warn!(parse_error_count, "Skipped invalid entries in env file");
        tracing::debug!(
            path = %env_path,
            parse_error_count,
            "Env file has invalid entries"
        );
    }
    if !vars.is_empty() {
        tracing::info!(
            variable_count = vars.len(),
            "Loaded environment variables from env file"
        );
        // Names only: values are commonly secrets.
        tracing::debug!(
            path = %env_path,
            names = ?vars.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            "Loaded environment variable names from env file"
        );
    }
    vars
}

pub(crate) fn django_settings_module_name(
    fs: &dyn FileSystem,
    root: &Utf8Path,
    settings: &Settings,
    process_settings_module: Option<&str>,
) -> Option<PythonModuleName> {
    if let Some(module_name) = settings.django_settings_module() {
        return PythonModuleName::parse(module_name).ok();
    }

    if let Some(module_name) = process_settings_module.filter(|value| !value.is_empty()) {
        return PythonModuleName::parse(module_name).ok();
    }

    let manage_path = root.join("manage.py");
    if !fs.exists(&manage_path) {
        tracing::debug!("No manage.py found, skipping Django settings auto-detection");
        return None;
    }

    let source = match fs.read_to_string(&manage_path) {
        Ok(source) => source,
        Err(error) => {
            tracing::warn!(
                error_kind = ?error.kind(),
                "Could not read manage.py for Django settings auto-detection"
            );
            tracing::debug!(path = %manage_path, %error, "manage.py read error");
            return None;
        }
    };
    let module = django_settings_module_from_manage_source(&source);
    if let Some(module) = &module {
        tracing::info!(source = "manage_py", "Auto-detected Django settings module");
        tracing::debug!(
            module = module.as_str(),
            "Auto-detected Django settings module name"
        );
        return Some(module.clone());
    }

    tracing::warn!(
        "manage.py found but could not statically determine a unique Django settings module"
    );
    None
}

fn django_settings_module_from_manage_source(source: &str) -> Option<PythonModuleName> {
    let module = parse_module(source).ok()?.into_syntax();
    let mut visitor = ManageSettingsVisitor::default();
    visitor.visit_body(&module.body);

    match visitor.module {
        ManageSettingsModule::Unique(module) => Some(module),
        ManageSettingsModule::Missing | ManageSettingsModule::Inconclusive => None,
    }
}

#[derive(Default)]
struct ManageSettingsVisitor {
    module: ManageSettingsModule,
}

#[derive(Default)]
enum ManageSettingsModule {
    #[default]
    Missing,
    Unique(PythonModuleName),
    Inconclusive,
}

impl ManageSettingsModule {
    fn observe(&mut self, declaration: ManageSettingsDeclaration<'_>) {
        let module = match declaration {
            ManageSettingsDeclaration::Other => return,
            ManageSettingsDeclaration::Inconclusive => {
                *self = Self::Inconclusive;
                return;
            }
            ManageSettingsDeclaration::Module(value) => match PythonModuleName::parse(value) {
                Ok(module) if module.as_str() == value => module,
                Ok(_) | Err(_) => {
                    *self = Self::Inconclusive;
                    return;
                }
            },
        };

        match self {
            Self::Missing => *self = Self::Unique(module),
            Self::Unique(previous) if previous != &module => *self = Self::Inconclusive,
            Self::Unique(_) | Self::Inconclusive => {}
        }
    }
}

impl<'a> Visitor<'a> for ManageSettingsVisitor {
    fn visit_expr(&mut self, expression: &'a Expr) {
        self.module.observe(manage_settings_declaration(expression));
        visitor::walk_expr(self, expression);
    }
}

#[derive(Clone, Copy)]
enum ManageSettingsDeclaration<'a> {
    Other,
    Inconclusive,
    Module(&'a str),
}

fn manage_settings_declaration(expression: &Expr) -> ManageSettingsDeclaration<'_> {
    let Expr::Call(call) = expression else {
        return ManageSettingsDeclaration::Other;
    };
    if call.func.path_segments().as_deref()
        != Some(&[
            "os".to_string(),
            "environ".to_string(),
            "setdefault".to_string(),
        ])
    {
        return ManageSettingsDeclaration::Other;
    }
    match call
        .arguments
        .args
        .first()
        .and_then(ExprExt::string_literal)
    {
        Some("DJANGO_SETTINGS_MODULE") => {}
        Some(_) => return ManageSettingsDeclaration::Other,
        None => return ManageSettingsDeclaration::Inconclusive,
    }
    if call.arguments.args.len() != 2 || !call.arguments.keywords.is_empty() {
        return ManageSettingsDeclaration::Inconclusive;
    }
    match call.arguments.args[1].string_literal() {
        Some(value) => ManageSettingsDeclaration::Module(value),
        None => ManageSettingsDeclaration::Inconclusive,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino::Utf8Path;
    use djls_conf::Settings;
    use djls_testing::capture_events;
    use tempfile::tempdir;

    use super::*;

    mod settings_module {
        use super::*;

        #[test]
        fn extracts_canonical_manage_py_declaration() {
            let source = r#"
import os

def main():
    os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
"#;

            let module = django_settings_module_from_manage_source(source)
                .expect("canonical manage.py should declare its settings module");

            assert_eq!(module.as_str(), "mysite.settings");
        }

        #[test]
        fn accepts_repeated_identical_declarations() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
"#;

            let module = django_settings_module_from_manage_source(source)
                .expect("identical declarations should be unambiguous");

            assert_eq!(module.as_str(), "mysite.settings");
        }

        #[test]
        fn rejects_dynamic_declaration() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", choose_settings())
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }

        #[test]
        fn rejects_dynamic_declaration_before_literal_declaration() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", choose_settings())
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }

        #[test]
        fn rejects_dynamic_key_before_literal_declaration() {
            let source = r#"
import os
os.environ.setdefault(choose_key(), "other.settings")
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }

        #[test]
        fn rejects_whitespace_padded_module() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", " mysite.settings ")
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }

        #[test]
        fn rejects_whitespace_padded_module_before_literal_declaration() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", " mysite.settings ")
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "mysite.settings")
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }

        #[test]
        fn rejects_conflicting_declarations() {
            let source = r#"
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "site1.settings")
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "site2.settings")
"#;

            assert!(django_settings_module_from_manage_source(source).is_none());
        }
    }

    mod env_file {
        use std::io;

        use djls_source::CaseSensitivity;
        use djls_source::RootWalk;
        use djls_source::WalkOptions;

        use super::*;

        struct DotEnvDirectory;

        impl FileSystem for DotEnvDirectory {
            fn read_to_string(&self, _path: &Utf8Path) -> io::Result<String> {
                Ok("SHOULD_NOT_BE_READ=true".to_string())
            }

            fn exists(&self, path: &Utf8Path) -> bool {
                path == Utf8Path::new("/project/.env")
            }

            fn is_file(&self, _path: &Utf8Path) -> bool {
                false
            }

            fn is_dir(&self, path: &Utf8Path) -> bool {
                path == Utf8Path::new("/project/.env")
            }

            fn case_sensitivity(&self) -> CaseSensitivity {
                CaseSensitivity::CaseSensitive
            }

            fn path_exists_case_sensitive(&self, path: &Utf8Path, _prefix: &Utf8Path) -> bool {
                self.exists(path)
            }

            fn walk_root(&self, _root: &Utf8Path, _options: &WalkOptions) -> RootWalk {
                RootWalk::Missing
            }
        }

        #[test]
        fn loads_default_dot_env() {
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(
                dir.path().join(".env"),
                "DJANGO_SECRET_KEY=test-secret\nDATABASE_URL=postgres://localhost/db\n",
            )
            .expect("test .env file should be written");

            let settings = Settings::default();
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

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
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(dir.path().join(".env.local"), "MY_VAR=hello\n")
                .expect("test .env.local file should be written");
            fs::write(dir.path().join("djls.toml"), r#"env_file = ".env.local""#)
                .expect("test djls.toml file should be written");

            let settings = Settings::new(root, None).expect("test settings should parse");
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0], ("MY_VAR".to_string(), "hello".to_string()));
        }

        #[test]
        fn returns_empty_when_no_env_file() {
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");

            let settings = Settings::default();
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

            assert!(vars.is_empty());
        }

        #[test]
        fn does_not_read_default_dot_env_directory() {
            let settings = Settings::default();
            let vars = load_env_file(&DotEnvDirectory, Utf8Path::new("/project"), &settings);

            assert!(vars.is_empty());
        }

        #[test]
        fn returns_empty_when_configured_file_missing() {
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(
                dir.path().join("djls.toml"),
                r#"env_file = ".env.nonexistent""#,
            )
            .expect("test djls.toml file should be written");

            let settings = Settings::new(root, None).expect("test settings should parse");
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

            assert!(vars.is_empty());
        }

        #[test]
        fn handles_comments_and_blank_lines() {
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(
                dir.path().join(".env"),
                "# This is a comment\n\nDJANGO_SECRET_KEY=secret\n\n# Another comment\nDEBUG=true\n",
            )
            .expect("test .env file should be written");

            let settings = Settings::default();
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(vars[0].0, "DJANGO_SECRET_KEY");
            assert_eq!(vars[1].0, "DEBUG");
        }

        #[test]
        fn handles_quoted_values() {
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(
                dir.path().join(".env"),
                "SECRET=\"my secret value\"\nOTHER='single quoted'\n",
            )
            .expect("test .env file should be written");

            let settings = Settings::default();
            let vars = load_env_file(&djls_source::OsFileSystem::default(), root, &settings);

            assert_eq!(vars.len(), 2);
            assert_eq!(
                vars[0],
                ("SECRET".to_string(), "my secret value".to_string())
            );
            assert_eq!(vars[1], ("OTHER".to_string(), "single quoted".to_string()));
        }

        #[test]
        fn malformed_entry_warning_does_not_include_file_contents() {
            const SENTINEL_PATH: &str = "private-sentinel.env";
            const SENTINEL_KEY: &str = "DJLS_PRIVATE_SENTINEL_KEY";
            const SENTINEL_VALUE: &str = "djls-sentinel-secret-do-not-log";
            let dir = tempdir().expect("test temporary directory should be created");
            let root = Utf8Path::from_path(dir.path())
                .expect("test temporary directory path should convert to UTF-8");
            fs::write(
                dir.path().join(SENTINEL_PATH),
                format!("{SENTINEL_KEY}=safe\nBROKEN=\"{SENTINEL_VALUE}"),
            )
            .expect("test env file should be written");
            fs::write(
                dir.path().join("djls.toml"),
                format!("env_file = \"{SENTINEL_PATH}\""),
            )
            .expect("test settings file should be written");
            let settings = Settings::new(root, None).expect("test settings should parse");

            let (vars, events) = capture_events(|| {
                load_env_file(&djls_source::OsFileSystem::default(), root, &settings)
            });

            assert_eq!(vars, vec![(SENTINEL_KEY.to_string(), "safe".to_string())]);
            let visible = &events.default_visible;
            assert!(visible.contains("parse_error_count=1"), "{visible}");
            assert!(visible.contains("variable_count=1"), "{visible}");
            for private in [SENTINEL_PATH, SENTINEL_KEY, SENTINEL_VALUE] {
                assert!(!visible.contains(private), "leaked {private}: {visible}");
            }
            let debug = &events.debug;
            assert!(debug.contains(SENTINEL_PATH), "{debug}");
            assert!(debug.contains(SENTINEL_KEY), "{debug}");
            assert!(!debug.contains(SENTINEL_VALUE), "leaked value: {debug}");
        }
    }

    #[test]
    fn settings_auto_detection_info_does_not_include_source_module() {
        const SENTINEL_MODULE: &str = "private_sentinel.settings";
        let dir = tempdir().expect("test temporary directory should be created");
        let root = Utf8Path::from_path(dir.path())
            .expect("test temporary directory path should convert to UTF-8");
        fs::write(
            dir.path().join("manage.py"),
            format!(
                "import os\nos.environ.setdefault(\"DJANGO_SETTINGS_MODULE\", \
                 \"{SENTINEL_MODULE}\")\n"
            ),
        )
        .expect("test manage.py should be written");

        let (module, events) = capture_events(|| {
            django_settings_module_name(
                &djls_source::OsFileSystem::default(),
                root,
                &Settings::default(),
                None,
            )
        });

        assert_eq!(module.expect("settings module").as_str(), SENTINEL_MODULE);
        let visible = &events.default_visible;
        assert!(visible.contains("source=manage_py"), "{visible}");
        assert!(!visible.contains(SENTINEL_MODULE), "{visible}");
        assert!(events.debug.contains(SENTINEL_MODULE), "{}", events.debug);
    }
}
