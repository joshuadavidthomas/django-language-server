use std::sync::Arc;

use djls_project::ProjectFactsPhase;
use djls_project::ScopedTemplateLibraries;
use djls_project::template_directories;
use djls_project::template_library_catalog;
use djls_project::template_library_definition_facts;
use djls_project::template_library_inventory_dependencies;
use djls_project::template_library_structure_facts;
use djls_project::template_resolution;
use djls_semantic::Db as SemanticDb;
use djls_semantic::semantic_grammar_vocabulary;
use djls_source::File;
use djls_source::path_to_file;

/// The intrinsic Template Library products covered by one complete priming pass.
///
/// The file set covers registration discovery, callable sources, candidates, and settings. It is
/// not exhaustive dependency coverage: rule-only helpers can be absent, including helpers read by
/// the structural compatibility fallback. Callers must also invalidate on Python edits under
/// active source roots so demand-driven detail cannot leave stale diagnostics.
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

    /// Published eager source coverage for this priming pass.
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
}

/// Prepare registration inventory and global topology for project-aware Template analysis.
///
/// This deliberately does no per-Template work. Catalog assembly provides the
/// definition-name index; each active keyed library then contributes source and Block Spec facts;
/// finally the shared semantic grammar vocabulary is evaluated. Occurrence and completion demand
/// evaluate detailed Tag Rules and Filter Arity after readiness when needed.
#[must_use]
pub fn prime_template_library_products(db: &dyn SemanticDb) -> Option<PrimedTemplateLibraries> {
    let project = db.project()?;
    let libraries = template_library_catalog(db, project);
    let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(libraries);
    let mut reprime_files = Vec::new();
    let mut library_count = 0;

    for library in scoped_libraries.resolved_libraries() {
        library_count += 1;
        let key = library.id();
        let _ = template_library_definition_facts(db, key);
        let _ = template_library_structure_facts(db, key);
        if let Some(file) = library.source_file()
            && !reprime_files.contains(&file)
        {
            reprime_files.push(file);
        }
        for &file in template_library_inventory_dependencies(db, key) {
            if !reprime_files.contains(&file) {
                reprime_files.push(file);
            }
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
    ResolveTemplateDirs,
    IndexTemplateLibraries,
    IndexTemplates,
}

impl WarmCachePhase {
    #[must_use]
    pub const fn progress(self) -> WarmCacheProgress {
        match self {
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
            Self::ResolveTemplateDirs => {
                Some(template_directories(db, project).known_roots().count())
            }
            Self::IndexTemplateLibraries => {
                let libraries = template_library_catalog(db, project);
                Some(
                    ScopedTemplateLibraries::from_project_inventory(libraries)
                        .resolved_library_count(),
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
    WarmCachePhase::ResolveTemplateDirs,
    WarmCachePhase::IndexTemplateLibraries,
    WarmCachePhase::IndexTemplates,
];

#[must_use]
pub const fn warm_cache_phases() -> &'static [WarmCachePhase] {
    WARM_CACHE_PHASES
}
