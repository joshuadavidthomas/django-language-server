use djls_project::template_directories;
use djls_project::template_libraries;
use djls_project::template_resolution;
use djls_semantic::Db as SemanticDb;
use djls_semantic::library_filter_specs;
use djls_semantic::library_tag_specs;

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
    BuildTagSpecs,
    BuildFilterAritySpecs,
    BuildModelGraph,
    ResolveTemplateDirs,
    IndexTemplateLibraries,
    IndexTemplates,
}

impl WarmCachePhase {
    #[must_use]
    pub const fn progress(self) -> WarmCacheProgress {
        match self {
            Self::BuildTagSpecs => WarmCacheProgress {
                message: "Building tag specs",
                count_label: None,
            },
            Self::BuildFilterAritySpecs => WarmCacheProgress {
                message: "Building filter arity specs",
                count_label: None,
            },
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
            Self::BuildTagSpecs => {
                for library in template_libraries(db, project).resolved_libraries() {
                    let _ = library_tag_specs(db, project, library.key(db));
                }
                None
            }
            Self::BuildFilterAritySpecs => {
                for library in template_libraries(db, project).resolved_libraries() {
                    let _ = library_filter_specs(db, library.key(db));
                }
                None
            }
            Self::BuildModelGraph => {
                let _ = db.model_graph();
                None
            }
            Self::ResolveTemplateDirs => {
                Some(template_directories(db, project).known_roots().count())
            }
            Self::IndexTemplateLibraries => {
                let libraries = djls_project::template_libraries(db, project);
                Some(libraries.installed_library_count())
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
    WarmCachePhase::BuildTagSpecs,
    WarmCachePhase::BuildFilterAritySpecs,
    WarmCachePhase::BuildModelGraph,
    WarmCachePhase::ResolveTemplateDirs,
    WarmCachePhase::IndexTemplateLibraries,
    WarmCachePhase::IndexTemplates,
];

#[must_use]
pub const fn warm_cache_phases() -> &'static [WarmCachePhase] {
    WARM_CACHE_PHASES
}
