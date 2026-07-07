use rustc_hash::FxHashSet;

use crate::ExtractionStatus;
use crate::settings::extraction::KnownSetting;
use crate::settings::types::DjangoSettings;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalBindings;
use crate::settings::types::LocalListBinding;
use crate::settings::types::ScalarSetting;
use crate::settings::types::SettingValues;
use crate::settings::types::SettingsParseStatus;
use crate::settings::types::StaticFilesDirsSetting;
use crate::settings::types::StaticFilesSettings;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateDirPath;
use crate::settings::types::TemplateSettings;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SettingsBindings {
    pub(super) installed_apps: Option<InstalledAppsSetting>,
    pub(super) templates: Option<TemplateSettings>,
    pub(super) static_url: Option<ScalarSetting<String>>,
    pub(super) static_root: Option<ScalarSetting<TemplateDirPath>>,
    pub(super) staticfiles_dirs: Option<StaticFilesDirsSetting>,
    pub(super) locals: LocalBindings,
}

impl SettingsBindings {
    pub(super) fn to_settings(&self) -> DjangoSettings {
        DjangoSettings {
            parse_status: SettingsParseStatus::default(),
            installed_apps: self
                .installed_apps
                .clone()
                .unwrap_or_else(InstalledAppsSetting::partial),
            templates: self
                .templates
                .clone()
                .unwrap_or_else(TemplateSettings::partial),
            staticfiles: StaticFilesSettings {
                static_url: self
                    .static_url
                    .clone()
                    .unwrap_or_else(ScalarSetting::partial),
                static_root: self
                    .static_root
                    .clone()
                    .unwrap_or_else(ScalarSetting::partial),
                staticfiles_dirs: self
                    .staticfiles_dirs
                    .clone()
                    .unwrap_or_else(StaticFilesDirsSetting::partial),
            },
        }
    }

    pub(super) fn merge_star_import(&mut self, other: &Self) {
        if let Some(installed_apps) = &other.installed_apps {
            self.installed_apps = Some(installed_apps.clone());
        }
        if let Some(templates) = &other.templates {
            self.templates = Some(templates.clone());
        }
        if let Some(static_url) = &other.static_url {
            self.static_url = Some(static_url.clone());
        }
        if let Some(static_root) = &other.static_root {
            self.static_root = Some(static_root.clone());
        }
        if let Some(staticfiles_dirs) = &other.staticfiles_dirs {
            self.staticfiles_dirs = Some(staticfiles_dirs.clone());
        }
        self.locals.extend(other.locals.clone());
    }

    pub(super) fn mark_partial(&mut self, setting: KnownSetting) {
        match setting {
            KnownSetting::InstalledApps => match &mut self.installed_apps {
                Some(setting) => setting.mark_partial(),
                None => self.installed_apps = Some(InstalledAppsSetting::partial()),
            },
            KnownSetting::Templates => match &mut self.templates {
                Some(templates) => templates.mark_partial(),
                None => self.templates = Some(TemplateSettings::partial()),
            },
            KnownSetting::StaticUrl => match &mut self.static_url {
                Some(static_url) => static_url.mark_partial(),
                None => self.static_url = Some(ScalarSetting::partial()),
            },
            KnownSetting::StaticRoot => match &mut self.static_root {
                Some(static_root) => static_root.mark_partial(),
                None => self.static_root = Some(ScalarSetting::partial()),
            },
            KnownSetting::StaticFilesDirs => match &mut self.staticfiles_dirs {
                Some(staticfiles_dirs) => staticfiles_dirs.mark_partial(),
                None => self.staticfiles_dirs = Some(StaticFilesDirsSetting::partial()),
            },
        }
    }

    pub(super) fn mark_unsupported(&mut self, setting: KnownSetting) {
        match setting {
            KnownSetting::InstalledApps => {
                let setting = self
                    .installed_apps
                    .get_or_insert_with(InstalledAppsSetting::partial);
                setting.clear_to_partial();
            }
            KnownSetting::Templates => {
                let templates = self.templates.get_or_insert_with(TemplateSettings::partial);
                templates.clear_to_partial();
            }
            KnownSetting::StaticUrl => {
                let static_url = self.static_url.get_or_insert_with(ScalarSetting::partial);
                static_url.clear_to_partial();
            }
            KnownSetting::StaticRoot => {
                let static_root = self.static_root.get_or_insert_with(ScalarSetting::partial);
                static_root.clear_to_partial();
            }
            KnownSetting::StaticFilesDirs => {
                let staticfiles_dirs = self
                    .staticfiles_dirs
                    .get_or_insert_with(StaticFilesDirsSetting::partial);
                staticfiles_dirs.clear_to_partial();
            }
        }
    }

    pub(super) fn can_mutate_installed_apps(&self) -> bool {
        self.installed_apps.is_some()
    }

    pub(super) fn join_ambiguous(
        mut self,
        branch_bindings: &[SettingsBindings],
        writes: &TouchedBindings,
    ) -> SettingsBindings {
        for setting in writes.settings.iter().copied() {
            match setting {
                KnownSetting::InstalledApps => {
                    self.installed_apps = Some(join_setting_values(branch_bindings, |bindings| {
                        bindings.installed_apps.as_ref()
                    }));
                }
                KnownSetting::Templates => {
                    self.templates = Some(TemplateSettings {
                        backends: join_template_backends(branch_bindings),
                        extraction: ExtractionStatus::Partial,
                    });
                }
                KnownSetting::StaticUrl => {
                    self.static_url = Some(join_setting_values(branch_bindings, |bindings| {
                        bindings.static_url.as_ref()
                    }));
                }
                KnownSetting::StaticRoot => {
                    self.static_root = Some(join_setting_values(branch_bindings, |bindings| {
                        bindings.static_root.as_ref()
                    }));
                }
                KnownSetting::StaticFilesDirs => {
                    self.staticfiles_dirs =
                        Some(join_setting_values(branch_bindings, |bindings| {
                            bindings.staticfiles_dirs.as_ref()
                        }));
                }
            }
        }
        join_local_lists(&mut self, branch_bindings, &writes.locals);
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
    settings: FxHashSet<KnownSetting>,
    locals: FxHashSet<String>,
}

impl TouchedBindings {
    pub(super) fn merge(&mut self, other: &Self) {
        self.settings.extend(other.settings.iter().copied());
        self.locals.extend(other.locals.iter().cloned());
    }

    pub(super) fn record_setting(&mut self, setting: KnownSetting) {
        self.settings.insert(setting);
    }

    pub(super) fn record_local(&mut self, name: &str) {
        self.locals.insert(name.to_string());
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

fn join_setting_values<T: Clone + Eq>(
    branch_bindings: &[SettingsBindings],
    values: impl Fn(&SettingsBindings) -> Option<&SettingValues<T>>,
) -> SettingValues<T> {
    let mut joined = Vec::new();

    for bindings in branch_bindings {
        let Some(setting) = values(bindings) else {
            continue;
        };
        for value in &setting.values {
            if !joined.contains(value) {
                joined.push(value.clone());
            }
        }
    }

    SettingValues::with_extraction(joined, ExtractionStatus::Partial)
}

fn join_local_lists(
    base: &mut SettingsBindings,
    branch_bindings: &[SettingsBindings],
    local_writes: &FxHashSet<String>,
) {
    for name in local_writes {
        let Some(binding) = join_local_list(branch_bindings, name) else {
            base.locals.clear_name(name);
            continue;
        };
        base.locals.set_list(name, binding);
    }
}

fn join_local_list(branch_bindings: &[SettingsBindings], name: &str) -> Option<LocalListBinding> {
    let mut values = Vec::new();

    for bindings in branch_bindings {
        let binding = bindings.locals.list_binding(name)?;
        for value in &binding.values {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
    }

    Some(LocalListBinding::partial(values))
}
