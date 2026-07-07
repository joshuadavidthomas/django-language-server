use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::Spanned;
use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::ExtractionStatus;
use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::PythonPathBindings;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SettingsParseStatus {
    #[default]
    Parsed,
    Unparseable,
}

/// Observed values for one extracted Django setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SettingValues<T> {
    pub(crate) values: Vec<T>,
    pub(crate) extraction: ExtractionStatus,
}

impl<T> Default for SettingValues<T> {
    fn default() -> Self {
        Self::partial()
    }
}

impl<T> SettingValues<T> {
    pub(crate) fn full(values: Vec<T>) -> Self {
        Self::with_extraction(values, ExtractionStatus::Complete)
    }

    pub(crate) fn partial() -> Self {
        Self::with_extraction(Vec::new(), ExtractionStatus::Partial)
    }

    pub(crate) fn with_extraction(values: Vec<T>, extraction: ExtractionStatus) -> Self {
        Self { values, extraction }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }

    pub(crate) fn clear_to_partial(&mut self) {
        self.values.clear();
        self.extraction = ExtractionStatus::Partial;
    }
}

pub(crate) type InstalledAppsSetting = SettingValues<String>;

/// The statically extracted subset of Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateSettings {
    pub(crate) backends: Vec<TemplateBackend>,
    pub(crate) extraction: ExtractionStatus,
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self::partial()
    }
}

impl TemplateSettings {
    pub(crate) fn full(backends: Vec<TemplateBackend>) -> Self {
        Self {
            backends,
            extraction: ExtractionStatus::Complete,
        }
    }

    pub(crate) fn partial() -> Self {
        Self {
            backends: Vec::new(),
            extraction: ExtractionStatus::Partial,
        }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }

    pub(crate) fn clear_to_partial(&mut self) {
        self.backends.clear();
        self.extraction = ExtractionStatus::Partial;
    }
}

/// One entry in Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: Option<String>,
    pub(crate) dirs: Vec<TemplateDirPath>,
    pub(crate) app_dirs: Option<bool>,
    pub(crate) libraries: Vec<(String, PythonModuleName)>,
    pub(crate) builtins: Vec<PythonModuleName>,
    pub(crate) context_processors: Vec<Spanned<TemplateContextProcessorPath>>,
    pub(crate) extraction: ExtractionStatus,
}

impl Default for TemplateBackend {
    fn default() -> Self {
        Self {
            backend: None,
            dirs: Vec::new(),
            app_dirs: None,
            libraries: Vec::new(),
            builtins: Vec::new(),
            context_processors: Vec::new(),
            extraction: ExtractionStatus::Complete,
        }
    }
}

impl TemplateBackend {
    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }

    #[must_use]
    pub(crate) fn is_django_templates_backend(&self, backend_count: usize) -> bool {
        match self.backend.as_deref() {
            Some(DJANGO_TEMPLATES_BACKEND) => true,
            None if backend_count == 1 => true,
            _ => false,
        }
    }

    pub(crate) fn mark_partial(&mut self) {
        self.extraction = ExtractionStatus::Partial;
    }
}

/// A dotted context processor callable path from `TEMPLATES[*]["OPTIONS"]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub(crate) struct TemplateContextProcessorPath(String);

impl TemplateContextProcessorPath {
    pub(crate) fn parse(path: &str) -> Result<Self, InvalidModuleName> {
        let name = PythonModuleName::parse(path)?;
        Ok(Self(name.into_string()))
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) type ScalarSetting<T> = SettingValues<Spanned<T>>;

/// The statically extracted subset of Django's staticfiles settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub(crate) struct StaticFilesSettings {
    pub(crate) static_url: ScalarSetting<String>,
    pub(crate) static_root: ScalarSetting<TemplateDirPath>,
    pub(crate) staticfiles_dirs: StaticFilesDirsSetting,
}

pub(crate) type StaticFilesDirsSetting = SettingValues<Spanned<TemplateDirPath>>;

/// The statically extracted subset of a Django settings module.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub(crate) struct DjangoSettings {
    pub(crate) parse_status: SettingsParseStatus,
    pub(crate) installed_apps: InstalledAppsSetting,
    pub(crate) templates: TemplateSettings,
    pub(crate) staticfiles: StaticFilesSettings,
}

/// A path expression evaluated against the settings file's own location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TemplateDirPath {
    Resolved(Utf8PathBuf),
    Unknown,
}

/// `from X import name`; the caller resolves the imported source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SettingsImport {
    pub(crate) level: u32,
    pub(crate) module: Option<String>,
}

/// Resolved source for a settings import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsSource {
    pub(crate) source: String,
    pub(crate) path: Utf8PathBuf,
}

/// Caller-supplied source lookup for settings imports.
pub(crate) trait SettingsSourceResolver {
    /// Return the source for a star-imported module, or `None` if it cannot be resolved.
    fn resolve_star_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;

    /// Return the source for a named-imported module, or `None` if it cannot be followed.
    fn resolve_named_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalListBinding {
    pub(crate) values: Vec<String>,
    extraction: ExtractionStatus,
}

impl LocalListBinding {
    pub(crate) fn full(values: Vec<String>) -> Self {
        Self {
            values,
            extraction: ExtractionStatus::Complete,
        }
    }

    pub(crate) fn partial(values: Vec<String>) -> Self {
        Self {
            values,
            extraction: ExtractionStatus::Partial,
        }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        self.extraction == ExtractionStatus::Complete
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LocalBindings {
    bools: FxHashMap<String, bool>,
    paths: PythonPathBindings,
    lists: FxHashMap<String, LocalListBinding>,
}

impl LocalBindings {
    pub(crate) fn extend(&mut self, other: Self) {
        for name in other.bools.keys() {
            self.paths.remove(name.as_str());
            self.lists.remove(name.as_str());
        }
        for name in other.paths.names() {
            self.bools.remove(name);
            self.lists.remove(name);
        }
        for name in other.lists.keys() {
            self.bools.remove(name.as_str());
            self.paths.remove(name.as_str());
        }

        self.bools.extend(other.bools);
        self.paths.extend(other.paths);
        self.lists.extend(other.lists);
    }

    pub(crate) fn bind_imported_local(
        &mut self,
        imported: &Self,
        imported_name: &str,
        bound_name: &str,
    ) -> bool {
        if let Some(list) = imported.lists.get(imported_name) {
            self.set_list(bound_name, list.clone());
            return true;
        }
        if let Some(value) = imported.bool_value(imported_name) {
            self.set_bool(bound_name, value);
            return true;
        }
        if let Some(path) = imported.paths.get(imported_name) {
            self.set_path(bound_name, path.clone());
            return true;
        }
        false
    }

    pub(crate) fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        let name = name.into();
        self.paths.remove(&name);
        self.lists.remove(&name);
        self.bools.insert(name, value);
    }

    pub(crate) fn remove_bool(&mut self, name: &str) {
        self.bools.remove(name);
    }

    pub(crate) fn bool_value(&self, name: &str) -> Option<bool> {
        self.bools.get(name).copied()
    }

    pub(crate) fn set_path(&mut self, name: impl Into<String>, value: Utf8PathBuf) {
        let name = name.into();
        self.bools.remove(&name);
        self.lists.remove(&name);
        self.paths.set(name, value);
    }

    pub(crate) fn remove_path(&mut self, name: &str) {
        self.paths.remove(name);
    }

    pub(crate) fn set_list(&mut self, name: impl Into<String>, value: LocalListBinding) {
        let name = name.into();
        self.bools.remove(&name);
        self.paths.remove(&name);
        self.lists.insert(name, value);
    }

    pub(crate) fn remove_list(&mut self, name: &str) {
        self.lists.remove(name);
    }

    pub(crate) fn list_binding(&self, name: &str) -> Option<&LocalListBinding> {
        self.lists.get(name)
    }

    pub(crate) fn clear_name(&mut self, name: &str) {
        self.bools.remove(name);
        self.paths.remove(name);
        self.lists.remove(name);
    }

    pub(crate) fn path_bindings(&self) -> &PythonPathBindings {
        &self.paths
    }
}
