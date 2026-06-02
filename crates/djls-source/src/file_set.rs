use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::File;
use crate::FileKind;
use crate::FileRootKind;

#[salsa::input]
#[derive(Debug)]
pub struct SourceFileSet {
    #[returns(ref)]
    pub data: SourceFileSetData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceFileSetData {
    roots: Vec<SourceRootEntry>,
    files: Vec<LoadedSourceFile>,
    summary: FileSetSummary,
}

impl SourceFileSetData {
    pub fn new(
        roots: Vec<SourceRootEntry>,
        files: Vec<LoadedSourceFile>,
    ) -> Result<Self, SourceFileSetInvariantError> {
        for (index, root) in roots.iter().enumerate() {
            if roots[..index]
                .iter()
                .any(|entry| entry.root().id() == root.root().id())
            {
                return Err(SourceFileSetInvariantError::DuplicateRootId {
                    root: root.root().id().clone(),
                    path: root.root().path().to_owned(),
                });
            }
        }

        if let Some(file) = files
            .iter()
            .find(|file| !roots.iter().any(|entry| entry.root().id() == file.root()))
        {
            return Err(SourceFileSetInvariantError::UnknownFileRoot {
                root: file.root().clone(),
                path: file.path().to_owned(),
            });
        }

        for (index, file) in files.iter().enumerate() {
            if files[..index]
                .iter()
                .any(|entry| entry.root() == file.root() && entry.path() == file.path())
            {
                return Err(SourceFileSetInvariantError::DuplicateFile {
                    root: file.root().clone(),
                    path: file.path().to_owned(),
                });
            }
        }

        let summary = FileSetSummary::new(files.len());
        Ok(Self {
            roots,
            files,
            summary,
        })
    }

    #[must_use]
    pub fn roots(&self) -> &[SourceRootEntry] {
        &self.roots
    }

    #[must_use]
    pub fn files(&self) -> &[LoadedSourceFile] {
        &self.files
    }

    #[must_use]
    pub fn summary(&self) -> &FileSetSummary {
        &self.summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFileSetInvariantError {
    DuplicateRootId {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
    DuplicateFile {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
    UnknownFileRoot {
        root: SourceRootId,
        path: Utf8PathBuf,
    },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceRootId(Utf8PathBuf);

impl SourceRootId {
    #[must_use]
    pub fn new(path: Utf8PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn as_path(&self) -> &Utf8Path {
        self.0.as_path()
    }

    #[must_use]
    pub fn into_path(self) -> Utf8PathBuf {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRoot {
    id: SourceRootId,
    path: Utf8PathBuf,
    kind: FileRootKind,
}

impl SourceRoot {
    #[must_use]
    pub fn new(id: SourceRootId, path: Utf8PathBuf, kind: FileRootKind) -> Self {
        Self { id, path, kind }
    }

    #[must_use]
    pub fn id(&self) -> &SourceRootId {
        &self.id
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn kind(&self) -> FileRootKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRootEntry {
    root: SourceRoot,
}

impl SourceRootEntry {
    #[must_use]
    pub fn new(root: SourceRoot) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &SourceRoot {
        &self.root
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredSourceFile {
    path: Utf8PathBuf,
    root: SourceRootId,
}

impl DiscoveredSourceFile {
    #[must_use]
    pub fn new(path: Utf8PathBuf, root: SourceRootId) -> Self {
        Self { path, root }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn root(&self) -> &SourceRootId {
        &self.root
    }

    #[must_use]
    pub fn kind(&self) -> FileKind {
        FileKind::from(self.path())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSourceFile {
    path: Utf8PathBuf,
    root: SourceRootId,
    file: File,
}

impl LoadedSourceFile {
    #[must_use]
    pub fn new(path: Utf8PathBuf, root: SourceRootId, file: File) -> Self {
        Self { path, root, file }
    }

    #[must_use]
    pub fn from_discovered(discovered: DiscoveredSourceFile, file: File) -> Self {
        Self::new(discovered.path, discovered.root, file)
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    #[must_use]
    pub fn root(&self) -> &SourceRootId {
        &self.root
    }

    #[must_use]
    pub fn kind(&self) -> FileKind {
        FileKind::from(self.path())
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct FileSetSummary {
    included_files: usize,
}

impl FileSetSummary {
    #[must_use]
    pub fn new(included_files: usize) -> Self {
        Self { included_files }
    }

    #[must_use]
    pub fn included_files(&self) -> usize {
        self.included_files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[test]
    fn source_file_set_stores_handle_bearing_data() {
        let db = TestDb::default();
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(&db, file_path.clone(), 0);
        let discovered = DiscoveredSourceFile::new(file_path.clone(), root_id.clone());
        let loaded = LoadedSourceFile::from_discovered(discovered, file);
        let data = SourceFileSetData::new(vec![SourceRootEntry::new(root)], vec![loaded])
            .expect("file root should be present");

        let set = SourceFileSet::new(&db, data);
        let data = set.data(&db);

        assert_eq!(data.roots().len(), 1);
        assert_eq!(data.files().len(), 1);
        assert_eq!(data.summary().included_files(), 1);
        assert_eq!(data.files()[0].path(), file_path.as_path());
        assert_eq!(data.files()[0].root(), &root_id);
        assert_eq!(data.files()[0].kind(), FileKind::Template);
        assert_eq!(data.files()[0].file(), file);
    }

    #[test]
    fn source_file_set_rejects_loaded_files_with_unknown_roots() {
        let db = TestDb::default();
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let unknown_root_id = SourceRootId::new(Utf8PathBuf::from("/other"));
        let root = SourceRoot::new(root_id, root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let file = File::new(&db, file_path.clone(), 0);
        let loaded = LoadedSourceFile::new(file_path.clone(), unknown_root_id.clone(), file);

        let error = SourceFileSetData::new(vec![SourceRootEntry::new(root)], vec![loaded])
            .expect_err("unknown file root should be rejected");

        assert_eq!(
            error,
            SourceFileSetInvariantError::UnknownFileRoot {
                root: unknown_root_id,
                path: file_path,
            }
        );
    }

    #[test]
    fn source_file_set_rejects_duplicate_files() {
        let db = TestDb::default();
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path.clone(), FileRootKind::Project);
        let file_path = root_path.join("templates/index.html");
        let first = File::new(&db, file_path.clone(), 0);
        let second = File::new(&db, file_path.clone(), 1);
        let files = vec![
            LoadedSourceFile::new(file_path.clone(), root_id.clone(), first),
            LoadedSourceFile::new(file_path.clone(), root_id.clone(), second),
        ];

        let error = SourceFileSetData::new(vec![SourceRootEntry::new(root)], files)
            .expect_err("duplicate files should be rejected");

        assert_eq!(
            error,
            SourceFileSetInvariantError::DuplicateFile {
                root: root_id,
                path: file_path,
            }
        );
    }

    #[test]
    fn source_file_set_rejects_duplicate_root_ids() {
        let root_id = SourceRootId::new(Utf8PathBuf::from("/identity"));
        let first = SourceRoot::new(
            root_id.clone(),
            Utf8PathBuf::from("/workspace/a"),
            FileRootKind::Project,
        );
        let second = SourceRoot::new(
            root_id.clone(),
            Utf8PathBuf::from("/workspace/b"),
            FileRootKind::LibrarySearchPath,
        );

        let error = SourceFileSetData::new(
            vec![SourceRootEntry::new(first), SourceRootEntry::new(second)],
            Vec::new(),
        )
        .expect_err("duplicate root ids should be rejected");

        assert_eq!(
            error,
            SourceFileSetInvariantError::DuplicateRootId {
                root: root_id,
                path: Utf8PathBuf::from("/workspace/b"),
            }
        );
    }

    #[test]
    fn file_kind_is_derived_from_path() {
        let db = TestDb::default();
        let root_id = SourceRootId::new(Utf8PathBuf::from("/workspace"));
        let discovered =
            DiscoveredSourceFile::new(Utf8PathBuf::from("/workspace/app.py"), root_id.clone());
        let file = File::new(&db, Utf8PathBuf::from("/workspace/template.html"), 0);
        let loaded =
            LoadedSourceFile::new(Utf8PathBuf::from("/workspace/template.html"), root_id, file);

        assert_eq!(discovered.kind(), FileKind::Python);
        assert_eq!(loaded.kind(), FileKind::Template);
    }

    #[test]
    fn discovered_source_file_has_no_file_handle() {
        let root_id = SourceRootId::new(Utf8PathBuf::from("/workspace"));
        let discovered =
            DiscoveredSourceFile::new(Utf8PathBuf::from("/workspace/app.py"), root_id.clone());

        assert_eq!(discovered.root(), &root_id);
        assert_eq!(discovered.kind(), FileKind::Python);
    }
}
