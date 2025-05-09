use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct ProjectMetadata {
    root: PathBuf,
    venv: Option<PathBuf>,
}

impl ProjectMetadata {
    pub fn new(root: PathBuf, venv: Option<PathBuf>) -> Self {
        ProjectMetadata { root, venv }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn venv(&self) -> Option<&PathBuf> {
        self.venv.as_ref()
    }
}
