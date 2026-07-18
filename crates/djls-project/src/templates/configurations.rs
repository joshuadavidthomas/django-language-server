use camino::Utf8PathBuf;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::settings::EvaluatedPath;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::DjangoSettings;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::PartialTemplateBackend;
use crate::settings::types::PathListEvidence;
use crate::settings::types::SettingCase;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateListEvidence;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TemplateConfigurationId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TemplateBackendId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateEvidenceState {
    Complete,
    Open,
}

impl TemplateEvidenceState {
    pub(super) const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    const fn open_if(condition: bool) -> Self {
        if condition {
            Self::Open
        } else {
            Self::Complete
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TemplateDirectoryEvidence {
    Path(Utf8PathBuf),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateConfigurations {
    configurations: Vec<TemplateConfiguration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateConfiguration {
    id: TemplateConfigurationId,
    installed_apps: Vec<InstalledAppEvidence>,
    backends: Vec<TemplateBackendConfiguration>,
    slots: Vec<TemplateConfigurationSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateBackendConfiguration {
    id: TemplateBackendId,
    configuration: TemplateConfigurationId,
    data: TemplateBackendData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateBackendData {
    backend_name: Option<String>,
    backend_state: TemplateEvidenceState,
    directories: Vec<TemplateDirectoryEvidence>,
    app_directories: Option<bool>,
    app_directories_state: TemplateEvidenceState,
    libraries: Vec<(String, PythonModuleName)>,
    libraries_state: TemplateEvidenceState,
    builtins: Vec<PythonModuleName>,
    builtins_state: TemplateEvidenceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateConfigurationSlot {
    Backend(TemplateBackendId),
    Remainder,
}

impl TemplateConfigurations {
    fn from_settings(settings: &DjangoSettings) -> Self {
        let mut result = Self {
            configurations: Vec::new(),
        };
        for feasible in settings.feasible_configurations() {
            let installed_apps = match feasible.installed_apps {
                SettingCase::Known(value) => value
                    .apps
                    .iter()
                    .cloned()
                    .map(InstalledAppEvidence::Known)
                    .collect(),
                SettingCase::Dynamic(value) | SettingCase::Malformed(value) => {
                    value.apps.evidence.clone()
                }
                SettingCase::Unset => Vec::new(),
            };
            let slots = match feasible.templates {
                SettingCase::Known(value) => value
                    .backends
                    .iter()
                    .map(TemplateBackendData::from_complete)
                    .map(Some)
                    .collect(),
                SettingCase::Dynamic(value) | SettingCase::Malformed(value) => value
                    .templates
                    .evidence
                    .iter()
                    .map(|evidence| match evidence {
                        TemplateListEvidence::Backend(backend) => {
                            Some(TemplateBackendData::from_partial(backend))
                        }
                        TemplateListEvidence::Issue(_) => None,
                    })
                    .collect(),
                SettingCase::Unset => Vec::new(),
            };
            result.push_configuration(installed_apps, slots);
        }
        result
    }

    fn unavailable() -> Self {
        let mut result = Self {
            configurations: Vec::new(),
        };
        result.push_configuration(Vec::new(), vec![None]);
        result
    }

    fn push_configuration(
        &mut self,
        installed_apps: Vec<InstalledAppEvidence>,
        backend_slots: Vec<Option<TemplateBackendData>>,
    ) {
        let id = TemplateConfigurationId(
            u32::try_from(self.configurations.len())
                .expect("template configuration count should fit in u32"),
        );
        let next_backend = self
            .configurations
            .iter()
            .map(|configuration| configuration.backends.len())
            .sum::<usize>();
        let mut backends = Vec::new();
        let mut slots = Vec::new();
        for data in backend_slots {
            let Some(data) = data else {
                slots.push(TemplateConfigurationSlot::Remainder);
                continue;
            };
            let backend_id = TemplateBackendId(
                u32::try_from(next_backend + backends.len())
                    .expect("template backend count should fit in u32"),
            );
            backends.push(TemplateBackendConfiguration {
                id: backend_id,
                configuration: id,
                data,
            });
            slots.push(TemplateConfigurationSlot::Backend(backend_id));
        }
        self.configurations.push(TemplateConfiguration {
            id,
            installed_apps,
            backends,
            slots,
        });
    }

    pub(super) fn configurations(&self) -> &[TemplateConfiguration] {
        &self.configurations
    }

    pub(super) fn backend(&self, id: TemplateBackendId) -> Option<&TemplateBackendConfiguration> {
        self.configurations
            .iter()
            .flat_map(TemplateConfiguration::backends)
            .find(|backend| backend.id == id)
    }

    pub(super) fn for_testing(backend_counts: &[usize], has_remainder: bool) -> Self {
        let mut result = Self {
            configurations: Vec::new(),
        };
        for &backend_count in backend_counts {
            let mut slots = std::iter::repeat_with(TemplateBackendData::for_testing)
                .map(Some)
                .take(backend_count)
                .collect::<Vec<_>>();
            if has_remainder {
                slots.push(None);
            }
            result.push_configuration(Vec::new(), slots);
        }
        result
    }
}

impl TemplateConfiguration {
    pub(super) fn id(&self) -> TemplateConfigurationId {
        self.id
    }

    pub(super) fn installed_apps(&self) -> &[InstalledAppEvidence] {
        &self.installed_apps
    }

    pub(super) fn backends(&self) -> &[TemplateBackendConfiguration] {
        &self.backends
    }

    pub(super) fn slots(&self) -> &[TemplateConfigurationSlot] {
        &self.slots
    }
}

impl TemplateBackendData {
    fn empty() -> Self {
        Self {
            backend_name: None,
            backend_state: TemplateEvidenceState::Complete,
            directories: Vec::new(),
            app_directories: None,
            app_directories_state: TemplateEvidenceState::Complete,
            libraries: Vec::new(),
            libraries_state: TemplateEvidenceState::Complete,
            builtins: Vec::new(),
            builtins_state: TemplateEvidenceState::Complete,
        }
    }

    fn from_complete(backend: &TemplateBackend) -> Self {
        Self {
            backend_name: Some(backend.backend.value.clone()),
            directories: backend
                .dirs
                .iter()
                .map(|directory| match &directory.value {
                    EvaluatedPath::Resolved(path) => TemplateDirectoryEvidence::Path(path.clone()),
                })
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
            backend_state: TemplateEvidenceState::open_if(!backend.backend.issues.is_empty()),
            directories: backend
                .dirs
                .evidence
                .iter()
                .map(|evidence| match evidence {
                    PathListEvidence::Known(directory) => match &directory.value {
                        EvaluatedPath::Resolved(path) => {
                            TemplateDirectoryEvidence::Path(path.clone())
                        }
                    },
                    PathListEvidence::Issue(_) => TemplateDirectoryEvidence::Unknown,
                })
                .collect(),
            app_directories: backend.app_dirs.known.as_ref().map(|value| value.value),
            app_directories_state: TemplateEvidenceState::open_if(
                !backend.app_dirs.issues.is_empty(),
            ),
            libraries: backend
                .libraries
                .known
                .iter()
                .map(|(name, module)| (name.clone(), module.value.clone()))
                .collect(),
            libraries_state: TemplateEvidenceState::open_if(
                !backend.options.issues.is_empty() || !backend.libraries.issues.is_empty(),
            ),
            builtins: backend
                .builtins
                .known
                .iter()
                .map(|module| module.value.clone())
                .collect(),
            builtins_state: TemplateEvidenceState::open_if(
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

impl TemplateBackendConfiguration {
    pub(super) fn id(&self) -> TemplateBackendId {
        self.id
    }

    pub(super) fn configuration(&self) -> TemplateConfigurationId {
        self.configuration
    }

    pub(super) fn backend_name(&self) -> Option<&str> {
        self.data.backend_name.as_deref()
    }

    pub(super) fn backend_state(&self) -> TemplateEvidenceState {
        self.data.backend_state
    }

    pub(super) fn directories(&self) -> &[TemplateDirectoryEvidence] {
        &self.data.directories
    }

    pub(super) fn app_directories(&self) -> Option<bool> {
        self.data.app_directories
    }

    pub(super) fn app_directories_state(&self) -> TemplateEvidenceState {
        self.data.app_directories_state
    }

    pub(super) fn libraries(&self) -> &[(String, PythonModuleName)] {
        &self.data.libraries
    }

    pub(super) fn libraries_state(&self) -> TemplateEvidenceState {
        self.data.libraries_state
    }

    pub(super) fn builtins(&self) -> &[PythonModuleName] {
        &self.data.builtins
    }

    pub(super) fn builtins_state(&self) -> TemplateEvidenceState {
        self.data.builtins_state
    }
}

#[salsa::tracked(returns(ref))]
pub(super) fn template_configurations(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateConfigurations {
    if settings_module_file(db, project).is_none() {
        TemplateConfigurations::unavailable()
    } else {
        TemplateConfigurations::from_settings(django_settings(db, project))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_deterministic_and_parent_consistent() {
        let first = TemplateConfigurations::for_testing(&[2, 1], false);
        let second = TemplateConfigurations::for_testing(&[2, 1], false);

        assert_eq!(first, second);
        let configurations = first.configurations();
        assert_ne!(configurations[0].id(), configurations[1].id());
        let backend_ids = configurations
            .iter()
            .flat_map(TemplateConfiguration::backends)
            .map(TemplateBackendConfiguration::id)
            .collect::<Vec<_>>();
        assert_eq!(backend_ids.len(), 3);
        assert_ne!(backend_ids[0], backend_ids[1]);
        assert_ne!(backend_ids[1], backend_ids[2]);
        for configuration in configurations {
            assert!(
                configuration
                    .backends()
                    .iter()
                    .all(|backend| backend.configuration() == configuration.id())
            );
        }
    }

    #[test]
    fn testing_owner_exposes_ids_only_through_entries() {
        let configurations = TemplateConfigurations::for_testing(&[1], false);
        let configuration = &configurations.configurations()[0];
        let backend = &configuration.backends()[0];

        assert_eq!(backend.configuration(), configuration.id());
        assert_eq!(
            configurations
                .backend(backend.id())
                .map(TemplateBackendConfiguration::id),
            Some(backend.id())
        );
    }

    #[test]
    fn configuration_remainder_has_no_fake_backend() {
        let configurations = TemplateConfigurations::unavailable();
        let configuration = &configurations.configurations()[0];

        assert_eq!(
            configuration.slots(),
            [TemplateConfigurationSlot::Remainder]
        );
        assert!(configuration.backends().is_empty());
    }
}
