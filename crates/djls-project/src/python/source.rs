use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

/// Source text plus the file identity that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSource {
    source: String,
    file: File,
    path: Utf8PathBuf,
}

impl PythonSource {
    pub(crate) fn new(file: File, path: Utf8PathBuf, source: String) -> Self {
        Self { source, file, path }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }

    pub(crate) fn path(&self) -> &Utf8Path {
        &self.path
    }
}
