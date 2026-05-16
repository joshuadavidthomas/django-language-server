use std::sync::Arc;
use std::sync::RwLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use salsa::Durability;

use crate::collections::FxDashMap;
use crate::File;

/// Registry that maps source paths to Salsa `File` inputs.
///
/// File durability is assigned when the `File` is first created. Register roots
/// before creating files beneath them.
#[derive(Clone, Default)]
pub struct SourceFiles {
    inner: Arc<SourceFilesInner>,
}

#[derive(Default)]
struct SourceFilesInner {
    by_path: FxDashMap<Utf8PathBuf, File>,
    roots: RwLock<Vec<FileRoot>>,
}

/// A source root as known when files are created.
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileRoot {
    path: Utf8PathBuf,
    kind: FileRootKind,
}

/// Classification used to assign durability to files under a root.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FileRootKind {
    /// First-party files edited by the user.
    Project,
    /// Dependency files from import/search paths.
    LibrarySearchPath,
}

impl FileRootKind {
    const fn durability(self) -> Durability {
        match self {
            Self::Project => Durability::LOW,
            Self::LibrarySearchPath => Durability::HIGH,
        }
    }
}

impl SourceFiles {
    #[must_use]
    pub fn get(&self, path: &Utf8Path) -> Option<File> {
        self.inner.by_path.get(path).map(|entry| *entry)
    }

    #[must_use]
    pub fn get_or_create<Db>(&self, db: &Db, path: &Utf8Path) -> File
    where
        Db: salsa::Database + ?Sized,
    {
        let path = path.to_owned();
        *self.inner.by_path.entry(path.clone()).or_insert_with(|| {
            File::builder(path.clone(), 0)
                .durability(self.durability_for(&path))
                .path_durability(Durability::HIGH)
                .new(db)
        })
    }

    /// Register a root for future file creation.
    ///
    /// If the same root already exists, its original kind is preserved.
    /// Existing files keep the durability assigned when they were created.
    pub fn try_add_root(&self, path: Utf8PathBuf, kind: FileRootKind) {
        let mut roots = self
            .inner
            .roots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if roots.iter().any(|root| root.path == path) {
            return;
        }

        roots.push(FileRoot { path, kind });
    }

    fn durability_for(&self, path: &Utf8Path) -> Durability {
        let roots = self
            .inner
            .roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        roots
            .iter()
            .filter(|root| path.starts_with(root.path.as_path()))
            .max_by_key(|root| root.path.as_str().len())
            .map_or(Durability::LOW, |root| root.kind.durability())
    }
}
