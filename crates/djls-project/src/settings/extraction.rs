//! Bounded Django settings extraction.
//!
//! This extractor intentionally recognizes a small set of settings idioms. For
//! string-list settings such as `INSTALLED_APPS`, unsupported elements are
//! skipped and the setting becomes partially extracted instead of failing the
//! whole list. That differs from ty's `__all__` collector, but it matches
//! Django settings in practice: one environment-driven entry should not hide
//! the static entries around it.

mod bindings;
mod env;
mod installed_apps;
mod staticfiles;
pub(super) mod substrate;
mod templates;
mod traversal;

use std::sync::Arc;

use camino::Utf8PathBuf;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::settings::extraction::bindings::SettingsBindings;
use crate::settings::extraction::substrate::SettingsImportResolver;
use crate::settings::extraction::substrate::SettingsSource;
use crate::settings::extraction::traversal::SettingsBindingsCollector;
use crate::settings::types::DjangoSettings;
use crate::settings::types::SettingsParseStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssignmentCompleteness {
    Full,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KnownSetting {
    InstalledApps,
    Templates,
    StaticUrl,
    StaticRoot,
    StaticFilesDirs,
}

const KNOWN_SETTINGS: &[KnownSetting] = &[
    KnownSetting::InstalledApps,
    KnownSetting::Templates,
    KnownSetting::StaticUrl,
    KnownSetting::StaticRoot,
    KnownSetting::StaticFilesDirs,
];

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
    source: &SettingsSource,
    resolver: &mut dyn SettingsImportResolver,
) -> DjangoSettings {
    let mut extraction = SettingsExtraction::default();
    let (bindings, parse_status) = extraction.extract_module(source, resolver);
    let mut settings = bindings.to_settings();
    settings.parse_status = parse_status;
    settings
}

#[derive(Debug, Default)]
pub(super) struct SettingsExtraction {
    active: FxHashSet<Utf8PathBuf>,
    cache: FxHashMap<Utf8PathBuf, Arc<SettingsBindings>>,
}

impl SettingsExtraction {
    fn extract_module(
        &mut self,
        source: &SettingsSource,
        resolver: &mut dyn SettingsImportResolver,
    ) -> (Arc<SettingsBindings>, SettingsParseStatus) {
        let path = source.path().to_path_buf();
        if let Some(cached) = self.cache.get(&path) {
            return (Arc::clone(cached), SettingsParseStatus::Parsed);
        }
        if !self.active.insert(path.clone()) {
            return (
                Arc::new(SettingsBindings::default()),
                SettingsParseStatus::Parsed,
            );
        }

        let mut collector = SettingsBindingsCollector::new(source, resolver, self);

        let parse_status = if let Ok(parsed) = parse_module(source.source()) {
            let module = parsed.into_syntax();
            collector.walk_body(&module.body);
            SettingsParseStatus::Parsed
        } else {
            collector.mark_syntax_error();
            SettingsParseStatus::Unparseable
        };

        let bindings = Arc::new(collector.into_bindings());
        self.active.remove(&path);
        self.cache.insert(path, Arc::clone(&bindings));
        (bindings, parse_status)
    }

    fn extract_import_source(
        &mut self,
        imported: &SettingsSource,
        resolver: &mut dyn SettingsImportResolver,
    ) -> Option<Arc<SettingsBindings>> {
        if self.active.contains(imported.path()) {
            return None;
        }
        Some(self.extract_module(imported, resolver).0)
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
    use crate::settings::extraction::substrate::SettingsImport;
    use crate::settings::extraction::substrate::SettingsImportResolver;
    use crate::settings::extraction::substrate::SettingsSource;
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

        fn resolve_mapped_import(
            &mut self,
            import: &SettingsImport,
            _importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            let module = import.module.as_ref()?;
            let source = self.modules.get(module)?.clone();
            let path =
                Utf8PathBuf::from(format!("/project/settings/{}.py", module.replace('.', "/")));
            self.db.add_file(path.as_str(), &source);
            let file = path_to_file(self.db, &path).ok()?;
            Some(SettingsSource::new(file, path, source))
        }
    }

    impl SettingsImportResolver for MapResolver<'_> {
        fn resolve_star_import(
            &mut self,
            import: &SettingsImport,
            importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            self.resolve_mapped_import(import, importer)
        }

        fn resolve_named_import(
            &mut self,
            import: &SettingsImport,
            importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            self.resolve_mapped_import(import, importer)
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

    impl SettingsImportResolver for RefusingNamedResolver<'_> {
        fn resolve_star_import(
            &mut self,
            import: &SettingsImport,
            importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            self.inner.resolve_star_import(import, importer)
        }

        fn resolve_named_import(
            &mut self,
            _import: &SettingsImport,
            _importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            None
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
        resolver: &mut dyn SettingsImportResolver,
    ) -> DjangoSettings {
        let path = Utf8Path::new("/project/config/settings.py");
        db.add_file(path.as_str(), source);
        let file = path_to_file(db, path).expect("settings file should exist");
        let source = SettingsSource::new(file, path.to_path_buf(), source.to_string());
        extract_settings(&source, resolver)
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
