use std::ops::Deref;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::db::Db;
use crate::line::LineIndex;

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
        LineIndex::from(text.0.source.as_str())
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
pub enum FileEncoding {
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

impl From<&str> for FileKind {
    fn from(value: &str) -> Self {
        match value {
            "py" => FileKind::Python,
            "html" | "htm" => FileKind::Template,
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
