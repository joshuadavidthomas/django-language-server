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

pub(crate) fn discover_files(
    paths: &[Utf8PathBuf],
    db: &DjangoDatabase,
    project_root: &Utf8Path,
    options: &WalkOptions,
) -> Vec<Utf8PathBuf> {
    let roots = discovery_roots(paths, db, project_root);

    let mut files = Vec::new();
    for path in &roots {
        let entries = match db.walk_root(path, options) {
            RootWalk::File(entry) => vec![entry],
            RootWalk::Directory { entries, .. } => entries,
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
    files
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
        .map_err(|_| anyhow::anyhow!("Current directory is not valid UTF-8"))
}

pub(crate) fn is_template(path: &Utf8Path) -> bool {
    FileKind::is_template(path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use djls_conf::Settings;
    use djls_source::OsFileSystem;

    use super::*;

    fn project_database(project_root: &Utf8Path) -> DjangoDatabase {
        let settings = Settings::new(project_root, None).unwrap();
        let mut db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &settings,
            Some(project_root),
        );
        db.apply_project_settings(settings);
        db
    }

    fn write_settings_project(project_root: &Utf8Path, settings_source: &str) {
        std::fs::write(
            project_root.join("djls.toml"),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        std::fs::write(project_root.join("settings.py"), settings_source).unwrap();
    }

    #[test]
    fn explicit_paths_take_precedence_over_discovered_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let configured = root.join("configured");
        write_settings_project(
            &root,
            &format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{configured}'], 'APP_DIRS': False}}]\n"
            ),
        );
        let db = project_database(&root);

        assert_eq!(
            discovery_roots(&[Utf8PathBuf::from("explicit")], &db, &root),
            [root.join("explicit")]
        );
    }

    #[test]
    fn no_paths_use_closed_known_roots_without_project_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        write_settings_project(
            &root,
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False}]\n",
        );
        let db = project_database(&root);

        assert!(discovery_roots(&[], &db, &root).is_empty());
    }

    #[test]
    fn incomplete_roots_add_project_root_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        write_settings_project(
            &root,
            &format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{root}', dynamic()], 'APP_DIRS': False}}]\n"
            ),
        );
        let db = project_database(&root);

        assert_eq!(discovery_roots(&[], &db, &root), [root]);
    }

    #[test]
    fn discovers_templates_under_explicit_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(dir.path().join("page.html"), "page").unwrap();
        std::fs::write(dir.path().join("style.css"), "style").unwrap();
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
        );
        let names: Vec<_> = files.iter().filter_map(|path| path.file_name()).collect();

        assert!(names.contains(&"page.html"));
        assert!(!names.contains(&"style.css"));
    }

    #[test]
    fn discovers_explicit_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let file_path = Utf8PathBuf::from_path_buf(dir.path().join("single.html")).unwrap();
        std::fs::write(file_path.as_std_path(), "single").unwrap();
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
        );

        let canonical =
            Utf8PathBuf::from_path_buf(file_path.as_std_path().canonicalize().unwrap()).unwrap();
        assert_eq!(files, vec![canonical]);
    }

    #[test]
    fn deduplicates_explicit_file_and_directory_results() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let file_path = Utf8PathBuf::from_path_buf(dir.path().join("page.html")).unwrap();
        std::fs::write(file_path.as_std_path(), "page").unwrap();
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
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name(), Some("page.html"));
    }
}
