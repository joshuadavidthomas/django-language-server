use std::fmt;

use camino::Utf8PathBuf;
use rustc_hash::FxHashMap;

/// How much to trust an extracted value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knowledge {
    Known,
    Partial,
    Unknown,
}

/// Why an extracted value is [`Knowledge::Partial`] or [`Knowledge::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    SyntaxErrors,
    UnresolvedStarImport,
    UnsupportedAssignment,
    UnsupportedMutation,
    NonLiteralElement,
    NonLiteralKey,
    UnsupportedValue,
    DictUnpack,
    AmbiguousCondition,
    UnsupportedPathExpression,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SyntaxErrors => "settings source contains syntax errors",
            Self::UnresolvedStarImport => "star import could not be resolved statically",
            Self::UnsupportedAssignment => "assignment is not statically supported",
            Self::UnsupportedMutation => "mutation is not statically supported",
            Self::NonLiteralElement => "element is not a literal",
            Self::NonLiteralKey => "dictionary key is not a literal",
            Self::UnsupportedValue => "value is not statically supported",
            Self::DictUnpack => "dictionary unpack is not statically supported",
            Self::AmbiguousCondition => "condition is not statically decidable",
            Self::UnsupportedPathExpression => "path expression is not statically supported",
        };
        f.write_str(message)
    }
}

/// A best-effort string list setting such as `INSTALLED_APPS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringListSetting {
    pub values: Vec<String>,
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

impl Default for StringListSetting {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            knowledge: Knowledge::Unknown,
            reasons: Vec::new(),
        }
    }
}

impl StringListSetting {
    pub(crate) fn known(values: Vec<String>) -> Self {
        Self {
            values,
            knowledge: Knowledge::Known,
            reasons: Vec::new(),
        }
    }

    pub(crate) fn make_partial(&mut self, reason: Reason) {
        if self.knowledge != Knowledge::Unknown || self.reasons.is_empty() {
            self.knowledge = Knowledge::Partial;
        }
        self.reasons.push(reason);
    }

    pub(crate) fn make_unknown(&mut self, reason: Reason) {
        self.values.clear();
        self.knowledge = Knowledge::Unknown;
        self.reasons.push(reason);
    }
}

/// One entry in Django's `TEMPLATES` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateBackend {
    pub backend: Option<String>,
    pub dirs: Vec<PathValue>,
    pub app_dirs: Option<bool>,
    pub libraries: Vec<(String, String)>,
    pub builtins: Vec<String>,
    pub knowledge: Knowledge,
    pub reasons: Vec<Reason>,
}

impl Default for TemplateBackend {
    fn default() -> Self {
        Self {
            backend: None,
            dirs: Vec::new(),
            app_dirs: None,
            libraries: Vec::new(),
            builtins: Vec::new(),
            knowledge: Knowledge::Known,
            reasons: Vec::new(),
        }
    }
}

impl TemplateBackend {
    pub(crate) fn make_partial(&mut self, reason: Reason) {
        if self.knowledge != Knowledge::Unknown || self.reasons.is_empty() {
            self.knowledge = Knowledge::Partial;
        }
        self.reasons.push(reason);
    }
}

/// The statically extracted subset of a Django settings module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DjangoSettings {
    pub installed_apps: StringListSetting,
    pub template_backends: Vec<TemplateBackend>,
    pub templates_knowledge: Knowledge,
}

/// A path expression evaluated against the settings file's own location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathValue {
    Resolved(Utf8PathBuf),
    Unknown(Reason),
}

/// `from X import *`; the caller resolves and recurses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StarImport {
    pub level: u32,
    pub module: Option<String>,
}

/// Caller-supplied recursion for star imports.
pub trait StarImportResolver {
    /// Return the already-extracted environment for the referenced module, or
    /// `None` if it cannot be resolved.
    fn resolve(&mut self, import: &StarImport) -> Option<SettingsEnv>;
}

/// The extractor's working state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsEnv {
    pub(crate) installed_apps: StringListSetting,
    pub(crate) installed_apps_bound: bool,
    pub(crate) template_backends: Vec<TemplateBackend>,
    pub(crate) templates_knowledge: Knowledge,
    pub(crate) templates_bound: bool,
    pub(crate) bools: FxHashMap<String, bool>,
    pub(crate) paths: FxHashMap<String, PathValue>,
}

impl Default for SettingsEnv {
    fn default() -> Self {
        Self {
            installed_apps: StringListSetting::default(),
            installed_apps_bound: false,
            template_backends: Vec::new(),
            templates_knowledge: Knowledge::Unknown,
            templates_bound: false,
            bools: FxHashMap::default(),
            paths: FxHashMap::default(),
        }
    }
}

impl SettingsEnv {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn into_settings(self) -> DjangoSettings {
        DjangoSettings {
            installed_apps: self.installed_apps,
            template_backends: self.template_backends,
            templates_knowledge: self.templates_knowledge,
        }
    }

    pub(crate) fn merge_star_import(&mut self, other: Self) {
        if other.installed_apps_bound {
            self.installed_apps = other.installed_apps;
            self.installed_apps_bound = true;
        }
        if other.templates_bound {
            self.template_backends = other.template_backends;
            self.templates_knowledge = other.templates_knowledge;
            self.templates_bound = true;
        }
        self.bools.extend(other.bools);
        self.paths.extend(other.paths);
    }

    pub(crate) fn assign_installed_apps(&mut self, values: Vec<String>) {
        self.installed_apps = StringListSetting::known(values);
        self.installed_apps_bound = true;
    }

    pub(crate) fn installed_apps_mut(&mut self) -> &mut StringListSetting {
        &mut self.installed_apps
    }

    pub(crate) fn bind_installed_apps(&mut self) {
        self.installed_apps_bound = true;
    }

    pub(crate) fn make_installed_apps_unknown(&mut self, reason: Reason) {
        self.installed_apps.make_unknown(reason);
        self.installed_apps_bound = true;
    }

    pub(crate) fn assign_templates(&mut self, backends: Vec<TemplateBackend>) {
        self.template_backends = backends;
        self.templates_knowledge = Knowledge::Known;
        self.templates_bound = true;
    }

    pub(crate) fn make_templates_partial(&mut self) {
        if self.templates_knowledge != Knowledge::Unknown || !self.templates_bound {
            self.templates_knowledge = Knowledge::Partial;
        }
    }

    pub(crate) fn make_templates_unknown(&mut self) {
        self.template_backends.clear();
        self.templates_knowledge = Knowledge::Unknown;
        self.templates_bound = true;
    }

    pub(crate) fn bind_templates(&mut self) {
        self.templates_bound = true;
    }

    pub(crate) fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        self.bools.insert(name.into(), value);
    }

    pub(crate) fn remove_bool(&mut self, name: &str) {
        self.bools.remove(name);
    }

    pub(crate) fn bool_value(&self, name: &str) -> Option<bool> {
        self.bools.get(name).copied()
    }

    pub(crate) fn set_path(&mut self, name: impl Into<String>, value: PathValue) {
        self.paths.insert(name.into(), value);
    }

    pub(crate) fn remove_path(&mut self, name: &str) {
        self.paths.remove(name);
    }

    pub(crate) fn path_value(&self, name: &str) -> Option<&PathValue> {
        self.paths.get(name)
    }
}
