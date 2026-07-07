use std::io::IsTerminal;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::ValueEnum;
use djls_db::DjangoDatabase;
use djls_semantic::Db as _;
use djls_source::Db as _;
use djls_source::FileKind;
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
    let roots: Vec<Utf8PathBuf> = if !paths.is_empty() {
        paths
            .iter()
            .map(|path| {
                if path.is_relative() {
                    project_root.join(path)
                } else {
                    path.clone()
                }
            })
            .collect()
    } else if let Some(dirs) = db.template_dirs() {
        dirs.into_iter().collect()
    } else {
        vec![project_root.to_owned()]
    };

    let mut files = Vec::new();
    for path in &roots {
        if db.path_is_file(path) {
            if is_template(path) {
                let path = match path.as_std_path().canonicalize() {
                    Ok(canonical) => {
                        #[cfg(windows)]
                        let canonical = dunce::simplified(&canonical).to_path_buf();
                        Utf8PathBuf::from_path_buf(canonical).unwrap_or_else(|_| path.clone())
                    }
                    Err(_) => path.clone(),
                };
                files.push(path);
            }
            continue;
        }

        if !db.path_is_dir(path) {
            continue;
        }

        let Ok(entries) = db.walk_entries(path, options) else {
            continue;
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
