use std::io::IsTerminal;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::ValueEnum;
use djls_db::DjangoDatabase;
use djls_semantic::Db as _;
use djls_source::FileKind;
use djls_workspace::walk_files;
use djls_workspace::WalkOptions;

#[derive(Clone, Debug, Default, ValueEnum)]
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
    pub(crate) fn should_use_color(&self) -> bool {
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
    if !paths.is_empty() {
        let resolved: Vec<Utf8PathBuf> = paths
            .iter()
            .map(|path| {
                if path.is_relative() {
                    project_root.join(path)
                } else {
                    path.clone()
                }
            })
            .collect();

        return walk_files(&resolved, is_template, options);
    }

    if let Some(dirs) = db.template_dirs() {
        let dirs: Vec<Utf8PathBuf> = dirs.into_iter().collect();
        walk_files(&dirs, is_template, options)
    } else {
        walk_files(&[project_root.to_owned()], is_template, options)
    }
}

pub(crate) fn resolve_project_root() -> Result<Utf8PathBuf> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|_| anyhow::anyhow!("Current directory is not valid UTF-8"))
}

pub(crate) fn is_template(path: &Utf8Path) -> bool {
    FileKind::is_template(path)
}
