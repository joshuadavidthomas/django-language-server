use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ProjectMetadata {
    root: PathBuf,
    venv: Option<PathBuf>,
}

impl ProjectMetadata {
    #[must_use]
    pub fn new(root: PathBuf, venv: Option<PathBuf>) -> Self {
        ProjectMetadata { root, venv }
    }

    #[must_use]
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    #[must_use]
    pub fn venv(&self) -> Option<&PathBuf> {
        self.venv.as_ref()
    }
}
