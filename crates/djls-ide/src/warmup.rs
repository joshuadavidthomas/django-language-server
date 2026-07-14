use std::sync::Arc;

use djls_project::ProjectFactsPhase;
use djls_project::template_directories;
use djls_project::template_libraries;
use djls_project::template_library_definition_facts;
use djls_project::template_resolution;
use djls_semantic::Db as SemanticDb;
use djls_semantic::library_filter_specs;
use djls_semantic::library_tag_specs;
use djls_semantic::semantic_grammar_vocabulary;
use djls_source::File;
use djls_source::path_to_file;

/// The intrinsic Template Library products covered by one complete priming pass.
///
/// The file set is the exact set of resolved Python sources whose keyed source,
/// Tag, and Filter products were evaluated. Callers use it to distinguish edits
/// that invalidate intrinsic readiness from unrelated Python changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimedTemplateLibraries {
    reprime_files: Arc<[File]>,
    full_reload_files: Arc<[File]>,
    library_count: usize,
}

impl PrimedTemplateLibraries {
    /// Python sources whose content changes require intrinsic re-priming.
    #[must_use]
    pub fn reprime_files(&self) -> &[File] {
        &self.reprime_files
    }

    /// Settings sources whose content changes require full Django Discovery.
    #[must_use]
    pub fn full_reload_files(&self) -> &[File] {
        &self.full_reload_files
    }

    /// All Python source dependencies covered by this priming pass.
    pub fn covered_files(&self) -> impl Iterator<Item = File> + '_ {
        self.full_reload_files
            .iter()
            .chain(self.reprime_files.iter())
            .copied()
    }

    #[must_use]
    pub const fn library_count(&self) -> usize {
        self.library_count
    }

    #[cfg(test)]
    #[must_use]
    fn covered_file_count(&self) -> usize {
        self.full_reload_files.len() + self.reprime_files.len()
    }
}

/// Evaluate every intrinsic product needed by project-aware Template analysis.
///
/// This deliberately does no per-Template work. Catalog assembly provides the
/// definition-name index; each active keyed library then contributes source
/// facts and independently backdatable Tag/Filter products; finally the shared
/// semantic grammar vocabulary is evaluated.
#[must_use]
pub fn prime_template_library_products(db: &dyn SemanticDb) -> Option<PrimedTemplateLibraries> {
    let project = db.project()?;
    let libraries = template_libraries(db, project);
    let environment = djls_project::TemplateEnvironment::from_project_inventory(libraries);
    let mut reprime_files = Vec::new();
    let mut library_count = 0;

    for library in environment.resolved_libraries() {
        library_count += 1;
        let key = library.key(db);
        let _ = template_library_definition_facts(db, key);
        let _ = library_tag_specs(db, project, key);
        let _ = library_filter_specs(db, key);
        if let Some(file) = library.source_file()
            && !reprime_files.contains(&file)
        {
            reprime_files.push(file);
        }
    }

    // Candidate sources can start or stop contributing registrations without
    // changing their file identity. Prime tracks every known candidate, not
    // only candidates selected into the current catalog.
    let candidate_sources = ProjectFactsPhase::TemplateTagCandidates.run(db, project);
    for path in candidate_sources.file_paths() {
        if let Ok(file) = path_to_file(db, path)
            && !reprime_files.contains(&file)
        {
            reprime_files.push(file);
        }
    }

    // Settings source edits can alter installed apps, library mappings,
    // builtins, and search configuration, so they must restart full discovery.
    let settings_sources = ProjectFactsPhase::SettingsSources.run(db, project);
    let mut full_reload_files = Vec::new();
    for path in settings_sources.file_paths() {
        if let Ok(file) = path_to_file(db, path) {
            full_reload_files.push(file);
            reprime_files.retain(|candidate| *candidate != file);
        }
    }

    let _ = semantic_grammar_vocabulary(db, project);

    Some(PrimedTemplateLibraries {
        reprime_files: reprime_files.into(),
        full_reload_files: full_reload_files.into(),
        library_count,
    })
}

/// Prepare all shared products used by one-shot project Template analysis.
///
/// Intrinsic Template Library products are always primed before the shared
/// Template index. Server readiness should use [`prime_template_library_products`]
/// directly because it deliberately excludes per-Template discovery.
#[must_use]
pub fn prepare_project_template_analysis(db: &dyn SemanticDb) -> Option<()> {
    prime_template_library_products(db)?;
    WarmCachePhase::IndexTemplates.run(db).count()?;
    Some(())
}

/// Noun pair used when reporting a count for an IDE cache warm-up phase.
///
/// This intentionally mirrors `djls_project::CountLabel`; each crate keeps a
/// tiny local type instead of depending on a shared utility vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountLabel {
    pub singular: &'static str,
    pub plural: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WarmCacheProgress {
    pub message: &'static str,
    pub count_label: Option<CountLabel>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarmCachePhase {
    BuildModelGraph,
    ResolveTemplateDirs,
    IndexTemplateLibraries,
    IndexTemplates,
}

impl WarmCachePhase {
    #[must_use]
    pub const fn progress(self) -> WarmCacheProgress {
        match self {
            Self::BuildModelGraph => WarmCacheProgress {
                message: "Building model graph",
                count_label: None,
            },
            Self::ResolveTemplateDirs => WarmCacheProgress {
                message: "Resolving template directories",
                count_label: Some(CountLabel {
                    singular: "template directory",
                    plural: "template directories",
                }),
            },
            Self::IndexTemplateLibraries => WarmCacheProgress {
                message: "Indexing template libraries",
                count_label: Some(CountLabel {
                    singular: "template library",
                    plural: "template libraries",
                }),
            },
            Self::IndexTemplates => WarmCacheProgress {
                message: "Indexing templates",
                count_label: Some(CountLabel {
                    singular: "template",
                    plural: "templates",
                }),
            },
        }
    }

    #[must_use]
    pub fn run(self, db: &dyn SemanticDb) -> WarmCachePart {
        let count = self.compute(db);
        WarmCachePart { phase: self, count }
    }

    fn compute(self, db: &dyn SemanticDb) -> Option<usize> {
        let project = db.project()?;

        match self {
            Self::BuildModelGraph => {
                let _ = db.model_graph();
                None
            }
            Self::ResolveTemplateDirs => {
                Some(template_directories(db, project).known_roots().count())
            }
            Self::IndexTemplateLibraries => {
                let libraries = djls_project::template_libraries(db, project);
                Some(
                    djls_project::TemplateEnvironment::from_project_inventory(libraries)
                        .installed_library_count(),
                )
            }
            Self::IndexTemplates => Some(template_resolution(db, project).origins(db).count()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WarmCachePart {
    phase: WarmCachePhase,
    count: Option<usize>,
}

impl WarmCachePart {
    #[must_use]
    pub const fn phase(&self) -> WarmCachePhase {
        self.phase
    }

    #[must_use]
    pub const fn count(&self) -> Option<usize> {
        self.count
    }
}

const WARM_CACHE_PHASES: &[WarmCachePhase] = &[
    WarmCachePhase::BuildModelGraph,
    WarmCachePhase::ResolveTemplateDirs,
    WarmCachePhase::IndexTemplateLibraries,
    WarmCachePhase::IndexTemplates,
];

#[must_use]
pub const fn warm_cache_phases() -> &'static [WarmCachePhase] {
    WARM_CACHE_PHASES
}

#[cfg(test)]
mod tests {
    use djls_project::Db as _;
    use djls_project::template_resolution;
    use djls_testing::ProjectFixture;
    use djls_testing::SalsaEventLog;
    use djls_testing::TestDatabase;

    use super::*;

    fn execution_count(names: &[String], query: &str) -> usize {
        names
            .iter()
            .filter(|name| name.rsplit("::").next() == Some(query))
            .count()
    }

    fn install_project_fixture(db: &mut TestDatabase) {
        ProjectFixture::new("/project")
            .django_settings_module("settings")
            .file(
                "/project/settings.py",
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/project/templates'], 'OPTIONS': {'builtins': ['tags']}}]\n",
            )
            .file(
                "/project/tags.py",
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello(): pass\n@register.filter\ndef shout(value): pass\n",
            )
            .file("/project/templates/page.html", "{% hello %}{{ value|shout }}")
            .install(db);
    }

    #[test]
    fn final_state_matrix_01_04_shared_prime_is_exact_and_has_no_template_work() {
        let events = SalsaEventLog::default();
        let mut db = TestDatabase::with_event_log(events.clone());
        install_project_fixture(&mut db);
        let _ = events.take();

        let primed = prime_template_library_products(&db).expect("fixture has a Project");
        let covered_paths: Vec<_> = primed
            .covered_files()
            .map(|file| file.path(&db).as_str())
            .collect();
        assert!(covered_paths.contains(&"/project/tags.py"));
        assert!(covered_paths.contains(&"/project/settings.py"));
        assert_eq!(primed.covered_file_count(), covered_paths.len());
        assert!(
            primed
                .reprime_files()
                .iter()
                .any(|file| file.path(&db) == camino::Utf8Path::new("/project/tags.py"))
        );
        assert_eq!(primed.full_reload_files().len(), 1);

        let names = events.take_will_execute_names(&db);
        assert_eq!(
            execution_count(&names, "library_tag_specs"),
            primed.library_count()
        );
        assert_eq!(
            execution_count(&names, "library_filter_specs"),
            primed.library_count()
        );
        assert_eq!(execution_count(&names, "semantic_grammar_vocabulary"), 1);
        for forbidden in [
            "parse_template",
            "template_analysis_projection_for_file_in_scope",
            "validate_template_file",
        ] {
            assert_eq!(
                execution_count(&names, forbidden),
                0,
                "priming ran {forbidden}"
            );
        }

        let repeated = prime_template_library_products(&db).expect("fixture has a Project");
        assert_eq!(repeated, primed);
        let names = events.take_will_execute_names(&db);
        for intrinsic in [
            "template_library_definition_facts",
            "library_tag_specs",
            "library_filter_specs",
            "semantic_grammar_vocabulary",
        ] {
            assert_eq!(
                execution_count(&names, intrinsic),
                0,
                "repeated prime ran {intrinsic}"
            );
        }
    }

    #[test]
    fn project_template_preparation_orders_and_reuses_shared_products() {
        let events = SalsaEventLog::default();
        let mut db = TestDatabase::with_event_log(events.clone());
        install_project_fixture(&mut db);
        let _ = events.take();

        prepare_project_template_analysis(&db).expect("fixture has a Project");
        let project = db.project().expect("fixture has a Project");
        assert_eq!(template_resolution(&db, project).origins(&db).count(), 1);

        let names = events.take_will_execute_names(&db);
        let intrinsic_position = names
            .iter()
            .position(|name| name.rsplit("::").next() == Some("semantic_grammar_vocabulary"))
            .expect("preparation primes intrinsic products");
        let index_position = names
            .iter()
            .position(|name| name.rsplit("::").next() == Some("template_directory_index"))
            .expect("preparation builds the shared Template index");
        assert!(intrinsic_position < index_position);

        assert_eq!(prepare_project_template_analysis(&db), Some(()));
        let repeated_names = events.take_will_execute_names(&db);
        for shared_query in ["semantic_grammar_vocabulary", "template_directory_index"] {
            assert_eq!(
                execution_count(&repeated_names, shared_query),
                0,
                "repeated preparation ran {shared_query}"
            );
        }
    }
}
