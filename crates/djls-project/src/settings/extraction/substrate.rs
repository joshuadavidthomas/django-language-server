use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

/// `from X import name`; the substrate resolves the imported source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SettingsImport {
    pub(crate) level: u32,
    pub(crate) module: Option<String>,
}

/// Resolved Python source for a settings import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsSource {
    source: String,
    file: File,
    path: Utf8PathBuf,
}

impl SettingsSource {
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

/// Import-following seam owned by the extraction substrate.
pub(crate) trait SettingsImportResolver {
    fn resolve_star_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;

    fn resolve_named_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;
}
