use std::ops::Deref;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::db::Db;

#[salsa::input]
pub struct File {
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
        let source = db.read_file_source(path).unwrap_or_default();
        SourceText::new(path, source)
    }

    #[salsa::tracked(returns(ref))]
    pub fn line_index(self, db: &dyn Db) -> LineIndex {
        let text = self.source(db);
        let mut starts = Vec::with_capacity(256);
        starts.push(0);
        for (i, b) in text.0.source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(u32::try_from(i).unwrap_or_default() + 1);
            }
        }
        LineIndex(starts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceText(Arc<SourceTextInner>);

impl SourceText {
    #[must_use]
    pub fn new(path: &Utf8Path, source: String) -> Self {
        let encoding = if source.is_ascii() {
            FileEncoding::Ascii
        } else {
            FileEncoding::Utf8
        };
        let kind = FileKind::from_path(path);
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
pub enum FileEncoding {
    Ascii,
    Utf8,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum FileKind {
    Other,
    Python,
    Template,
}

impl FileKind {
    /// Determine [`FileKind`] from a file path extension.
    #[must_use]
    pub fn from_path(path: &Utf8Path) -> Self {
        match path.extension() {
            Some("py") => FileKind::Python,
            Some("html" | "htm") => FileKind::Template,
            _ => FileKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex(Vec<u32>);
