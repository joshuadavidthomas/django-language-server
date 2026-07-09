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
        for element in elements {
            let PythonValueKind::Dict(dict) = element.kind() else {
                complete = false;
                continue;
            };
            let backend = extract_template_backend(model, dict);
            if !backend.is_fully_extracted() || !element.is_complete() {
                complete = false;
            }
            backends.push(backend);
        }
    }

    TemplateSettings {
        backends,
        extraction: extraction_status(complete),
    }
}

fn extract_template_backend(model: &PythonSemanticModel, dict: &PythonDict) -> TemplateBackend {
    let mut backend = TemplateBackend::default();
    for entry in dict.entries() {
        let PythonValueKind::Str(key) = entry.key().kind() else {
            backend.mark_partial();
            continue;
        };
        match key.as_str() {
            "BACKEND" => match entry.value().kind() {
                PythonValueKind::Str(value) => backend.backend = Some(value.clone()),
                _ => backend.mark_partial(),
            },
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

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::path_to_file;
    use djls_testing::TestDatabase;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::ExtractionStatus;
    use crate::python::ImportLoadResult;
    use crate::python::PythonImportRequest;
    use crate::python::PythonSemanticModel;
    use crate::python::PythonSource;
    use crate::settings::types::EvaluatedPath;

    struct MapResolver<'db> {
        db: &'db TestDatabase,
        modules: FxHashMap<String, String>,
    }

    impl<'db> MapResolver<'db> {
        fn new(db: &'db TestDatabase) -> Self {
            Self {
                db,
                modules: FxHashMap::default(),
            }
        }

        fn with_module(mut self, name: &str, source: &str) -> Self {
            self.modules.insert(name.to_string(), source.to_string());
            self
        }

        fn resolve_mapped_import(&mut self, import: PythonImportRequest<'_>) -> ImportLoadResult {
            let Some(module) = import.module else {
                return ImportLoadResult::Unresolved;
            };
            let Some(source) = self.modules.get(module).cloned() else {
                return ImportLoadResult::Unresolved;
            };
            let path =
                Utf8PathBuf::from(format!("/project/settings/{}.py", module.replace('.', "/")));
            self.db.add_file(path.as_str(), &source);
            let Ok(file) = path_to_file(self.db, &path) else {
                return ImportLoadResult::Unresolved;
            };
            ImportLoadResult::Loaded(PythonSource::new(file, path, source))
        }
    }

    impl PythonImportLoader for MapResolver<'_> {
        fn load_star_import(&mut self, import: PythonImportRequest<'_>) -> ImportLoadResult {
            self.resolve_mapped_import(import)
        }

        fn load_named_import(&mut self, import: PythonImportRequest<'_>) -> ImportLoadResult {
            self.resolve_mapped_import(import)
        }
    }

    struct RefusingNamedResolver<'db> {
        inner: MapResolver<'db>,
    }

    impl<'db> RefusingNamedResolver<'db> {
        fn with_module(db: &'db TestDatabase, name: &str, source: &str) -> Self {
            Self {
                inner: MapResolver::new(db).with_module(name, source),
            }
        }
    }

    impl PythonImportLoader for RefusingNamedResolver<'_> {
        fn load_star_import(&mut self, import: PythonImportRequest<'_>) -> ImportLoadResult {
            self.inner.load_star_import(import)
        }

        fn load_named_import(&mut self, _import: PythonImportRequest<'_>) -> ImportLoadResult {
            ImportLoadResult::Unresolved
        }
    }

    struct PanickingResolver;

    impl PythonImportLoader for PanickingResolver {
        fn load_star_import(&mut self, _import: PythonImportRequest<'_>) -> ImportLoadResult {
            panic!("unreachable star import should not be resolved")
        }

        fn load_named_import(&mut self, _import: PythonImportRequest<'_>) -> ImportLoadResult {
            panic!("unreachable named import should not be resolved")
        }
    }

    fn extract(source: &str) -> DjangoSettings {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db);
        extract_with_resolver(&db, source, &mut resolver)
    }

    fn extract_with_resolver(
        db: &TestDatabase,
        source: &str,
        resolver: &mut dyn PythonImportLoader,
    ) -> DjangoSettings {
        let source = settings_source(db, source);
        extract_settings(&source, resolver)
    }

    fn settings_source(db: &TestDatabase, source: &str) -> PythonSource {
        let path = Utf8Path::new("/project/config/settings.py");
        db.add_file(path.as_str(), source);
        let file = path_to_file(db, path).expect("settings file should exist");
        PythonSource::new(file, path.to_path_buf(), source.to_string())
    }

    #[test]
    fn unreachable_import_is_not_a_semantic_dependency() {
        let db = TestDatabase::new();
        let mut resolver = PanickingResolver;
        let source = settings_source(
            &db,
            "if False:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read(), &[source.file()]);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn unreachable_elif_import_is_not_a_semantic_dependency() {
        let db = TestDatabase::new();
        let mut resolver = PanickingResolver;
        let source = settings_source(
            &db,
            "if FLAG:\n    INSTALLED_APPS = ['local']\nelif False:\n    from base import INSTALLED_APPS",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read(), &[source.file()]);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn ambiguous_branch_import_effects_are_semantic_dependencies() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "INSTALLED_APPS = [");
        let source = settings_source(
            &db,
            "if FLAG:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read()[0], source.file());
        assert_eq!(
            model.files_read()[1].path(&db).as_str(),
            "/project/settings/base.py"
        );
        assert_eq!(settings.parse_status, ParseStatus::Unparseable);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn loop_import_effects_are_dependencies_without_accepting_values() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "INSTALLED_APPS = [");
        let source = settings_source(&db, "for app in []:\n    from base import INSTALLED_APPS");

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read()[0], source.file());
        assert_eq!(
            model.files_read()[1].path(&db).as_str(),
            "/project/settings/base.py"
        );
        assert_eq!(settings.parse_status, ParseStatus::Unparseable);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn loop_star_import_degrades_existing_bindings_without_accepting_values() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "INSTALLED_APPS = ['base']");
        let source = settings_source(
            &db,
            "INSTALLED_APPS = ['local']\nfor app in PLUGINS:\n    from base import *",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read()[0], source.file());
        assert_eq!(
            model.files_read()[1].path(&db).as_str(),
            "/project/settings/base.py"
        );
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn loop_nested_unreachable_star_import_does_not_degrade_bindings() {
        let db = TestDatabase::new();
        let mut resolver = PanickingResolver;
        let source = settings_source(
            &db,
            "INSTALLED_APPS = ['local']\nfor app in PLUGINS:\n    if False:\n        from base import *",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read(), &[source.file()]);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn while_false_body_import_is_not_a_semantic_dependency() {
        let db = TestDatabase::new();
        let mut resolver = PanickingResolver;
        let source = settings_source(
            &db,
            "while False:\n    from base import INSTALLED_APPS\nelse:\n    INSTALLED_APPS = ['local']",
        );

        let model = PythonSemanticModel::analyze(&source, &mut resolver);
        let settings = settings_from_model(&model);

        assert_eq!(model.files_read(), &[source.file()]);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn literal_tuple_assignment_is_full() {
        let settings = extract("INSTALLED_APPS = ('django.contrib.auth', 'app')");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(
            settings.installed_apps.values,
            ["django.contrib.auth", "app"]
        );
    }

    #[test]
    fn annotated_assignment_is_full() {
        let settings = extract("INSTALLED_APPS: list[str] = ['app']");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["app"]);
    }

    #[test]
    fn plus_equals_extends_existing_values() {
        assert_eq!(
            extract("INSTALLED_APPS = ['base']\nINSTALLED_APPS += ['extra']")
                .installed_apps
                .values,
            ["base", "extra"]
        );
    }

    #[test]
    fn plus_chain_combines_literal_lists() {
        assert_eq!(
            extract("INSTALLED_APPS = ['a'] + ['b'] + ('c',)")
                .installed_apps
                .values,
            ["a", "b", "c"]
        );
    }

    #[test]
    fn plus_chain_splices_known_name() {
        assert_eq!(
            extract("INSTALLED_APPS = ['a']\nINSTALLED_APPS = INSTALLED_APPS + ['b']")
                .installed_apps
                .values,
            ["a", "b"]
        );
    }

    #[test]
    fn mutation_methods_update_values() {
        assert_eq!(
            extract(
                "INSTALLED_APPS = ['a', 'c']\n\
                 INSTALLED_APPS.append('d')\n\
                 INSTALLED_APPS.extend(['e'])\n\
                 INSTALLED_APPS.insert(1, 'b')\n\
                 INSTALLED_APPS.remove('c')",
            )
            .installed_apps
            .values,
            ["a", "b", "d", "e"]
        );
    }

    #[test]
    fn reassignment_replaces_prior_values() {
        assert_eq!(
            extract("INSTALLED_APPS = ['old']\nINSTALLED_APPS.append('ignored')\nINSTALLED_APPS = ['new']")
                .installed_apps
                .values,
            ["new"]
        );
    }

    #[test]
    fn unsupported_branch_mutation_remains_partial_when_other_branch_assigns() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'context_processors': []}}]\n\
             if FLAG:\n\
                 TEMPLATES[0]['OPTIONS']['context_processors'].append('django.template.context_processors.request')\n\
             else:\n\
                 TEMPLATES = [{'OPTIONS': {'context_processors': []}}]",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert!(settings.templates.backends.is_empty());
    }

    #[test]
    fn unsupported_branch_mutation_is_order_independent() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'context_processors': []}}]\n\
             if FLAG:\n\
                 TEMPLATES = [{'OPTIONS': {'context_processors': []}}]\n\
             else:\n\
                 TEMPLATES[0]['OPTIONS']['context_processors'].append('django.template.context_processors.request')",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert!(settings.templates.backends.is_empty());
    }

    #[test]
    fn non_literal_element_is_partial_and_skipped() {
        let settings = extract("INSTALLED_APPS = ['a', env('EXTRA'), 'b']");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
    }

    #[test]
    fn unsupported_assignment_is_unsupported() {
        let settings = extract("INSTALLED_APPS = get_apps()");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn decidable_if_true_picks_body() {
        assert_eq!(
            extract(
                "if True:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']"
            )
            .installed_apps
            .values,
            ["body"]
        );
    }

    #[test]
    fn decidable_if_false_picks_else() {
        assert_eq!(
            extract(
                "if False:\n    INSTALLED_APPS = ['body']\nelse:\n    INSTALLED_APPS = ['else']"
            )
            .installed_apps
            .values,
            ["else"]
        );
    }

    #[test]
    fn bool_name_condition_is_decidable() {
        assert_eq!(
            extract("DEBUG = True\nif DEBUG:\n    INSTALLED_APPS = ['debug']\nelse:\n    INSTALLED_APPS = ['prod']")
                .installed_apps
                .values,
            ["debug"]
        );
    }

    #[test]
    fn later_assignment_replaces_unsupported_touch_uncertainty() {
        let settings = extract(
            "INSTALLED_APPS = ['old']\nconfigure(INSTALLED_APPS)\nINSTALLED_APPS = ['new']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["new"]);
    }

    #[test]
    fn later_assignment_replaces_unresolved_star_import_uncertainty() {
        let settings = extract("from missing import *\nINSTALLED_APPS = ['local']");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn addition_from_partial_local_binding_stays_partial() {
        let settings =
            extract("if FLAG:\n    LOCAL_APPS = ['a']\nINSTALLED_APPS = LOCAL_APPS + ['b']");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
    }

    #[test]
    fn ambiguous_condition_walks_all_arms_and_marks_partial() {
        let settings = extract(
            "INSTALLED_APPS = ['base']\nif os.environ.get('X'):\n    INSTALLED_APPS.append('debug')\nelse:\n    INSTALLED_APPS.append('prod')",
        );
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["base", "debug", "prod"]);
    }

    #[test]
    fn same_value_in_ambiguous_branches_is_complete() {
        let settings = extract(
            "if FLAG:\n    INSTALLED_APPS = ['django.contrib.admin']\nelse:\n    INSTALLED_APPS = ['django.contrib.admin']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["django.contrib.admin"]);
    }

    #[test]
    fn same_relative_path_string_from_different_files_stays_partial() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("one.base", "STATIC_ROOT = 'static'")
            .with_module("two.base", "STATIC_ROOT = 'static'");
        let settings = extract_with_resolver(
            &db,
            "if FLAG:\n    from one.base import STATIC_ROOT\nelse:\n    from two.base import STATIC_ROOT",
            &mut resolver,
        );

        assert_eq!(
            settings.staticfiles.static_root.extraction,
            ExtractionStatus::Partial
        );
        let values: Vec<_> = settings
            .staticfiles
            .static_root
            .values
            .iter()
            .map(Originated::value)
            .cloned()
            .collect();
        assert_eq!(
            values,
            [
                EvaluatedPath::Resolved(Utf8PathBuf::from("/project/settings/one/static")),
                EvaluatedPath::Resolved(Utf8PathBuf::from("/project/settings/two/static")),
            ]
        );
    }

    #[test]
    fn for_loop_degrades_touched_settings_without_loop_candidates() {
        let settings = extract(
            "INSTALLED_APPS = ['base']\nfor app in EXTRA_APPS:\n    INSTALLED_APPS = ['loop']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["base"]);
    }

    #[test]
    fn for_loop_degrades_local_list_alias_without_dropping_prior_candidates() {
        let settings = extract(
            "LOCAL_APPS = ['base']\nfor app in EXTRA_APPS:\n    LOCAL_APPS = ['loop']\nINSTALLED_APPS = LOCAL_APPS",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["base"]);
    }

    #[test]
    fn while_loop_degrades_touched_settings_without_loop_candidates() {
        let settings = extract("while enabled():\n    STATIC_URL = '/loop-static/'");

        assert_eq!(
            settings.staticfiles.static_url.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.staticfiles.static_url.values.is_empty());
    }

    #[test]
    fn try_except_joins_alternative_setting_assignments() {
        let settings = extract(
            "try:\n    INSTALLED_APPS = ['try']\nexcept ImportError:\n    INSTALLED_APPS = ['except']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["try", "except"]);
    }

    #[test]
    fn try_except_handler_can_see_try_body_writes() {
        let settings = extract(
            "try:\n    INSTALLED_APPS = ['base']\n    risky()\nexcept Exception:\n    INSTALLED_APPS += ['fallback']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["base", "fallback"]);
    }

    #[test]
    fn try_except_preserves_pre_try_candidates_when_exception_may_happen_before_write() {
        let settings = extract(
            "INSTALLED_APPS = ['base']\ntry:\n    risky()\n    INSTALLED_APPS = ['try']\nexcept Exception:\n    pass",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["try", "base"]);
    }

    #[test]
    fn match_joins_case_assignments() {
        let settings = extract(
            "match ENV:\n    case 'prod':\n        INSTALLED_APPS = ['prod']\n    case _:\n        INSTALLED_APPS = ['dev']",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["prod", "dev"]);
    }

    #[test]
    fn match_or_pattern_with_wildcard_is_exhaustive() {
        let settings =
            extract("match ENV:\n    case 'prod' | _:\n        INSTALLED_APPS = ['app']");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["app"]);
    }

    #[test]
    fn match_capture_pattern_is_irrefutable() {
        let settings = extract("match ENV:\n    case captured:\n        INSTALLED_APPS = ['app']");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["app"]);
    }

    #[test]
    fn match_as_capture_pattern_is_irrefutable() {
        let settings =
            extract("match ENV:\n    case _ as captured:\n        INSTALLED_APPS = ['app']");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["app"]);
    }

    #[test]
    fn match_capture_pattern_shadows_existing_local_binding() {
        let settings = extract(
            "from pathlib import Path\nBASE_DIR = Path(__file__).resolve().parent.parent\nmatch ENV:\n    case BASE_DIR:\n        TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Unknown]
        );
    }

    #[test]
    fn duplicate_context_processor_keys_use_last_value() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'context_processors': ['project.context_processors.first'], 'context_processors': ['project.context_processors.second']}}]",
        );

        let processors = &settings.templates.backends[0].context_processors;
        assert_eq!(processors.len(), 1);
        assert_eq!(
            processors[0].value().as_str(),
            "project.context_processors.second"
        );
    }

    #[test]
    fn invalid_overwritten_context_processor_value_does_not_mark_partial() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'context_processors': [unknown], 'context_processors': ['project.context_processors.second']}}]",
        );

        let backend = &settings.templates.backends[0];
        assert!(backend.is_fully_extracted());
        assert_eq!(
            backend.context_processors[0].value().as_str(),
            "project.context_processors.second"
        );
    }

    #[test]
    fn duplicate_template_library_aliases_use_last_value() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'libraries': {'custom': 'project.templatetags.first', 'custom': 'project.templatetags.second'}}}]",
        );

        let libraries = &settings.templates.backends[0].libraries;
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].0, "custom");
        assert_eq!(libraries[0].1.as_str(), "project.templatetags.second");
    }

    #[test]
    fn invalid_overwritten_template_library_value_does_not_mark_partial() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'libraries': {'custom': unknown, 'custom': 'project.templatetags.second'}}}]",
        );

        let backend = &settings.templates.backends[0];
        assert!(backend.is_fully_extracted());
        assert_eq!(backend.libraries[0].0, "custom");
        assert_eq!(
            backend.libraries[0].1.as_str(),
            "project.templatetags.second"
        );
    }

    #[test]
    fn invalid_overwritten_template_backend_value_does_not_mark_partial() {
        let settings = extract("TEMPLATES = [{'DIRS': unknown, 'DIRS': ['templates']}]");

        let backend = &settings.templates.backends[0];
        assert!(backend.is_fully_extracted());
        assert_eq!(
            backend.dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/config/templates"
            ))]
        );
    }

    #[test]
    fn template_backend_spread_keeps_prior_known_facts_partial() {
        let settings = extract("TEMPLATES = [{'DIRS': ['templates'], **extra}]");

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/config/templates"
            ))]
        );
    }

    #[test]
    fn template_options_spread_keeps_prior_known_facts_partial() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'context_processors': ['project.context_processors.first'], **extra}}]",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        let processors = &settings.templates.backends[0].context_processors;
        assert_eq!(processors.len(), 1);
        assert_eq!(
            processors[0].value().as_str(),
            "project.context_processors.first"
        );
    }

    #[test]
    fn template_library_spread_keeps_prior_known_aliases_partial() {
        let settings = extract(
            "TEMPLATES = [{'OPTIONS': {'libraries': {'custom': 'project.templatetags.first', **extra}}}]",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        let libraries = &settings.templates.backends[0].libraries;
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].0, "custom");
        assert_eq!(libraries[0].1.as_str(), "project.templatetags.first");
    }

    #[test]
    fn unsupported_dict_expression_touching_known_setting_degrades_it() {
        let settings = extract("INSTALLED_APPS = ['base']\nconfigure({'apps': INSTALLED_APPS})");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn star_import_without_setting_does_not_overwrite_existing_fact() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent");
        let settings = extract_with_resolver(
            &db,
            "INSTALLED_APPS = ['local']\nfrom paths import *",
            &mut resolver,
        );
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn star_imported_scalar_keeps_imported_file_origin() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "STATIC_URL = '/static/'");
        let settings = extract_with_resolver(&db, "from base import *", &mut resolver);

        let origin = settings.staticfiles.static_url.values[0].origin();
        assert_eq!(origin.file.path(&db).as_str(), "/project/settings/base.py");
    }

    #[test]
    fn star_imported_bool_overwrites_stale_local_path_binding() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("flags", "BASE_DIR = False");
        let settings = extract_with_resolver(
            &db,
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             from flags import *\n\
             TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Unknown]
        );
    }

    #[test]
    fn star_imported_path_overwrites_stale_local_bool_binding() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent.parent");
        let settings = extract_with_resolver(
            &db,
            "BASE_DIR = False\n\
             from paths import *\n\
             TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn cyclic_star_import_does_not_recurse_forever() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("cycle", "from cycle import *\nINSTALLED_APPS = ['local']");
        let settings = extract_with_resolver(&db, "from cycle import *", &mut resolver);

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn unresolvable_star_import_is_partial() {
        let settings = extract("from missing import *");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    }

    #[test]
    fn aliased_non_star_imported_installed_apps_can_feed_assignment() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "INSTALLED_APPS = ['base']");
        let settings = extract_with_resolver(
            &db,
            "from base import INSTALLED_APPS as IA\nINSTALLED_APPS = IA + ['local']",
            &mut resolver,
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["base", "local"]);
    }

    #[test]
    fn refused_non_star_import_falls_back_to_definition_write() {
        let db = TestDatabase::new();
        let mut resolver =
            RefusingNamedResolver::with_module(&db, "base", "INSTALLED_APPS = ['base']");
        let settings = extract_with_resolver(&db, "from base import INSTALLED_APPS", &mut resolver);

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn imported_parse_error_marks_settings_unparseable() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module("base", "INSTALLED_APPS = [");
        let settings = extract_with_resolver(&db, "from base import INSTALLED_APPS", &mut resolver);

        assert_eq!(settings.parse_status, ParseStatus::Unparseable);
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
    }

    #[test]
    fn pathlib_named_import_does_not_affect_extraction_when_unresolved() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn template_dirs_string_list_resolves_relative_paths() {
        let settings = extract("TEMPLATES = [{'DIRS': ['templates']}]");

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/config/templates"
            ))]
        );
    }

    #[test]
    fn bare_template_dirs_string_is_partial() {
        let settings = extract("TEMPLATES = [{'DIRS': 'templates'}]");

        let backend = &settings.templates.backends[0];
        assert_eq!(backend.extraction, ExtractionStatus::Partial);
        assert!(backend.dirs.is_empty());
    }

    #[test]
    fn aliased_non_star_imported_path_can_feed_template_dirs() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("base", "BASE_DIR = Path(__file__).resolve().parent.parent");
        let settings = extract_with_resolver(
            &db,
            "from base import BASE_DIR as BD\nTEMPLATES = [{'DIRS': [BD / 'templates']}]",
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn non_star_import_chain_reuses_extracted_imported_bindings() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db)
            .with_module("common", "COMMON_APPS = ['common']")
            .with_module(
                "base",
                "from common import COMMON_APPS\nINSTALLED_APPS = COMMON_APPS + ['base']",
            );
        let settings = extract_with_resolver(&db, "from base import INSTALLED_APPS", &mut resolver);

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["common", "base"]);
    }

    #[test]
    fn cyclic_non_star_import_does_not_recurse_forever() {
        let db = TestDatabase::new();
        let mut resolver = MapResolver::new(&db).with_module(
            "cycle",
            "from cycle import INSTALLED_APPS\nINSTALLED_APPS = ['local']",
        );
        let settings =
            extract_with_resolver(&db, "from cycle import INSTALLED_APPS", &mut resolver);

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn tuple_literal_local_can_feed_installed_apps() {
        let settings = extract("LOCAL_APPS = ('a', 'b')\nINSTALLED_APPS = LOCAL_APPS");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
    }

    #[test]
    fn local_list_unknown_write_invalidates_stale_values() {
        let settings =
            extract("LOCAL_APPS = ['stale']\nLOCAL_APPS = get_apps()\nINSTALLED_APPS = LOCAL_APPS");

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn templates_dirs_append_mutates_existing_backend() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': []}]\n\
             TEMPLATES[0]['DIRS'].append(BASE_DIR / 'templates')",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn templates_dirs_plus_equals_extends_existing_backend() {
        let settings = extract(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': []}]\n\
             TEMPLATES[0]['DIRS'] += [BASE_DIR / 'templates']",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn non_literal_backend_is_partial() {
        let settings = extract("TEMPLATES = [{'BACKEND': backend_name}]");
        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(
            settings.templates.backends[0].extraction,
            ExtractionStatus::Partial
        );
    }

    #[test]
    fn template_backend_spread_then_reset_keeps_later_key_fact() {
        let settings = extract("TEMPLATES = [{'DIRS': ['a'], **extra, 'DIRS': ['b']}]");

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(settings.templates.backends[0].dirs.len(), 1);
        assert_eq!(
            settings.templates.backends[0].dirs[0],
            EvaluatedPath::Resolved(Utf8PathBuf::from("/project/config/b"))
        );
    }

    #[test]
    fn os_path_join_resolves_relative_to_base_dir() {
        let settings = extract(
            "from pathlib import Path\n\
             import os\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             TEMPLATES = [{'DIRS': [os.path.join(BASE_DIR, 'templates')]}]",
        );
        assert_eq!(
            settings.templates.backends[0].dirs,
            [EvaluatedPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn unknown_path_call_becomes_unknown_path_value() {
        let settings = extract("TEMPLATES = [{'DIRS': [dynamic_path()]}]");
        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert!(matches!(
            settings.templates.backends[0].dirs[0],
            EvaluatedPath::Unknown
        ));
    }

    #[test]
    fn ambiguous_assignment_preserves_pre_branch_possibility() {
        let settings =
            extract("INSTALLED_APPS = ['base']\nif FLAG:\n    INSTALLED_APPS = ['debug']");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["debug", "base"]);
    }

    #[test]
    fn ambiguous_branch_local_alias_preserves_possible_values() {
        let settings = extract(
            "if FLAG:\n    LOCAL_APPS = ['a']\nelse:\n    LOCAL_APPS = ['b']\nINSTALLED_APPS = LOCAL_APPS",
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
    }

    #[test]
    fn unsupported_assignment_then_valid_assignment_is_full() {
        let settings = extract("INSTALLED_APPS = get_apps()\nINSTALLED_APPS = ['blog']");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["blog"]);
    }

    #[test]
    fn unsupported_assignment_followed_by_soft_demotion_stays_unsupported() {
        let settings = extract("INSTALLED_APPS = get_apps()\nfrom missing import *");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn syntax_error_without_prior_settings_returns_partial_settings() {
        let settings = extract("INSTALLED_APPS = [");
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Partial
        );
        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
    }
}
