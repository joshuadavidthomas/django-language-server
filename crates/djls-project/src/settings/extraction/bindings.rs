use crate::settings::types::DjangoSettings;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalBindings;
use crate::settings::types::SettingExtraction;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateSettings;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SettingsBindings {
    pub(super) installed_apps: Option<InstalledAppsSetting>,
    pub(super) templates: Option<TemplateSettings>,
    pub(super) locals: LocalBindings,
}

impl SettingsBindings {
    pub(super) fn to_settings(&self) -> DjangoSettings {
        DjangoSettings {
            installed_apps: self
                .installed_apps
                .clone()
                .unwrap_or_else(InstalledAppsSetting::unsupported),
            templates: self
                .templates
                .clone()
                .unwrap_or_else(TemplateSettings::unsupported),
        }
    }

    pub(super) fn merge_star_import(&mut self, other: &Self) {
        if let Some(installed_apps) = &other.installed_apps {
            self.installed_apps = Some(installed_apps.clone());
        }
        if let Some(templates) = &other.templates {
            self.templates = Some(templates.clone());
        }
        self.locals.extend(other.locals.clone());
    }

    pub(super) fn mark_installed_apps_partial(&mut self) {
        match &mut self.installed_apps {
            Some(setting) => setting.mark_partial(),
            None => self.installed_apps = Some(InstalledAppsSetting::partial()),
        }
    }

    pub(super) fn mark_installed_apps_unsupported(&mut self) {
        let setting = self
            .installed_apps
            .get_or_insert_with(InstalledAppsSetting::partial);
        setting.mark_unsupported();
    }

    pub(super) fn can_mutate_installed_apps(&self) -> bool {
        self.installed_apps
            .as_ref()
            .is_some_and(InstalledAppsSetting::is_usable_for_app_scan)
    }

    pub(super) fn mark_templates_partial(&mut self) {
        match &mut self.templates {
            Some(templates) => templates.mark_partial(),
            None => self.templates = Some(TemplateSettings::partial()),
        }
    }

    pub(super) fn mark_templates_unsupported(&mut self) {
        let templates = self.templates.get_or_insert_with(TemplateSettings::partial);
        templates.mark_unsupported();
    }

    pub(super) fn join_ambiguous(
        mut self,
        branch_bindings: &[SettingsBindings],
        writes: &TouchedBindings,
    ) -> SettingsBindings {
        if writes.installed_apps {
            self.installed_apps = Some(join_installed_apps(branch_bindings));
        }
        if writes.templates {
            self.templates = Some(TemplateSettings {
                backends: join_template_backends(branch_bindings),
                extraction: SettingExtraction::Partial,
            });
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExtractedListStatus {
    Complete,
    Incomplete,
}

impl ExtractedListStatus {
    pub(super) const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub(super) fn mark_incomplete(&mut self) {
        *self = Self::Incomplete;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExtractedList<T> {
    pub(super) values: Vec<T>,
    pub(super) status: ExtractedListStatus,
}

impl<T> ExtractedList<T> {
    pub(super) fn complete(values: Vec<T>) -> Self {
        Self {
            values,
            status: ExtractedListStatus::Complete,
        }
    }

    pub(super) fn incomplete(values: Vec<T>) -> Self {
        Self {
            values,
            status: ExtractedListStatus::Incomplete,
        }
    }
}

#[derive(Default, Clone)]
pub(super) struct TouchedBindings {
    pub(super) installed_apps: bool,
    pub(super) templates: bool,
}

impl TouchedBindings {
    pub(super) fn merge(&mut self, other: &Self) {
        self.installed_apps |= other.installed_apps;
        self.templates |= other.templates;
    }
}

fn join_installed_apps(branch_bindings: &[SettingsBindings]) -> InstalledAppsSetting {
    let mut values = Vec::new();

    for bindings in branch_bindings {
        let Some(setting) = &bindings.installed_apps else {
            continue;
        };
        for value in &setting.values {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
    }

    InstalledAppsSetting {
        values,
        extraction: SettingExtraction::Partial,
    }
}

fn join_template_backends(branch_bindings: &[SettingsBindings]) -> Vec<TemplateBackend> {
    let mut backends = Vec::new();
    for bindings in branch_bindings {
        let Some(templates) = &bindings.templates else {
            continue;
        };
        for backend in &templates.backends {
            if !backends.contains(backend) {
                backends.push(backend.clone());
            }
        }
    }
    backends
}
