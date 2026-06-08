use std::ops::Deref;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use salsa::Durability;

use crate::collections::FxDashMap;
use crate::db::Db;
use crate::line::LineIndex;
use crate::position::LineCol;
use crate::protocol::PositionEncoding;

#[salsa::input]
pub struct File {
    // TODO(virtual-paths): This will accept synthetic paths for virtual documents
    // e.g., /virtual/untitled/Untitled-1.html derived from untitled:Untitled-1
    #[returns(ref)]
    pub path: Utf8PathBuf,
    /// The revision number for invalidation tracking
    pub revision: u64,
}

#[salsa::tracked]
impl File {
    #[salsa::tracked]
    pub fn source(self, db: &dyn Db) -> SourceText {
        let _ = self.revision(db);
        let path = self.path(db);
        let source = db.read_file(path).unwrap_or_default();
        SourceText::new(path, source)
    }

    #[salsa::tracked(returns(ref))]
    pub fn line_index(self, db: &dyn Db) -> LineIndex {
        let text = self.source(db);
        LineIndex::from(text.as_str())
    }

    #[must_use]
    pub fn end_line_col(self, db: &dyn Db, encoding: PositionEncoding) -> LineCol {
        let source = self.source(db);
        let line_index = self.line_index(db);
        line_index.end_line_col(source.as_str(), encoding)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceText(Arc<SourceTextInner>);

impl SourceText {
    #[must_use]
    pub fn new(path: &Utf8Path, source: String) -> Self {
        let encoding = FileEncoding::from(source.as_str());
        let kind = FileKind::from(path);
        Self(Arc::new(SourceTextInner {
            encoding,
            kind,
            source,
        }))
    }

    #[must_use]
    pub fn kind(&self) -> &FileKind {
        &self.0.kind
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0.source
    }
}

impl Default for SourceText {
    fn default() -> Self {
        Self(Arc::new(SourceTextInner {
            encoding: FileEncoding::Ascii,
            kind: FileKind::Other,
            source: String::new(),
        }))
    }
}

impl AsRef<str> for SourceText {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for SourceText {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceTextInner {
    encoding: FileEncoding,
    kind: FileKind,
    source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileEncoding {
    Ascii,
    Utf8,
}

impl From<&str> for FileEncoding {
    fn from(value: &str) -> Self {
        if value.is_ascii() {
            Self::Ascii
        } else {
            Self::Utf8
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum FileKind {
    Other,
    Python,
    Template,
}

impl FileKind {
    #[must_use]
    pub fn is_template(path: &Utf8Path) -> bool {
        Self::from(path) == Self::Template
    }
}

impl From<&str> for FileKind {
    fn from(value: &str) -> Self {
        match value {
            "py" => FileKind::Python,
            "djhtml" | "html" | "htm" => FileKind::Template,
            _ => FileKind::Other,
        }
    }
}

impl From<&Utf8Path> for FileKind {
    fn from(path: &Utf8Path) -> Self {
        match path.extension() {
            Some(ext) => Self::from(ext),
            _ => FileKind::Other,
        }
    }
}

impl From<&Utf8PathBuf> for FileKind {
    fn from(path: &Utf8PathBuf) -> Self {
        match path.extension() {
            Some(ext) => Self::from(ext),
            _ => FileKind::Other,
        }
    }
}

/// Registry that maps source paths to Salsa `File` inputs.
///
/// File durability is assigned when the `File` is first created. Register roots
/// before creating files beneath them.
#[derive(Clone, Default)]
pub struct SourceFiles(Arc<SourceFilesInner>);

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
    pub(crate) fn get_or_create_file<SalsaDb>(&self, db: &SalsaDb, path: &Utf8Path) -> File
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        let path = path.to_owned();
        *self.0.by_path.entry(path.clone()).or_insert_with(|| {
            File::builder(path.clone(), 0)
                .durability(self.durability_for(&path))
                .path_durability(Durability::HIGH)
                .new(db)
        })
    }

    fn roots(&self) -> RwLockReadGuard<'_, Vec<FileRoot>> {
        self.0
            .roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn roots_mut(&self) -> RwLockWriteGuard<'_, Vec<FileRoot>> {
        self.0
            .roots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a root for future file creation.
    ///
    /// If the same root already exists, its original kind is preserved.
    /// Existing files keep the durability assigned when they were created.
    pub fn try_add_root(&self, path: Utf8PathBuf, kind: FileRootKind) {
        let mut roots = self.roots_mut();
        if roots.iter().any(|root| root.path == path) {
            return;
        }

        roots.push(FileRoot { path, kind });
    }

    fn durability_for(&self, path: &Utf8Path) -> Durability {
        self.roots()
            .iter()
            .filter(|root| path.starts_with(root.path.as_path()))
            .max_by_key(|root| root.path.as_str().len())
            .map_or(Durability::LOW, |root| root.kind.durability())
    }
}
