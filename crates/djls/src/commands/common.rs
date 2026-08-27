use std::fmt;
use std::io;
use std::io::IsTerminal;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::ValueEnum;
use djls_db::DjangoDatabase;
use djls_project::Db as _;
use djls_project::template_directories;
use djls_source::Db as _;
use djls_source::FileKind;
use djls_source::RootWalk;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum ColorMode {
    /// Use colors when output is a terminal.
    #[default]
    Auto,
    /// Always use colors.
    Always,
    /// Never use colors.
    Never,
}

impl ColorMode {
    pub(crate) fn should_use_color(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => std::io::stdout().is_terminal(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileDiscoveryError {
    Missing(Utf8PathBuf),
    UnsupportedFile(Utf8PathBuf),
    Inaccessible {
        path: Utf8PathBuf,
        kind: io::ErrorKind,
    },
}

impl fmt::Display for FileDiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "Cannot check `{path}`: path does not exist"),
            Self::UnsupportedFile(path) => write!(
                f,
                "Cannot check `{path}`: expected a .html, .htm, or .djhtml file"
            ),
            Self::Inaccessible { path, kind } => {
                write!(f, "Cannot check `{path}`: {}", io::Error::from(*kind))
            }
        }
    }
}

impl std::error::Error for FileDiscoveryError {}

pub(crate) fn discover_files(
    paths: &[Utf8PathBuf],
    db: &DjangoDatabase,
    project_root: &Utf8Path,
    options: &WalkOptions,
) -> std::result::Result<Vec<Utf8PathBuf>, FileDiscoveryError> {
    let roots = discovery_roots(paths, db, project_root);
    let explicit = !paths.is_empty();

    let mut files = Vec::new();
    for path in &roots {
        let entries = match db.walk_root(path, options) {
            RootWalk::File(entry) => {
                if explicit && !is_template(&entry.path) {
                    return Err(FileDiscoveryError::UnsupportedFile(entry.path));
                }
                vec![entry]
            }
            RootWalk::Directory { entries, issues } => {
                if explicit && let Some(kind) = issues.into_iter().next() {
                    return Err(FileDiscoveryError::Inaccessible {
                        path: path.clone(),
                        kind,
                    });
                }
                entries
            }
            RootWalk::Missing if explicit => {
                return Err(FileDiscoveryError::Missing(path.clone()));
            }
            RootWalk::Inaccessible(kind) if explicit => {
                return Err(FileDiscoveryError::Inaccessible {
                    path: path.clone(),
                    kind,
                });
            }
            RootWalk::Missing | RootWalk::Inaccessible(_) => continue,
        };
        for entry in entries {
            if entry.kind != WalkEntryKind::File || !is_template(&entry.path) {
                continue;
            }

            let path = match entry.path.as_std_path().canonicalize() {
                Ok(canonical) => {
                    #[cfg(windows)]
                    let canonical = dunce::simplified(&canonical).to_path_buf();
                    Utf8PathBuf::from_path_buf(canonical).unwrap_or(entry.path)
                }
                Err(_) => entry.path,
            };
            files.push(path);
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

/// Selects the directories a batch command enumerates templates from.
///
/// Explicit CLI paths always win. Otherwise every known template root is scanned, and the
/// project root is added only when configuration may omit roots: a batch scan would rather
/// visit extra files than silently skip templates the settings could not enumerate. A fully
/// extracted configuration with no roots gets no fallback, so the scan does not invent roots
/// the project never declared.
fn discovery_roots(
    paths: &[Utf8PathBuf],
    db: &DjangoDatabase,
    project_root: &Utf8Path,
) -> Vec<Utf8PathBuf> {
    if !paths.is_empty() {
        return paths
            .iter()
            .map(|path| {
                if path.is_relative() {
                    project_root.join(path)
                } else {
                    path.clone()
                }
            })
            .collect();
    }

    let Some(project) = db.project() else {
        return vec![project_root.to_owned()];
    };
    let directories = template_directories(db, project);
    let mut roots: Vec<Utf8PathBuf> = directories
        .known_roots()
        .map(Utf8Path::to_path_buf)
        .collect();
    if directories.settings_cases_may_omit_roots() && !roots.iter().any(|root| root == project_root)
    {
        roots.push(project_root.to_owned());
    }
    roots
}

pub(crate) fn resolve_project_root() -> Result<Utf8PathBuf> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| anyhow::anyhow!("Current directory is not valid UTF-8: {}", path.display()))
}

pub(crate) fn is_template(path: &Utf8Path) -> bool {
    FileKind::is_template(path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use djls_conf::Settings;
    use djls_source::CaseSensitivity;
    use djls_source::FileSystem;
    use djls_source::OsFileSystem;

    use super::*;

    struct InaccessibleDirectoryFileSystem;

    impl FileSystem for InaccessibleDirectoryFileSystem {
        fn read_to_string(&self, _path: &Utf8Path) -> io::Result<String> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }

        fn exists(&self, _path: &Utf8Path) -> bool {
            true
        }

        fn is_file(&self, _path: &Utf8Path) -> bool {
            false
        }

        fn is_dir(&self, _path: &Utf8Path) -> bool {
            true
        }

        fn case_sensitivity(&self) -> CaseSensitivity {
            CaseSensitivity::CaseSensitive
        }

        fn path_exists_case_sensitive(&self, _path: &Utf8Path, _prefix: &Utf8Path) -> bool {
            true
        }

        fn walk_root(&self, _root: &Utf8Path, _options: &WalkOptions) -> RootWalk {
            RootWalk::Directory {
                entries: Vec::new(),
                issues: vec![io::ErrorKind::PermissionDenied],
            }
        }
    }

    fn project_database(project_root: &Utf8Path) -> anyhow::Result<DjangoDatabase> {
        let settings = Settings::new(project_root, None)?;
        let mut db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &settings,
            Some(project_root),
        );
        db.apply_project_settings(settings);
        Ok(db)
    }

    fn write_settings_project(
        project_root: &Utf8Path,
        settings_source: &str,
    ) -> std::io::Result<()> {
        std::fs::write(
            project_root.join("djls.toml"),
            "django_settings_module = \"settings\"\n",
        )?;
        std::fs::write(project_root.join("settings.py"), settings_source)
    }

    #[test]
    fn explicit_paths_take_precedence_over_discovered_roots() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        let configured = root.join("configured");
        write_settings_project(
            &root,
            &format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{configured}'], 'APP_DIRS': False}}]\n"
            ),
        )
        .expect("test settings project should be written");
        let db = project_database(&root).expect("test project database should be created");

        assert_eq!(
            discovery_roots(&[Utf8PathBuf::from("explicit")], &db, &root),
            [root.join("explicit")]
        );
    }

    #[test]
    fn no_paths_use_closed_known_roots_without_project_fallback() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        write_settings_project(
            &root,
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False}]\n",
        )
        .expect("test settings project should be written");
        let db = project_database(&root).expect("test project database should be created");

        assert!(discovery_roots(&[], &db, &root).is_empty());
    }

    #[test]
    fn incomplete_roots_add_project_root_without_duplicates() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        write_settings_project(
            &root,
            &format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{root}', dynamic()], 'APP_DIRS': False}}]\n"
            ),
        )
        .expect("test settings project should be written");
        let db = project_database(&root).expect("test project database should be created");

        assert_eq!(discovery_roots(&[], &db, &root), [root]);
    }

    #[test]
    fn discovers_templates_under_explicit_directory() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        std::fs::write(dir.path().join("page.html"), "page")
            .expect("page.html fixture should be written");
        std::fs::write(dir.path().join("style.css"), "style")
            .expect("style.css fixture should be written");
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );

        let files = discover_files(
            std::slice::from_ref(&dir_path),
            &db,
            &dir_path,
            &WalkOptions::default(),
        )
        .expect("explicit Template directory should be discovered");
        let names: Vec<_> = files.iter().filter_map(|path| path.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"style.css"));
    }

    #[test]
    fn discovers_explicit_file_path() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        let file_path = Utf8PathBuf::from_path_buf(dir.path().join("single.html"))
            .expect("temporary test path should be valid UTF-8");
        std::fs::write(file_path.as_std_path(), "single")
            .expect("single.html fixture should be written");
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );

        let files = discover_files(
            std::slice::from_ref(&file_path),
            &db,
            &dir_path,
            &WalkOptions::default(),
        )
        .expect("explicit Template file should be discovered");

        let canonical = Utf8PathBuf::from_path_buf(
            file_path
                .as_std_path()
                .canonicalize()
                .expect("test fixture path should be canonicalized"),
        )
        .expect("canonical test fixture path should be valid UTF-8");
        assert_eq!(files, vec![canonical]);
    }

    #[test]
    fn rejects_explicit_files_with_unsupported_types() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let project_root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );

        for name in ["style.css", "README"] {
            let path = project_root.join(name);
            std::fs::write(path.as_std_path(), "not a Template")
                .expect("unsupported test file should be written");

            let error = discover_files(
                std::slice::from_ref(&path),
                &db,
                &project_root,
                &WalkOptions::default(),
            )
            .expect_err("an explicit non-Template file should fail discovery");

            assert_eq!(error, FileDiscoveryError::UnsupportedFile(path.clone()));
            assert_eq!(
                error.to_string(),
                format!("Cannot check `{path}`: expected a .html, .htm, or .djhtml file")
            );
        }
    }

    #[test]
    fn deduplicates_explicit_file_and_directory_results() {
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        let file_path = Utf8PathBuf::from_path_buf(dir.path().join("page.html"))
            .expect("temporary test path should be valid UTF-8");
        std::fs::write(file_path.as_std_path(), "page")
            .expect("page.html fixture should be written");
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );

        let files = discover_files(
            &[dir_path.clone(), file_path],
            &db,
            &dir_path,
            &WalkOptions::default(),
        )
        .expect("explicit Template paths should be discovered");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name(), Some("page.html"));
    }

    #[test]
    fn missing_explicit_path_is_an_error() {
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let project_root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .expect("temporary test path should be valid UTF-8");
        let missing = project_root.join("missing.html");

        let error = discover_files(
            std::slice::from_ref(&missing),
            &db,
            &project_root,
            &WalkOptions::default(),
        )
        .expect_err("a missing explicit path should fail discovery");

        assert_eq!(error, FileDiscoveryError::Missing(missing.clone()));
        assert_eq!(
            error.to_string(),
            format!("Cannot check `{missing}`: path does not exist")
        );
    }

    #[test]
    fn inaccessible_explicit_directory_is_an_error() {
        let db = DjangoDatabase::new(
            Arc::new(InaccessibleDirectoryFileSystem),
            &Settings::default(),
            None,
        );
        let project_root = Utf8Path::new("/project");
        let inaccessible = Utf8PathBuf::from("/project/templates");

        let error = discover_files(
            std::slice::from_ref(&inaccessible),
            &db,
            project_root,
            &WalkOptions::default(),
        )
        .expect_err("an inaccessible explicit directory should fail discovery");

        assert_eq!(
            error,
            FileDiscoveryError::Inaccessible {
                path: inaccessible,
                kind: io::ErrorKind::PermissionDenied,
            }
        );
    }

    #[test]
    fn missing_implicit_root_produces_empty_discovery() {
        let db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            None,
        );
        let dir = tempfile::tempdir().expect("temporary test directory should be created");
        let project_root = Utf8PathBuf::from_path_buf(dir.path().join("missing-project"))
            .expect("temporary test path should be valid UTF-8");

        let files = discover_files(&[], &db, &project_root, &WalkOptions::default())
            .expect("a missing implicit root should be an empty discovery");

        assert!(files.is_empty());
    }
}
