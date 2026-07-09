//! Bounded Django settings extraction.
//!
//! This extractor intentionally recognizes a small set of settings idioms. For
//! string-list settings such as `INSTALLED_APPS`, unsupported elements are
//! skipped and the setting becomes partially extracted instead of failing the
//! whole list. That differs from ty's `__all__` collector, but it matches
//! Django settings in practice: one environment-driven entry should not hide
//! the static entries around it.

use camino::Utf8Path;

use crate::ExtractionStatus;
use crate::python::ParseStatus;
use crate::python::PythonDict;
use crate::python::PythonImportLoader;
use crate::python::PythonMutationAccess;
use crate::python::PythonSemanticModel;
use crate::python::PythonSource;
use crate::python::PythonValue;
use crate::python::PythonValueKind;
use crate::settings::types::DjangoSettings;
use crate::settings::types::EvaluatedPath;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::Originated;
use crate::settings::types::ScalarSetting;
use crate::settings::types::SettingValues;
use crate::settings::types::StaticFilesDirsSetting;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateContextProcessorPath;
use crate::settings::types::TemplateSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KnownSetting {
    InstalledApps,
    Templates,
    StaticUrl,
    StaticRoot,
    StaticFilesDirs,
}

impl KnownSetting {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "INSTALLED_APPS" => Some(Self::InstalledApps),
            "TEMPLATES" => Some(Self::Templates),
            "STATIC_URL" => Some(Self::StaticUrl),
            "STATIC_ROOT" => Some(Self::StaticRoot),
            "STATICFILES_DIRS" => Some(Self::StaticFilesDirs),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::InstalledApps => "INSTALLED_APPS",
            Self::Templates => "TEMPLATES",
            Self::StaticUrl => "STATIC_URL",
            Self::StaticRoot => "STATIC_ROOT",
            Self::StaticFilesDirs => "STATICFILES_DIRS",
        }
    }
}

/// Extract Django settings from Python source.
#[must_use]
pub(crate) fn extract_settings(
    source: &PythonSource,
    resolver: &mut dyn PythonImportLoader,
) -> DjangoSettings {
    let model = PythonSemanticModel::analyze(source, resolver);
    settings_from_model(&model)
}

fn settings_from_model(model: &PythonSemanticModel) -> DjangoSettings {
    let mut settings = DjangoSettings {
        parse_status: model.parse_status(),
        installed_apps: extract_installed_apps(model),
        templates: extract_templates(model),
        staticfiles: crate::settings::types::StaticFilesSettings {
            static_url: extract_static_url(model),
            static_root: extract_static_root(model),
            staticfiles_dirs: extract_staticfiles_dirs(model),
        },
    };

    for mutation in model.mutations() {
        if let Some(setting) = KnownSetting::from_name(mutation.root())
            && !settings_supports_mutation(setting, mutation.access(), mutation.method())
        {
            clear_setting_to_partial(&mut settings, setting);
        }
    }

    if model.parse_status() == ParseStatus::Unparseable {
        settings.installed_apps.mark_partial();
        settings.templates.mark_partial();
        settings.staticfiles.static_url.mark_partial();
        settings.staticfiles.static_root.mark_partial();
        settings.staticfiles.staticfiles_dirs.mark_partial();
    }

    settings
}

fn settings_supports_mutation(
    setting: KnownSetting,
    access: &[PythonMutationAccess],
    method: &str,
) -> bool {
    match setting {
        KnownSetting::InstalledApps => {
            access.is_empty() && matches!(method, "append" | "extend" | "insert" | "remove")
        }
        KnownSetting::Templates => {
            matches!(method, "append" | "extend")
                && matches!(
                    access,
                    [PythonMutationAccess::Index(_), PythonMutationAccess::Key(key)] if key == "DIRS"
                )
        }
        KnownSetting::StaticUrl | KnownSetting::StaticRoot | KnownSetting::StaticFilesDirs => false,
    }
}

fn clear_setting_to_partial(settings: &mut DjangoSettings, setting: KnownSetting) {
    match setting {
        KnownSetting::InstalledApps => {
            settings.installed_apps.values.clear();
            settings.installed_apps.mark_partial();
        }
        KnownSetting::Templates => {
            settings.templates.backends.clear();
            settings.templates.mark_partial();
        }
        KnownSetting::StaticUrl => {
            settings.staticfiles.static_url.values.clear();
            settings.staticfiles.static_url.mark_partial();
        }
        KnownSetting::StaticRoot => {
            settings.staticfiles.static_root.values.clear();
            settings.staticfiles.static_root.mark_partial();
        }
        KnownSetting::StaticFilesDirs => {
            settings.staticfiles.staticfiles_dirs.values.clear();
            settings.staticfiles.staticfiles_dirs.mark_partial();
        }
    }
}

fn extract_installed_apps(model: &PythonSemanticModel) -> InstalledAppsSetting {
    let Some(binding) = model.binding(KnownSetting::InstalledApps.name()) else {
        return InstalledAppsSetting::partial();
    };

    let mut values = Vec::new();
    let mut complete = binding.is_complete();
    for bound in binding.values() {
        let PythonValueKind::List(elements) = bound.value().kind() else {
            complete = false;
            continue;
        };
        for element in elements {
            match element.kind() {
                PythonValueKind::Str(value) => push_unique(&mut values, value.clone()),
                _ => complete = false,
            }
            if !element.is_complete() {
                complete = false;
            }
        }
    }

    setting_values(values, complete)
}

fn extract_templates(model: &PythonSemanticModel) -> TemplateSettings {
    let Some(binding) = model.binding(KnownSetting::Templates.name()) else {
        return TemplateSettings::partial();
    };

    let mut backends = Vec::new();
    let mut complete = binding.is_complete();
    for bound in binding.values() {
        let PythonValueKind::List(elements) = bound.value().kind() else {
            complete = false;
            continue;
        };

        let existing_backend_count = backends.len();
        let mut matched_existing = vec![false; existing_backend_count];
        let sole_backend_in_alternative = elements.len() == 1;
        for element in elements {
            let PythonValueKind::Dict(dict) = element.kind() else {
                complete = false;
                continue;
            };
            let backend = extract_template_backend(model, dict, sole_backend_in_alternative);
            if !backend.is_fully_extracted() || !element.is_complete() {
                complete = false;
            }
            merge_alternative_template_backend(&mut backends, &mut matched_existing, backend);
        }
    }

    TemplateSettings {
        backends,
        extraction: extraction_status(complete),
    }
}

fn merge_alternative_template_backend(
    backends: &mut Vec<TemplateBackend>,
    matched_existing: &mut [bool],
    backend: TemplateBackend,
) {
    let equivalent = backends[..matched_existing.len()]
        .iter()
        .enumerate()
        .find(|(index, candidate)| {
            !matched_existing[*index] && template_backends_are_equivalent(candidate, &backend)
        })
        .map(|(index, _)| index);

    let Some(index) = equivalent else {
        backends.push(backend);
        return;
    };

    matched_existing[index] = true;
    for processor in backend.context_processors {
        if !backends[index].context_processors.contains(&processor) {
            backends[index].context_processors.push(processor);
        }
    }
}

fn template_backends_are_equivalent(left: &TemplateBackend, right: &TemplateBackend) -> bool {
    left.has_same_identity_as(right)
        && left.dirs == right.dirs
        && left.app_dirs == right.app_dirs
        && left.libraries == right.libraries
        && left.builtins == right.builtins
        && left.extraction == right.extraction
        && distinct_context_processor_values(&left.context_processors)
            == distinct_context_processor_values(&right.context_processors)
}

fn distinct_context_processor_values(
    processors: &[Originated<TemplateContextProcessorPath>],
) -> Vec<&TemplateContextProcessorPath> {
    let mut values = Vec::new();
    for processor in processors {
        if !values.contains(&processor.value()) {
            values.push(processor.value());
        }
    }
    values
}

fn extract_template_backend(
    model: &PythonSemanticModel,
    dict: &PythonDict,
    sole_backend_in_alternative: bool,
) -> TemplateBackend {
    let mut backend = if sole_backend_in_alternative {
        TemplateBackend::implicit_django()
    } else {
        TemplateBackend::default()
    };
    for entry in dict.entries() {
        let PythonValueKind::Str(key) = entry.key().kind() else {
            backend.mark_partial();
            continue;
        };
        match key.as_str() {
            "BACKEND" => {
                backend.mark_explicit_backend();
                match entry.value().kind() {
                    PythonValueKind::Str(value) => backend.backend = Some(value.clone()),
                    _ => backend.mark_partial(),
                }
            }
            "DIRS" => extract_template_dirs(model, entry.value(), &mut backend),
            "APP_DIRS" => match entry.value().kind() {
                PythonValueKind::Bool(value) => backend.app_dirs = Some(*value),
                _ => backend.mark_partial(),
            },
            "OPTIONS" => extract_template_options(entry.value(), &mut backend),
            _ => {}
        }
        if !entry.value().is_complete() {
            backend.mark_partial();
        }
    }
    backend
}

fn extract_template_dirs(
    model: &PythonSemanticModel,
    value: &PythonValue,
    backend: &mut TemplateBackend,
) {
    backend.dirs.clear();
    let PythonValueKind::List(elements) = value.kind() else {
        backend.mark_partial();
        return;
    };
    for element in elements {
        let path = evaluated_path(model, element);
        if path == EvaluatedPath::Unknown || !element.is_complete() {
            backend.mark_partial();
        }
        backend.dirs.push(path);
    }
}

fn extract_template_options(value: &PythonValue, backend: &mut TemplateBackend) {
    backend.libraries.clear();
    backend.builtins.clear();
    backend.context_processors.clear();
    let PythonValueKind::Dict(dict) = value.kind() else {
        backend.mark_partial();
        return;
    };

    for entry in dict.entries() {
        let PythonValueKind::Str(key) = entry.key().kind() else {
            backend.mark_partial();
            continue;
        };
        match key.as_str() {
            "libraries" => {
                let (libraries, complete) = extract_template_library_dict(entry.value());
                backend.libraries = libraries;
                if !complete {
                    backend.mark_partial();
                }
            }
            "builtins" => {
                let (builtins, complete) = extract_python_module_name_list(entry.value());
                backend.builtins = builtins;
                if !complete {
                    backend.mark_partial();
                }
            }
            "context_processors" => {
                let (context_processors, complete) =
                    extract_context_processor_path_list(entry.value());
                backend.context_processors = context_processors;
                if !complete {
                    backend.mark_partial();
                }
            }
            _ => {}
        }
        if !entry.value().is_complete() {
            backend.mark_partial();
        }
    }
}

fn extract_template_library_dict(
    value: &PythonValue,
) -> (Vec<(String, crate::python::PythonModuleName)>, bool) {
    let PythonValueKind::Dict(dict) = value.kind() else {
        return (Vec::new(), false);
    };

    let mut values = Vec::new();
    let mut complete = value.is_complete();
    for entry in dict.entries() {
        let PythonValueKind::Str(key) = entry.key().kind() else {
            complete = false;
            continue;
        };
        let PythonValueKind::Str(value) = entry.value().kind() else {
            complete = false;
            continue;
        };
        match crate::python::PythonModuleName::parse(value) {
            Ok(module_name) => values.push((key.clone(), module_name)),
            Err(_) => complete = false,
        }
    }
    (values, complete)
}

fn extract_python_module_name_list(
    value: &PythonValue,
) -> (Vec<crate::python::PythonModuleName>, bool) {
    let PythonValueKind::List(elements) = value.kind() else {
        return (Vec::new(), false);
    };

    let mut values = Vec::new();
    let mut complete = value.is_complete();
    for element in elements {
        let PythonValueKind::Str(value) = element.kind() else {
            complete = false;
            continue;
        };
        match crate::python::PythonModuleName::parse(value) {
            Ok(module_name) => values.push(module_name),
            Err(_) => complete = false,
        }
    }
    (values, complete)
}

fn extract_context_processor_path_list(
    value: &PythonValue,
) -> (Vec<Originated<TemplateContextProcessorPath>>, bool) {
    let PythonValueKind::List(elements) = value.kind() else {
        return (Vec::new(), false);
    };

    let mut values = Vec::new();
    let mut complete = value.is_complete();
    for element in elements {
        let PythonValueKind::Str(value) = element.kind() else {
            complete = false;
            continue;
        };
        match TemplateContextProcessorPath::parse(value) {
            Ok(path) => values.push(Originated::new(path, element.origin())),
            Err(_) => complete = false,
        }
    }
    (values, complete)
}

fn extract_static_url(model: &PythonSemanticModel) -> ScalarSetting<String> {
    let Some(binding) = model.binding(KnownSetting::StaticUrl.name()) else {
        return ScalarSetting::partial();
    };

    let mut values = Vec::new();
    let mut complete = binding.is_complete();
    for bound in binding.values() {
        match bound.value().kind() {
            PythonValueKind::Str(value) => {
                values.push(Originated::new(value.clone(), bound.value_origin()));
            }
            _ => complete = false,
        }
        if !bound.is_complete() {
            complete = false;
        }
    }
    setting_values(values, complete)
}

fn extract_static_root(model: &PythonSemanticModel) -> ScalarSetting<EvaluatedPath> {
    let Some(binding) = model.binding(KnownSetting::StaticRoot.name()) else {
        return ScalarSetting::partial();
    };

    let mut values = Vec::new();
    let mut complete = binding.is_complete();
    for bound in binding.values() {
        let path = evaluated_path(model, bound.value());
        if path == EvaluatedPath::Unknown || !bound.is_complete() {
            complete = false;
        }
        values.push(Originated::new(path, bound.value_origin()));
    }
    setting_values(values, complete)
}

fn extract_staticfiles_dirs(model: &PythonSemanticModel) -> StaticFilesDirsSetting {
    let Some(binding) = model.binding(KnownSetting::StaticFilesDirs.name()) else {
        return StaticFilesDirsSetting::partial();
    };

    let mut values = Vec::new();
    let mut complete = binding.is_complete();
    for bound in binding.values() {
        let PythonValueKind::List(elements) = bound.value().kind() else {
            complete = false;
            continue;
        };
        for element in elements {
            let path = evaluated_path(model, element);
            if path == EvaluatedPath::Unknown || !element.is_complete() {
                complete = false;
            }
            values.push(Originated::new(path, element.origin()));
        }
    }
    setting_values(values, complete)
}

fn evaluated_path(model: &PythonSemanticModel, value: &PythonValue) -> EvaluatedPath {
    match value.kind() {
        PythonValueKind::Path(path) => EvaluatedPath::Resolved(path.clone()),
        PythonValueKind::Str(path) => model
            .source_path(value.origin().file)
            .and_then(|source_path| resolve_string_path(source_path, path))
            .map_or(EvaluatedPath::Unknown, EvaluatedPath::Resolved),
        _ => EvaluatedPath::Unknown,
    }
}

fn resolve_string_path(source_path: &Utf8Path, path: &str) -> Option<camino::Utf8PathBuf> {
    let path = Utf8Path::new(path);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    source_path.parent().map(|parent| parent.join(path))
}

fn push_unique<T: Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn setting_values<T>(values: Vec<T>, complete: bool) -> SettingValues<T> {
    SettingValues::with_extraction(values, extraction_status(complete))
}

fn extraction_status(complete: bool) -> ExtractionStatus {
    if complete {
        ExtractionStatus::Complete
    } else {
        ExtractionStatus::Partial
    }
}
