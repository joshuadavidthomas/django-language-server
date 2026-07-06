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
mod templates;
mod traversal;

use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::settings::extraction::bindings::SettingsBindings;
use crate::settings::extraction::traversal::SettingsBindingsCollector;
use crate::settings::types::DjangoSettings;
use crate::settings::types::SettingsParseStatus;
use crate::settings::types::SettingsSource;
use crate::settings::types::SettingsSourceResolver;

const INSTALLED_APPS: &str = "INSTALLED_APPS";
const TEMPLATES: &str = "TEMPLATES";

/// Extract Django settings from Python source.
#[must_use]
pub(crate) fn extract_settings(
    source: &str,
    module_path: &Utf8Path,
    resolver: &mut dyn SettingsSourceResolver,
) -> DjangoSettings {
    let mut extraction = SettingsExtraction::default();
    let (bindings, parse_status) = extraction.extract_module(source, module_path, resolver);
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
        source: &str,
        module_path: &Utf8Path,
        resolver: &mut dyn SettingsSourceResolver,
    ) -> (Arc<SettingsBindings>, SettingsParseStatus) {
        let path = module_path.to_path_buf();
        if let Some(cached) = self.cache.get(&path) {
            return (Arc::clone(cached), SettingsParseStatus::Parsed);
        }
        if !self.active.insert(path.clone()) {
            return (
                Arc::new(SettingsBindings::default()),
                SettingsParseStatus::Parsed,
            );
        }

        let mut collector = SettingsBindingsCollector::new(module_path, resolver, self);

        let parse_status = if let Ok(parsed) = parse_module(source) {
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
        resolver: &mut dyn SettingsSourceResolver,
    ) -> Option<Arc<SettingsBindings>> {
        if self.active.contains(&imported.path) {
            return None;
        }
        Some(
            self.extract_module(&imported.source, &imported.path, resolver)
                .0,
        )
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::ExtractionStatus;
    use crate::settings::types::SettingsImport;
    use crate::settings::types::TemplateDirPath;

    #[derive(Default)]
    struct MapResolver {
        modules: FxHashMap<String, String>,
    }

    impl MapResolver {
        fn with_module(mut self, name: &str, source: &str) -> Self {
            self.modules.insert(name.to_string(), source.to_string());
            self
        }
    }

    impl SettingsSourceResolver for MapResolver {
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

    impl MapResolver {
        fn resolve_mapped_import(
            &mut self,
            import: &SettingsImport,
            _importer: &Utf8Path,
        ) -> Option<SettingsSource> {
            let module = import.module.as_ref()?;
            let source = self.modules.get(module)?.clone();
            Some(SettingsSource {
                source,
                path: Utf8PathBuf::from(format!(
                    "/project/settings/{}.py",
                    module.replace('.', "/")
                )),
            })
        }
    }

    struct RefusingNamedResolver {
        inner: MapResolver,
    }

    impl RefusingNamedResolver {
        fn with_module(name: &str, source: &str) -> Self {
            Self {
                inner: MapResolver::default().with_module(name, source),
            }
        }
    }

    impl SettingsSourceResolver for RefusingNamedResolver {
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
        extract_settings(
            source,
            Utf8Path::new("/project/config/settings.py"),
            &mut MapResolver::default(),
        )
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
    fn plus_chain_splices_watched_name() {
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
    fn star_import_without_setting_does_not_overwrite_existing_fact() {
        let mut resolver = MapResolver::default()
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent");
        let settings = extract_settings(
            "INSTALLED_APPS = ['local']\nfrom paths import *",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );
        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn star_imported_bool_overwrites_stale_local_path_binding() {
        let mut resolver = MapResolver::default().with_module("flags", "BASE_DIR = False");
        let settings = extract_settings(
            "from pathlib import Path\n\
             BASE_DIR = Path(__file__).resolve().parent.parent\n\
             from flags import *\n\
             TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Partial);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Unknown]
        );
    }

    #[test]
    fn star_imported_path_overwrites_stale_local_bool_binding() {
        let mut resolver = MapResolver::default()
            .with_module("paths", "BASE_DIR = Path(__file__).resolve().parent.parent");
        let settings = extract_settings(
            "BASE_DIR = False\n\
             from paths import *\n\
             TEMPLATES = [{'DIRS': [BASE_DIR / 'templates']}]",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn cyclic_star_import_does_not_recurse_forever() {
        let mut resolver = MapResolver::default()
            .with_module("cycle", "from cycle import *\nINSTALLED_APPS = ['local']");
        let settings = extract_settings(
            "from cycle import *",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

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
        let mut resolver = MapResolver::default().with_module("base", "INSTALLED_APPS = ['base']");
        let settings = extract_settings(
            "from base import INSTALLED_APPS as IA\nINSTALLED_APPS = IA + ['local']",
            Utf8Path::new("/project/config/settings.py"),
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
        let mut resolver = RefusingNamedResolver::with_module("base", "INSTALLED_APPS = ['base']");
        let settings = extract_settings(
            "from base import INSTALLED_APPS",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

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
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn aliased_non_star_imported_path_can_feed_template_dirs() {
        let mut resolver = MapResolver::default()
            .with_module("base", "BASE_DIR = Path(__file__).resolve().parent.parent");
        let settings = extract_settings(
            "from base import BASE_DIR as BD\nTEMPLATES = [{'DIRS': [BD / 'templates']}]",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(settings.templates.extraction, ExtractionStatus::Complete);
        assert_eq!(
            settings.templates.backends[0].dirs,
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
                "/project/templates"
            ))]
        );
    }

    #[test]
    fn non_star_import_chain_reuses_extracted_imported_bindings() {
        let mut resolver = MapResolver::default()
            .with_module("common", "COMMON_APPS = ['common']")
            .with_module(
                "base",
                "from common import COMMON_APPS\nINSTALLED_APPS = COMMON_APPS + ['base']",
            );
        let settings = extract_settings(
            "from base import INSTALLED_APPS",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

        assert_eq!(
            settings.installed_apps.extraction,
            ExtractionStatus::Complete
        );
        assert_eq!(settings.installed_apps.values, ["common", "base"]);
    }

    #[test]
    fn cyclic_non_star_import_does_not_recurse_forever() {
        let mut resolver = MapResolver::default().with_module(
            "cycle",
            "from cycle import INSTALLED_APPS\nINSTALLED_APPS = ['local']",
        );
        let settings = extract_settings(
            "from cycle import INSTALLED_APPS",
            Utf8Path::new("/project/config/settings.py"),
            &mut resolver,
        );

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
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
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
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
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
            TemplateDirPath::Resolved(Utf8PathBuf::from("/project/config/b"))
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
            [TemplateDirPath::Resolved(Utf8PathBuf::from(
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
            TemplateDirPath::Unknown
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
