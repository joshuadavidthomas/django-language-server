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
    SettingsExtraction::default()
        .extract_module(source, module_path, resolver)
        .to_settings()
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
    ) -> Arc<SettingsBindings> {
        let path = module_path.to_path_buf();
        if let Some(cached) = self.cache.get(&path) {
            return Arc::clone(cached);
        }
        if !self.active.insert(path.clone()) {
            return Arc::new(SettingsBindings::default());
        }

        let mut collector = SettingsBindingsCollector::new(module_path, resolver, self);

        if let Ok(parsed) = parse_module(source) {
            let module = parsed.into_syntax();
            collector.walk_body(&module.body);
        } else {
            collector.mark_syntax_error();
        }

        let bindings = Arc::new(collector.into_bindings());
        self.active.remove(&path);
        self.cache.insert(path, Arc::clone(&bindings));
        bindings
    }

    fn extract_star_import(
        &mut self,
        imported: &SettingsSource,
        star_imports: &mut dyn SettingsSourceResolver,
    ) -> Option<Arc<SettingsBindings>> {
        if self.active.contains(&imported.path) {
            return None;
        }
        Some(self.extract_module(&imported.source, &imported.path, star_imports))
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use rustc_hash::FxHashMap;

    use super::*;
    use crate::settings::types::SettingExtraction;
    use crate::settings::types::SettingsStarImport;
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
            import: &SettingsStarImport,
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
        assert_eq!(settings.installed_apps.extraction, SettingExtraction::Full);
        assert_eq!(
            settings.installed_apps.values,
            ["django.contrib.auth", "app"]
        );
    }

    #[test]
    fn annotated_assignment_is_full() {
        let settings = extract("INSTALLED_APPS: list[str] = ['app']");
        assert_eq!(settings.installed_apps.extraction, SettingExtraction::Full);
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
            SettingExtraction::Partial
        );
        assert_eq!(settings.installed_apps.values, ["a", "b"]);
    }

    #[test]
    fn unsupported_assignment_is_unsupported() {
        let settings = extract("INSTALLED_APPS = get_apps()");
        assert_eq!(
            settings.installed_apps.extraction,
            SettingExtraction::Unsupported
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
            SettingExtraction::Partial
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
        assert_eq!(settings.installed_apps.extraction, SettingExtraction::Full);
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

        assert_eq!(settings.templates.extraction, SettingExtraction::Partial);
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

        assert_eq!(settings.templates.extraction, SettingExtraction::Full);
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

        assert_eq!(settings.installed_apps.extraction, SettingExtraction::Full);
        assert_eq!(settings.installed_apps.values, ["local"]);
    }

    #[test]
    fn unresolvable_star_import_is_partial() {
        let settings = extract("from missing import *");
        assert_eq!(
            settings.installed_apps.extraction,
            SettingExtraction::Partial
        );
        assert_eq!(settings.templates.extraction, SettingExtraction::Partial);
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
        assert_eq!(settings.templates.extraction, SettingExtraction::Partial);
        assert_eq!(
            settings.templates.backends[0].extraction,
            SettingExtraction::Partial
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
        assert_eq!(settings.templates.extraction, SettingExtraction::Partial);
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
            SettingExtraction::Partial
        );
        assert_eq!(settings.installed_apps.values, ["debug", "base"]);
    }

    #[test]
    fn unsupported_assignment_then_valid_assignment_is_full() {
        let settings = extract("INSTALLED_APPS = get_apps()\nINSTALLED_APPS = ['blog']");
        assert_eq!(settings.installed_apps.extraction, SettingExtraction::Full);
        assert_eq!(settings.installed_apps.values, ["blog"]);
    }

    #[test]
    fn unsupported_assignment_followed_by_soft_demotion_stays_unsupported() {
        let settings = extract("INSTALLED_APPS = get_apps()\nfrom missing import *");
        assert_eq!(
            settings.installed_apps.extraction,
            SettingExtraction::Unsupported
        );
        assert!(settings.installed_apps.values.is_empty());
    }

    #[test]
    fn syntax_error_without_prior_settings_returns_partial_settings() {
        let settings = extract("INSTALLED_APPS = [");
        assert_eq!(
            settings.installed_apps.extraction,
            SettingExtraction::Partial
        );
        assert_eq!(settings.templates.extraction, SettingExtraction::Partial);
    }
}
