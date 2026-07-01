use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use rustc_hash::FxHashMap;

use crate::python::PythonModuleName;
use crate::python::PythonPathBindings;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

/// How much to trust an extracted value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum StaticKnowledge {
    Known,
    Partial,
    Unknown,
}

impl StaticKnowledge {
    #[must_use]
    pub(crate) fn weakened_by(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Partial, _) | (_, Self::Partial) => Self::Partial,
            (Self::Known, Self::Known) => Self::Known,
        }
    }

    #[must_use]
    pub(crate) fn demoted_to_partial(self) -> Self {
        match self {
            Self::Known | Self::Partial => Self::Partial,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// Why an extracted value is [`StaticKnowledge::Partial`] or [`StaticKnowledge::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    SyntaxErrors,
    UnresolvedSettingsStarImport,
    UnsupportedAssignment,
    UnsupportedMutation,
    NonLiteralElement,
    NonLiteralKey,
    UnsupportedValue,
    InvalidModuleName,
    DictUnpack,
    AmbiguousCondition,
    UnsupportedPathExpression,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SyntaxErrors => "settings source contains syntax errors",
            Self::UnresolvedSettingsStarImport => "star import could not be resolved statically",
            Self::UnsupportedAssignment => "assignment is not statically supported",
            Self::UnsupportedMutation => "mutation is not statically supported",
            Self::NonLiteralElement => "element is not a literal",
            Self::NonLiteralKey => "dictionary key is not a literal",
            Self::UnsupportedValue => "value is not statically supported",
            Self::InvalidModuleName => "value is not a valid Python module name",
            Self::DictUnpack => "dictionary unpack is not statically supported",
            Self::AmbiguousCondition => "condition is not statically decidable",
            Self::UnsupportedPathExpression => "path expression is not statically supported",
        };
        f.write_str(message)
    }
}

/// A best-effort string list setting such as `INSTALLED_APPS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledAppsSetting {
    pub(crate) values: Vec<String>,
    pub(crate) knowledge: StaticKnowledge,
    pub(crate) reasons: Vec<Reason>,
}

impl Default for InstalledAppsSetting {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            knowledge: StaticKnowledge::Unknown,
            reasons: Vec::new(),
        }
    }
}

impl InstalledAppsSetting {
    pub(crate) fn known(values: Vec<String>) -> Self {
        Self {
            values,
            knowledge: StaticKnowledge::Known,
            reasons: Vec::new(),
        }
    }

    pub(crate) fn make_partial(&mut self, reason: Reason) {
        if self.knowledge != StaticKnowledge::Unknown || self.reasons.is_empty() {
            self.knowledge = StaticKnowledge::Partial;
        }
        self.reasons.push(reason);
    }

    pub(crate) fn make_unknown(&mut self, reason: Reason) {
        self.values.clear();
        self.knowledge = StaticKnowledge::Unknown;
        self.reasons.push(reason);
    }
}

/// The statically extracted subset of Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateSettings {
    pub(crate) backends: Vec<TemplateBackend>,
    pub(crate) knowledge: StaticKnowledge,
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self {
            backends: Vec::new(),
            knowledge: StaticKnowledge::Unknown,
        }
    }
}

impl TemplateSettings {
    pub(crate) fn known(backends: Vec<TemplateBackend>) -> Self {
        Self {
            backends,
            knowledge: StaticKnowledge::Known,
        }
    }

    pub(crate) fn partial() -> Self {
        Self {
            backends: Vec::new(),
            knowledge: StaticKnowledge::Partial,
        }
    }

    pub(crate) fn make_partial(&mut self) {
        if self.knowledge != StaticKnowledge::Unknown {
            self.knowledge = StaticKnowledge::Partial;
        }
    }

    pub(crate) fn make_unknown(&mut self) {
        self.backends.clear();
        self.knowledge = StaticKnowledge::Unknown;
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
    pub(crate) knowledge: StaticKnowledge,
    reasons: Vec<Reason>,
}

impl Default for TemplateBackend {
    fn default() -> Self {
        Self {
            backend: None,
            dirs: Vec::new(),
            app_dirs: None,
            libraries: Vec::new(),
            builtins: Vec::new(),
            knowledge: StaticKnowledge::Known,
            reasons: Vec::new(),
        }
    }
}

impl TemplateBackend {
    #[must_use]
    pub(crate) fn is_django_templates_backend(&self, backend_count: usize) -> bool {
        match self.backend.as_deref() {
            Some(DJANGO_TEMPLATES_BACKEND) => true,
            None if backend_count == 1 => true,
            _ => false,
        }
    }

    pub(crate) fn make_partial(&mut self, reason: Reason) {
        if self.knowledge != StaticKnowledge::Unknown || self.reasons.is_empty() {
            self.knowledge = StaticKnowledge::Partial;
        }
        self.reasons.push(reason);
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
