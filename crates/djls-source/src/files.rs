use std::ops::Deref;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use salsa::Durability;
use salsa::Setter;

use crate::collections::FxDashMap;
use crate::db::Db;
use crate::line::LineIndex;
use crate::position::LineCol;
use crate::protocol::PositionEncoding;

#[salsa::input]
#[derive(Debug)]
pub struct File {
    // TODO(virtual-paths): This will accept synthetic paths for virtual documents
    // e.g., /virtual/untitled/Untitled-1.html derived from untitled:Untitled-1
    #[returns(ref)]
    pub path: Utf8PathBuf,
    /// The revision number for invalidation tracking
    pub revision: u64,
    pub status: FileStatus,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FileStatus {
    Exists,
    IsADirectory,
    NotFound,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    #[error("Is a directory")]
    IsADirectory,
    #[error("Not found")]
    NotFound,
}

fn file_status(db: &dyn Db, path: &Utf8Path) -> FileStatus {
    let file_system = db.file_system();
    if file_system.is_file(path) {
        FileStatus::Exists
    } else if file_system.is_dir(path) {
        FileStatus::IsADirectory
    } else {
        FileStatus::NotFound
    }
}

/// Get or create a tracked file for `path` and return it if it exists.
#[inline]
pub fn path_to_file(db: &dyn Db, path: &Utf8Path) -> Result<File, FileError> {
    let file = db.files().get_or_create_file(db, path);
    match file.status(db) {
        FileStatus::Exists => Ok(file),
        FileStatus::IsADirectory => Err(FileError::IsADirectory),
        FileStatus::NotFound => Err(FileError::NotFound),
    }
}

#[salsa::input]
#[derive(Debug)]
pub struct FileRoot {
    #[returns(ref)]
    pub path: Utf8PathBuf,
    pub kind: FileRootKind,
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

    pub fn sync(self, db: &mut dyn Db) {
        let path = self.path(db).clone();
        Self::sync_path(db, &path);
    }

    pub fn sync_path(db: &mut dyn Db, path: &Utf8Path) {
        let Some(file) = db.files().try_file(path) else {
            return;
        };

        let current_status = file.status(db);
        let next_status = file_status(db, path);
        if current_status == next_status {
            return;
        }

        file.set_status(db).to(next_status);
        if matches!(current_status, FileStatus::Exists) != matches!(next_status, FileStatus::Exists)
        {
            db.bump_file_revision(file);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceText(Arc<SourceTextInner>);

impl SourceText {
    #[must_use]
    fn new(path: &Utf8Path, source: String) -> Self {
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

/// Classification used to assign durability to files under a root.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FileRootKind {
    /// User-edited files: project code and extra Python paths.
    Project,
    /// Installed packages discovered through library search paths; these
    /// rarely change during a session.
    SearchPath,
}

impl FileRootKind {
    const fn durability(self) -> Durability {
        match self {
            Self::Project => Durability::LOW,
            Self::SearchPath => Durability::HIGH,
        }
    }
}

impl SourceFiles {
    #[must_use]
    fn get_or_create_file(&self, db: &dyn Db, path: &Utf8Path) -> File {
        let path = path.to_owned();
        *self.0.by_path.entry(path.clone()).or_insert_with(|| {
            File::builder(path.clone(), 0, file_status(db, &path))
                .durability(self.durability_for(db, &path))
                .path_durability(Durability::HIGH)
                .new(db)
        })
    }

    #[must_use]
    pub fn try_file(&self, path: &Utf8Path) -> Option<File> {
        self.0.by_path.get(path).map(|entry| *entry.value())
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

    #[must_use]
    pub fn root<SalsaDb>(&self, db: &SalsaDb, path: &Utf8Path) -> Option<FileRoot>
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        self.roots()
            .iter()
            .filter(|root| path.starts_with(root.path(db).as_path()))
            .max_by_key(|root| root.path(db).as_str().len())
            .copied()
    }

    /// Return the registered source root that contains `path`.
    ///
    /// # Panics
    ///
    /// Panics when no registered source root contains `path`.
    #[must_use]
    pub fn expect_root<SalsaDb>(&self, db: &SalsaDb, path: &Utf8Path) -> FileRoot
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        self.root(db, path)
            .unwrap_or_else(|| panic!("expected registered source root for {path}"))
    }

    /// Register a root for future file creation.
    ///
    /// If the same root already exists, its original kind is preserved.
    /// Existing files keep the durability assigned when they were created.
    pub fn try_add_root<SalsaDb>(
        &self,
        db: &SalsaDb,
        path: Utf8PathBuf,
        kind: FileRootKind,
    ) -> FileRoot
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        let mut roots = self.roots_mut();
        if let Some(root) = roots.iter().find(|root| root.path(db) == &path) {
            return *root;
        }

        let root = Self::new_root(db, path, kind);
        roots.push(root);
        root
    }

    /// Replace the active source roots with the supplied root set.
    ///
    /// Existing exact roots with the same kind are retained; obsolete roots stop
    /// participating in root lookup.
    pub fn replace_roots<SalsaDb>(&self, db: &SalsaDb, root_specs: Vec<(Utf8PathBuf, FileRootKind)>)
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        let mut roots = self.roots_mut();
        let existing = roots.clone();
        let next_roots = root_specs
            .into_iter()
            .map(|(path, kind)| {
                existing
                    .iter()
                    .find(|root| root.path(db) == &path && root.kind(db) == kind)
                    .copied()
                    .unwrap_or_else(|| Self::new_root(db, path, kind))
            })
            .collect();
        *roots = next_roots;
    }

    fn new_root<SalsaDb>(db: &SalsaDb, path: Utf8PathBuf, kind: FileRootKind) -> FileRoot
    where
        SalsaDb: salsa::Database + ?Sized,
    {
        FileRoot::builder(path, kind, 0)
            .durability(Durability::HIGH)
            .revision_durability(kind.durability())
            .new(db)
    }

    fn durability_for(&self, db: &dyn Db, path: &Utf8Path) -> Durability {
        self.root(db, path)
            .map_or(Durability::LOW, |root| root.kind(db).durability())
    }
}
