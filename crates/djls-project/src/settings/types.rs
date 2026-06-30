use camino::Utf8Path;
use camino::Utf8PathBuf;
use rustc_hash::FxHashMap;

use crate::python::PythonModuleName;
use crate::python::PythonPathBindings;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

/// How completely a watched settings value was extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingExtraction {
    Full,
    Partial,
    Unsupported,
}

/// A best-effort string list setting such as `INSTALLED_APPS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledAppsSetting {
    pub(crate) values: Vec<String>,
    pub(crate) extraction: SettingExtraction,
}

impl Default for InstalledAppsSetting {
    fn default() -> Self {
        Self::unsupported()
    }
}

impl InstalledAppsSetting {
    pub(crate) fn full(values: Vec<String>) -> Self {
        Self {
            values,
            extraction: SettingExtraction::Full,
        }
    }

    pub(crate) fn partial() -> Self {
        Self {
            values: Vec::new(),
            extraction: SettingExtraction::Partial,
        }
    }

    pub(crate) fn unsupported() -> Self {
        Self {
            values: Vec::new(),
            extraction: SettingExtraction::Unsupported,
        }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        matches!(self.extraction, SettingExtraction::Full)
    }

    #[must_use]
    pub(crate) fn is_usable_for_app_scan(&self) -> bool {
        !matches!(self.extraction, SettingExtraction::Unsupported)
    }

    pub(crate) fn mark_partial(&mut self) {
        if !matches!(self.extraction, SettingExtraction::Unsupported) {
            self.extraction = SettingExtraction::Partial;
        }
    }

    pub(crate) fn mark_unsupported(&mut self) {
        self.values.clear();
        self.extraction = SettingExtraction::Unsupported;
    }
}

/// The statically extracted subset of Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateSettings {
    pub(crate) backends: Vec<TemplateBackend>,
    pub(crate) extraction: SettingExtraction,
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self::unsupported()
    }
}

impl TemplateSettings {
    pub(crate) fn full(backends: Vec<TemplateBackend>) -> Self {
        Self {
            backends,
            extraction: SettingExtraction::Full,
        }
    }

    pub(crate) fn partial() -> Self {
        Self {
            backends: Vec::new(),
            extraction: SettingExtraction::Partial,
        }
    }

    pub(crate) fn unsupported() -> Self {
        Self {
            backends: Vec::new(),
            extraction: SettingExtraction::Unsupported,
        }
    }

    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        matches!(self.extraction, SettingExtraction::Full)
    }

    pub(crate) fn mark_partial(&mut self) {
        if !matches!(self.extraction, SettingExtraction::Unsupported) {
            self.extraction = SettingExtraction::Partial;
        }
    }

    pub(crate) fn mark_unsupported(&mut self) {
        self.backends.clear();
        self.extraction = SettingExtraction::Unsupported;
    }
}

/// One entry in Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateBackend {
    pub(crate) backend: Option<String>,
    pub(crate) dirs: Vec<TemplateDirPath>,
    pub(crate) app_dirs: Option<bool>,
    pub(crate) libraries: Vec<(String, PythonModuleName)>,
    pub(crate) builtins: Vec<PythonModuleName>,
    pub(crate) extraction: SettingExtraction,
}

impl Default for TemplateBackend {
    fn default() -> Self {
        Self {
            backend: None,
            dirs: Vec::new(),
            app_dirs: None,
            libraries: Vec::new(),
            builtins: Vec::new(),
            extraction: SettingExtraction::Full,
        }
    }
}

impl TemplateBackend {
    #[must_use]
    pub(crate) fn is_fully_extracted(&self) -> bool {
        matches!(self.extraction, SettingExtraction::Full)
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
        if !matches!(self.extraction, SettingExtraction::Unsupported) {
            self.extraction = SettingExtraction::Partial;
        }
    }
}

/// The statically extracted subset of a Django settings module.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DjangoSettings {
    pub(crate) installed_apps: InstalledAppsSetting,
    pub(crate) templates: TemplateSettings,
}

/// A path expression evaluated against the settings file's own location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateDirPath {
    Resolved(Utf8PathBuf),
    Unknown,
}

/// `from X import *`; the caller resolves the imported source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SettingsStarImport {
    pub(crate) level: u32,
    pub(crate) module: Option<String>,
}

/// Resolved source for a `from X import *` import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsSource {
    pub(crate) source: String,
    pub(crate) path: Utf8PathBuf,
}

/// Caller-supplied source lookup for star imports.
pub(crate) trait SettingsSourceResolver {
    /// Return the source for the referenced module, or `None` if it cannot be resolved.
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LocalBindings {
    bools: FxHashMap<String, bool>,
    paths: PythonPathBindings,
}

impl LocalBindings {
    pub(crate) fn extend(&mut self, other: Self) {
        for name in other.bools.keys() {
            self.paths.remove(name.as_str());
        }
        for name in other.paths.names() {
            self.bools.remove(name);
        }

        self.bools.extend(other.bools);
        self.paths.extend(other.paths);
    }

    pub(crate) fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        let name = name.into();
        self.paths.remove(&name);
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
        self.paths.set(name, value);
    }

    pub(crate) fn remove_path(&mut self, name: &str) {
        self.paths.remove(name);
    }

    pub(crate) fn path_bindings(&self) -> &PythonPathBindings {
        &self.paths
    }
}
