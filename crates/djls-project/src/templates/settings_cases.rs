use std::iter;

use camino::Utf8PathBuf;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::DjangoSettings;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::SettingCase;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateBackendEvidence;
use crate::settings::types::TemplateDirectoryEvidence;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TemplateSettingsCaseId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TemplateBackendId {
    settings_case: TemplateSettingsCaseId,
    slot: usize,
}

impl TemplateBackendId {
    pub(super) const fn settings_case(self) -> TemplateSettingsCaseId {
        self.settings_case
    }
}

/// Whether Template settings evidence is exhaustive or may omit additional values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TemplateEvidenceCompleteness {
    #[default]
    Complete,
    Open,
}

impl TemplateEvidenceCompleteness {
    pub(super) const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    pub(super) const fn open_if(condition: bool) -> Self {
        if condition {
            Self::Open
        } else {
            Self::Complete
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TemplateDirectorySlot {
    Path(Utf8PathBuf),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateSettingsCases {
    settings_cases: Vec<TemplateSettingsCase>,
}

/// One feasible branch-correlated combination of supported Django Template settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateSettingsCase {
    id: TemplateSettingsCaseId,
    installed_apps: Vec<InstalledAppEvidence>,
    slots: Vec<TemplateBackendSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateBackendCase {
    id: TemplateBackendId,
    data: TemplateBackendSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateBackendSettings {
    backend_name: Option<String>,
    backend_completeness: TemplateEvidenceCompleteness,
    directories: Vec<TemplateDirectorySlot>,
    app_directories: Option<bool>,
    app_directories_completeness: TemplateEvidenceCompleteness,
    libraries: Vec<(String, PythonModuleName)>,
    libraries_completeness: TemplateEvidenceCompleteness,
    builtins: Vec<PythonModuleName>,
    builtins_completeness: TemplateEvidenceCompleteness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TemplateBackendSlot {
    Backend(TemplateBackendCase),
    Remainder,
}

impl TemplateSettingsCases {
    fn from_settings(settings: &DjangoSettings) -> Self {
        let mut result = Self {
            settings_cases: Vec::new(),
        };
        for feasible in settings.feasible_cases() {
            let installed_apps = match feasible.installed_apps {
                SettingCase::Known(value) => value
                    .apps
                    .iter()
                    .cloned()
                    .map(InstalledAppEvidence::Known)
                    .collect(),
                SettingCase::Dynamic(value) | SettingCase::Malformed(value) => {
                    value.evidence.clone()
                }
                SettingCase::Unset => Vec::new(),
            };
            let slots = match feasible.templates {
                SettingCase::Known(value) => value
                    .backends
                    .iter()
                    .map(TemplateBackendSettings::from_complete)
                    .map(Some)
                    .collect(),
                SettingCase::Dynamic(value) | SettingCase::Malformed(value) => value
                    .evidence
                    .iter()
                    .map(|evidence| match evidence {
                        TemplateBackendEvidence::Backend(backend) => {
                            Some(TemplateBackendSettings::from_partial(backend))
                        }
                        TemplateBackendEvidence::Issue(_) => None,
                    })
                    .collect(),
                SettingCase::Unset => Vec::new(),
            };
            result.push_settings_case(installed_apps, slots);
        }
        result
    }

    fn unavailable() -> Self {
        let mut result = Self {
            settings_cases: Vec::new(),
        };
        result.push_settings_case(Vec::new(), vec![None]);
        result
    }

    fn push_settings_case(
        &mut self,
        installed_apps: Vec<InstalledAppEvidence>,
        backend_slots: Vec<Option<TemplateBackendSettings>>,
    ) {
        let id = TemplateSettingsCaseId(self.settings_cases.len());
        let slots = backend_slots
            .into_iter()
            .enumerate()
            .map(|(slot, data)| {
                data.map_or(TemplateBackendSlot::Remainder, |data| {
                    TemplateBackendSlot::Backend(TemplateBackendCase {
                        id: TemplateBackendId {
                            settings_case: id,
                            slot,
                        },
                        data,
                    })
                })
            })
            .collect();
        self.settings_cases.push(TemplateSettingsCase {
            id,
            installed_apps,
            slots,
        });
    }

    pub(super) fn settings_cases(&self) -> &[TemplateSettingsCase] {
        &self.settings_cases
    }

    pub(super) fn for_testing(backend_counts: &[usize], has_remainder: bool) -> Self {
        let mut result = Self {
            settings_cases: Vec::new(),
        };
        for &backend_count in backend_counts {
            let mut slots = iter::repeat_with(TemplateBackendSettings::for_testing)
                .map(Some)
                .take(backend_count)
                .collect::<Vec<_>>();
            if has_remainder {
                slots.push(None);
            }
            result.push_settings_case(Vec::new(), slots);
        }
        result
    }
}

impl TemplateSettingsCase {
    pub(super) fn id(&self) -> TemplateSettingsCaseId {
        self.id
    }

    pub(super) fn installed_apps(&self) -> &[InstalledAppEvidence] {
        &self.installed_apps
    }

    pub(super) fn backends(&self) -> impl Iterator<Item = &TemplateBackendCase> {
        self.slots.iter().filter_map(|slot| match slot {
            TemplateBackendSlot::Backend(backend) => Some(backend),
            TemplateBackendSlot::Remainder => None,
        })
    }

    pub(super) fn slots(&self) -> &[TemplateBackendSlot] {
        &self.slots
    }
}

impl TemplateBackendSettings {
    fn empty() -> Self {
        Self {
            backend_name: None,
            backend_completeness: TemplateEvidenceCompleteness::Complete,
            directories: Vec::new(),
            app_directories: None,
            app_directories_completeness: TemplateEvidenceCompleteness::Complete,
            libraries: Vec::new(),
            libraries_completeness: TemplateEvidenceCompleteness::Complete,
            builtins: Vec::new(),
            builtins_completeness: TemplateEvidenceCompleteness::Complete,
        }
    }

    fn from_complete(backend: &TemplateBackend) -> Self {
        Self {
            backend_name: Some(backend.backend.value.clone()),
            directories: backend
                .dirs
                .iter()
                .map(|directory| TemplateDirectorySlot::Path(directory.value.path().to_path_buf()))
                .collect(),
            app_directories: backend.app_dirs.as_ref().map(|value| value.value),
            libraries: backend
                .libraries
                .iter()
                .map(|(name, module)| (name.clone(), module.value.clone()))
                .collect(),
            builtins: backend
                .builtins
                .iter()
                .map(|module| module.value.clone())
                .collect(),
            ..Self::empty()
        }
    }

    fn from_partial(backend: &PartialTemplateBackend) -> Self {
        Self {
            backend_name: backend
                .backend
                .known
                .as_ref()
                .map(|name| name.value.clone()),
            backend_completeness: TemplateEvidenceCompleteness::open_if(
                !backend.backend.issues.is_empty(),
            ),
            directories: backend
                .dirs
                .evidence
                .iter()
                .map(|evidence| match evidence {
                    TemplateDirectoryEvidence::Known(directory) => {
                        TemplateDirectorySlot::Path(directory.value.path().to_path_buf())
                    }
                    TemplateDirectoryEvidence::Issue(_) => TemplateDirectorySlot::Unknown,
                })
                .collect(),
            app_directories: backend.app_dirs.known.as_ref().map(|value| value.value),
            app_directories_completeness: TemplateEvidenceCompleteness::open_if(
                !backend.app_dirs.issues.is_empty(),
            ),
            libraries: backend
                .libraries
                .known
                .iter()
                .map(|(name, module)| (name.clone(), module.value.clone()))
                .collect(),
            libraries_completeness: TemplateEvidenceCompleteness::open_if(
                !backend.options.issues.is_empty() || !backend.libraries.issues.is_empty(),
            ),
            builtins: backend
                .builtins
                .known
                .iter()
                .map(|module| module.value.clone())
                .collect(),
            builtins_completeness: TemplateEvidenceCompleteness::open_if(
                !backend.options.issues.is_empty() || !backend.builtins.issues.is_empty(),
            ),
        }
    }

    fn for_testing() -> Self {
        Self {
            backend_name: Some("django.template.backends.django.DjangoTemplates".to_string()),
            ..Self::empty()
        }
    }
}

impl TemplateBackendCase {
    pub(super) fn id(&self) -> TemplateBackendId {
        self.id
    }

    pub(super) fn backend_name(&self) -> Option<&str> {
        self.data.backend_name.as_deref()
    }

    pub(super) fn backend_completeness(&self) -> TemplateEvidenceCompleteness {
        self.data.backend_completeness
    }

    pub(super) fn directories(&self) -> &[TemplateDirectorySlot] {
        &self.data.directories
    }

    pub(super) fn app_directories(&self) -> Option<bool> {
        self.data.app_directories
    }

    pub(super) fn app_directories_completeness(&self) -> TemplateEvidenceCompleteness {
        self.data.app_directories_completeness
    }

    pub(super) fn libraries(&self) -> &[(String, PythonModuleName)] {
        &self.data.libraries
    }

    pub(super) fn libraries_completeness(&self) -> TemplateEvidenceCompleteness {
        self.data.libraries_completeness
    }

    pub(super) fn builtins(&self) -> &[PythonModuleName] {
        &self.data.builtins
    }

    pub(super) fn builtins_completeness(&self) -> TemplateEvidenceCompleteness {
        self.data.builtins_completeness
    }
}

#[salsa::tracked(returns(ref))]
pub(super) fn template_settings_cases(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateSettingsCases {
    if settings_module_file(db, project).is_none() {
        TemplateSettingsCases::unavailable()
    } else {
        TemplateSettingsCases::from_settings(django_settings(db, project))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_deterministic_and_parent_consistent() {
        let first = TemplateSettingsCases::for_testing(&[2, 1], false);
        let second = TemplateSettingsCases::for_testing(&[2, 1], false);

        assert_eq!(first, second);
        let settings_cases = first.settings_cases();
        assert_ne!(settings_cases[0].id(), settings_cases[1].id());
        let backend_ids = settings_cases
            .iter()
            .flat_map(TemplateSettingsCase::backends)
            .map(TemplateBackendCase::id)
            .collect::<Vec<_>>();
        assert_eq!(backend_ids.len(), 3);
        assert_ne!(backend_ids[0], backend_ids[1]);
        assert_ne!(backend_ids[1], backend_ids[2]);
        for settings_case in settings_cases {
            assert!(
                settings_case
                    .backends()
                    .all(|backend| backend.id().settings_case() == settings_case.id())
            );
        }
    }

    #[test]
    fn testing_owner_exposes_ids_only_through_entries() {
        let settings_cases = TemplateSettingsCases::for_testing(&[1], false);
        let settings_case = &settings_cases.settings_cases()[0];
        let backend = settings_case
            .backends()
            .next()
            .expect("testing settings case should contain one backend");

        assert_eq!(backend.id().settings_case(), settings_case.id());
    }

    #[test]
    fn settings_case_remainder_has_no_fake_backend() {
        let settings_cases = TemplateSettingsCases::unavailable();
        let settings_case = &settings_cases.settings_cases()[0];

        assert_eq!(settings_case.slots(), [TemplateBackendSlot::Remainder]);
        assert_eq!(settings_case.backends().count(), 0);
    }
}
