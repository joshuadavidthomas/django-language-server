//! Django project model facts.
//!
//! These types are the confidence-aware boundary for project model assembly. They
//! intentionally do not feed validators yet; later milestones will populate them
//! from module resolution, settings extraction, app registry discovery, and
//! template assembly.

#![allow(
    dead_code,
    reason = "Milestone A1 defines fact types before later milestones populate them."
)]

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;
use crate::project::names::TemplateSymbolName;
use crate::project::symbols::TemplateSymbolKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Confidence {
    Known,
    Partial,
    Unknown,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "confidence", rename_all = "snake_case")]
pub(crate) enum Fact<T> {
    Known {
        value: T,
    },
    Partial {
        value: T,
        reasons: Vec<Reason>,
    },
    Unknown {
        reasons: Vec<Reason>,
    },
    Ambiguous {
        candidates: Vec<T>,
        reasons: Vec<Reason>,
    },
}

impl<T> Fact<T> {
    #[must_use]
    pub(crate) fn known(value: T) -> Self {
        Self::Known { value }
    }

    #[must_use]
    pub(crate) fn partial(value: T, reasons: Vec<Reason>) -> Self {
        Self::Partial { value, reasons }
    }

    #[must_use]
    pub(crate) fn unknown(reasons: Vec<Reason>) -> Self {
        Self::Unknown { reasons }
    }

    #[must_use]
    pub(crate) fn ambiguous(candidates: Vec<T>, reasons: Vec<Reason>) -> Self {
        Self::Ambiguous {
            candidates,
            reasons,
        }
    }

    #[must_use]
    pub(crate) fn confidence(&self) -> Confidence {
        match self {
            Self::Known { .. } => Confidence::Known,
            Self::Partial { .. } => Confidence::Partial,
            Self::Unknown { .. } => Confidence::Unknown,
            Self::Ambiguous { .. } => Confidence::Ambiguous,
        }
    }

    #[must_use]
    pub(crate) fn value(&self) -> Option<&T> {
        match self {
            Self::Known { value } | Self::Partial { value, .. } => Some(value),
            Self::Unknown { .. } | Self::Ambiguous { .. } => None,
        }
    }

    #[must_use]
    pub(crate) fn candidates(&self) -> &[T] {
        match self {
            Self::Ambiguous { candidates, .. } => candidates,
            Self::Known { .. } | Self::Partial { .. } | Self::Unknown { .. } => &[],
        }
    }

    #[must_use]
    pub(crate) fn reasons(&self) -> &[Reason] {
        match self {
            Self::Known { .. } => &[],
            Self::Partial { reasons, .. }
            | Self::Unknown { reasons }
            | Self::Ambiguous { reasons, .. } => reasons,
        }
    }

    #[must_use]
    pub(crate) fn with_reason(self, reason: Reason) -> Self {
        match self {
            Self::Known { value } => Self::Partial {
                value,
                reasons: vec![reason],
            },
            Self::Partial { value, mut reasons } => {
                reasons.push(reason);
                Self::Partial { value, reasons }
            }
            Self::Unknown { mut reasons } => {
                reasons.push(reason);
                Self::Unknown { reasons }
            }
            Self::Ambiguous {
                candidates,
                mut reasons,
            } => {
                reasons.push(reason);
                Self::Ambiguous {
                    candidates,
                    reasons,
                }
            }
        }
    }

    #[must_use]
    pub(crate) fn map<U>(self, mut map_value: impl FnMut(T) -> U) -> Fact<U> {
        match self {
            Self::Known { value } => Fact::Known {
                value: map_value(value),
            },
            Self::Partial { value, reasons } => Fact::Partial {
                value: map_value(value),
                reasons,
            },
            Self::Unknown { reasons } => Fact::Unknown { reasons },
            Self::Ambiguous {
                candidates,
                reasons,
            } => Fact::Ambiguous {
                candidates: candidates.into_iter().map(map_value).collect(),
                reasons,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Reason {
    pub(crate) field: Field,
    pub(crate) source: ReasonSource,
    pub(crate) message: String,
}

impl Reason {
    #[must_use]
    pub(crate) fn new(field: Field, source: ReasonSource, message: impl Into<String>) -> Self {
        Self {
            field,
            source,
            message: message.into(),
        }
    }

    #[must_use]
    pub(crate) fn file(
        field: Field,
        file: impl Into<Utf8PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(field, ReasonSource::File(file.into()), message)
    }

    #[must_use]
    pub(crate) fn path(
        field: Field,
        path: impl Into<Utf8PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(field, ReasonSource::Path(path.into()), message)
    }

    #[must_use]
    pub(crate) fn module(field: Field, module: PyModuleName, message: impl Into<String>) -> Self {
        Self::new(field, ReasonSource::Module(module), message)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum Field {
    #[serde(rename = "resolver.module_search_paths")]
    ResolverModuleSearchPaths,
    #[serde(rename = "resolver.module")]
    ResolverModule,
    #[serde(rename = "resolver.relative_import")]
    ResolverRelativeImport,
    #[serde(rename = "django.environment_discovery")]
    DjangoEnvironmentDiscovery,
    #[serde(rename = "settings.installed_apps")]
    SettingsInstalledApps,
    #[serde(rename = "settings.templates")]
    SettingsTemplates,
    #[serde(rename = "settings.template_dirs")]
    SettingsTemplateDirs,
    #[serde(rename = "settings.template_options")]
    SettingsTemplateOptions,
    #[serde(rename = "apps.installed")]
    AppsInstalled,
    #[serde(rename = "apps.config")]
    AppsConfig,
    #[serde(rename = "apps.path")]
    AppsPath,
    #[serde(rename = "templates.dirs")]
    TemplateDirs,
    #[serde(rename = "templates.libraries")]
    TemplateLibraries,
    #[serde(rename = "templates.builtins")]
    TemplateBuiltins,
    #[serde(rename = "templates.symbols")]
    TemplateSymbols,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ReasonSource {
    File(Utf8PathBuf),
    Path(Utf8PathBuf),
    Module(PyModuleName),
    DjangoEnvironmentRoot(Utf8PathBuf),
    Workspace(Utf8PathBuf),
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModuleSearchPathEntry {
    pub(crate) kind: ModuleSearchPathKind,
    pub(crate) path: Utf8PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModuleSearchPathKind {
    Workspace,
    AutoSrc,
    ExplicitPythonPath,
    SitePackages,
    PthFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModuleResolution {
    pub(crate) requested: PyModuleName,
    pub(crate) resolved: Fact<ResolvedModule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResolvedModule {
    pub(crate) module: PyModuleName,
    pub(crate) file: Utf8PathBuf,
    pub(crate) search_path: Utf8PathBuf,
    pub(crate) location: ModuleLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModuleLocation {
    Workspace,
    External,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SettingsFacts {
    pub(crate) file: Utf8PathBuf,
    pub(crate) files_read: Vec<Utf8PathBuf>,
    pub(crate) installed_apps: Fact<Vec<String>>,
    pub(crate) template_backends: Fact<Vec<TemplateBackendFact>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct InstalledAppFact {
    pub(crate) entry: String,
    pub(crate) module: Fact<PyModuleName>,
    pub(crate) path: Fact<Utf8PathBuf>,
    pub(crate) config: Option<AppConfigFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AppConfigFact {
    pub(crate) module: PyModuleName,
    pub(crate) file: Utf8PathBuf,
    pub(crate) class_name: String,
    pub(crate) name: Fact<PyModuleName>,
    pub(crate) label: Fact<String>,
    pub(crate) path: Fact<Utf8PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AppFact {
    pub(crate) entry: String,
    pub(crate) module: PyModuleName,
    pub(crate) path: Utf8PathBuf,
    pub(crate) config: Option<AppConfigFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TemplateBackendFact {
    pub(crate) backend: Option<String>,
    pub(crate) dirs: Fact<Vec<TemplateDirFact>>,
    pub(crate) app_dirs: Fact<bool>,
    pub(crate) option_libraries: Fact<Vec<TemplateLibraryFact>>,
    pub(crate) option_builtins: Fact<Vec<PyModuleName>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TemplateDirFact {
    pub(crate) path: Utf8PathBuf,
    pub(crate) source: TemplateDirSource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TemplateDirSource {
    SettingsDir,
    AppDir { app: PyModuleName },
    UserOverride,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TemplateLibraryFact {
    pub(crate) load_name: LibraryName,
    pub(crate) module: PyModuleName,
    pub(crate) source: TemplateLibrarySource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TemplateLibrarySource {
    AppTemplateTags { app: PyModuleName },
    SettingsLibraries,
    SettingsBuiltins,
    DjangoDefaultLibrary,
    DjangoDefaultBuiltin,
    Discovered,
    UserOverride,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TemplateSymbolFact {
    pub(crate) library: LibraryName,
    pub(crate) module: PyModuleName,
    pub(crate) kind: TemplateSymbolKind,
    pub(crate) name: TemplateSymbolName,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn library(name: &str) -> LibraryName {
        LibraryName::parse(name).unwrap()
    }

    fn symbol(name: &str) -> TemplateSymbolName {
        TemplateSymbolName::parse(name).unwrap()
    }

    fn unsupported_settings_reason() -> Reason {
        Reason::file(
            Field::SettingsInstalledApps,
            "project/settings.py",
            "unsupported list comprehension in INSTALLED_APPS",
        )
    }

    #[test]
    fn fact_confidence_tracks_known_partial_unknown_and_ambiguous() {
        let known = Fact::known(vec!["django.contrib.auth".to_string()]);
        assert_eq!(known.confidence(), Confidence::Known);
        assert_eq!(known.value().unwrap(), &["django.contrib.auth"]);
        assert!(known.reasons().is_empty());

        let reason = unsupported_settings_reason();
        let partial = Fact::partial(
            vec!["django.contrib.auth".to_string()],
            vec![reason.clone()],
        );
        assert_eq!(partial.confidence(), Confidence::Partial);
        assert_eq!(partial.value().unwrap(), &["django.contrib.auth"]);
        assert_eq!(partial.reasons(), std::slice::from_ref(&reason));

        let unknown = Fact::<Vec<String>>::unknown(vec![reason.clone()]);
        assert_eq!(unknown.confidence(), Confidence::Unknown);
        assert!(unknown.value().is_none());
        assert_eq!(unknown.reasons(), std::slice::from_ref(&reason));

        let ambiguous = Fact::ambiguous(
            vec![module("clientname.app2"), module("shared.clientname.app2")],
            vec![reason.clone()],
        );
        assert_eq!(ambiguous.confidence(), Confidence::Ambiguous);
        assert_eq!(ambiguous.candidates().len(), 2);
        assert_eq!(ambiguous.reasons(), &[reason]);
    }

    #[test]
    fn with_reason_preserves_value_while_downgrading_known_to_partial() {
        let reason = Reason::path(
            Field::TemplateDirs,
            "templates",
            "template directory expression depends on runtime state",
        );
        let fact = Fact::known(vec![Utf8PathBuf::from("templates")]).with_reason(reason.clone());

        assert_eq!(fact.confidence(), Confidence::Partial);
        assert_eq!(fact.value().unwrap(), &[Utf8PathBuf::from("templates")]);
        assert_eq!(fact.reasons(), &[reason]);
    }

    #[test]
    fn map_preserves_confidence_and_reasons() {
        let reason = Reason::module(
            Field::ResolverModule,
            module("clientname.app2"),
            "module exists in more than one module search path",
        );
        let fact = Fact::ambiguous(
            vec![module("clientname.app2"), module("shared.clientname.app2")],
            vec![reason.clone()],
        );
        let mapped = fact.map(|candidate| candidate.as_str().to_string());

        assert_eq!(mapped.confidence(), Confidence::Ambiguous);
        assert_eq!(
            mapped.candidates(),
            &[
                "clientname.app2".to_string(),
                "shared.clientname.app2".to_string()
            ]
        );
        assert_eq!(mapped.reasons(), &[reason]);
    }

    #[test]
    fn facts_are_cache_serializable() {
        let reason = Reason::file(
            Field::SettingsTemplates,
            "project/settings.py",
            "TEMPLATES includes an unsupported call expression",
        );
        let fact = Fact::<Vec<TemplateBackendFact>>::unknown(vec![reason]);

        let json = serde_json::to_string(&fact).unwrap();
        let roundtrip: Fact<Vec<TemplateBackendFact>> = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip, fact);
    }

    #[test]
    fn domain_facts_cover_template_libraries_and_symbols() {
        let library_fact = TemplateLibraryFact {
            load_name: library("app1_tags"),
            module: module("clientname.app1.templatetags.app1_tags"),
            source: TemplateLibrarySource::AppTemplateTags {
                app: module("clientname.app1"),
            },
        };
        let symbol_fact = TemplateSymbolFact {
            library: library("app1_tags"),
            module: module("clientname.app1.templatetags.app1_tags"),
            kind: TemplateSymbolKind::Tag,
            name: symbol("app1_name"),
        };

        let libraries = Fact::known(vec![library_fact.clone()]);
        let symbols = Fact::known(vec![symbol_fact.clone()]);

        assert_eq!(libraries.value().unwrap(), &[library_fact]);
        assert_eq!(symbols.value().unwrap(), &[symbol_fact]);
    }
}
