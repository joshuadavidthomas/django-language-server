//! Discover the Django Environment and Project Facts and synchronize them into Salsa.
//!
//! Environment discovery must be computed and applied before Project Facts are
//! computed. Applying the environment registers and invalidates source roots,
//! then rescans known files. Project Facts can then be computed from a fresh
//! database clone without a second invalidation wave.

use camino::Utf8PathBuf;
use djls_source::ChangeEvent;
use djls_source::SourceChanges;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::models::model_modules;
use crate::project::Project;
use crate::python::SearchPaths;
use crate::settings::DjangoSettingsSources;
use crate::settings::settings_sources;
use crate::templates::TemplateLibrary;
use crate::templates::discover_templatetag_candidate_paths;
use crate::templates::template_libraries;

/// The resolved Python environment for a Project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DjangoEnvironmentData {
    search_paths: SearchPaths,
}

/// The source identities reached while discovering Project Facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFactsData {
    file_paths: Vec<Utf8PathBuf>,
}

/// Noun pair used when reporting a count for a Django Discovery phase.
///
/// This intentionally mirrors `djls_ide::CountLabel`; each crate keeps a tiny
/// local type instead of depending on a shared utility vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountLabel {
    pub singular: &'static str,
    pub plural: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DjangoDiscoveryProgress {
    pub message: &'static str,
    pub count_label: CountLabel,
}

/// Environment discovery phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentPhase {
    SearchPaths,
}

impl EnvironmentPhase {
    const fn next(self) -> Option<Self> {
        match self {
            Self::SearchPaths => None,
        }
    }

    #[must_use]
    pub const fn progress(self) -> DjangoDiscoveryProgress {
        match self {
            Self::SearchPaths => DjangoDiscoveryProgress {
                message: "Resolving Python search paths",
                count_label: CountLabel {
                    singular: "search path",
                    plural: "search paths",
                },
            },
        }
    }

    #[must_use]
    pub fn run(self, db: &dyn ProjectDb, project: Project) -> EnvironmentPart {
        match self {
            Self::SearchPaths => {
                let search_paths = SearchPaths::from_project_settings(
                    db.file_system(),
                    project.root(db),
                    project.interpreter(db),
                    project.pythonpath(db),
                );
                EnvironmentPart {
                    phase: self,
                    count: search_paths.iter().count(),
                    search_paths,
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentPart {
    phase: EnvironmentPhase,
    count: usize,
    search_paths: SearchPaths,
}

impl EnvironmentPart {
    #[must_use]
    pub const fn phase(&self) -> EnvironmentPhase {
        self.phase
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }
}

pub fn environment_phases() -> impl Iterator<Item = EnvironmentPhase> {
    std::iter::successors(Some(EnvironmentPhase::SearchPaths), |phase| phase.next())
}

impl DjangoEnvironmentData {
    /// Build environment data from the closed set of Environment phase results.
    ///
    /// # Panics
    ///
    /// Panics if the parts do not contain exactly one result for each phase.
    #[must_use]
    pub fn assemble(parts: impl IntoIterator<Item = EnvironmentPart>) -> Self {
        let mut search_paths = None;

        for part in parts {
            match part.phase {
                EnvironmentPhase::SearchPaths => {
                    assert!(
                        search_paths.is_none(),
                        "environment data must not include duplicate phase results"
                    );
                    search_paths = Some(part.search_paths);
                }
            }
        }

        let Some(search_paths) = search_paths else {
            panic!("environment data must include every Environment phase result")
        };
        Self { search_paths }
    }
}

#[must_use]
pub fn compute_django_environment(db: &dyn ProjectDb, project: Project) -> DjangoEnvironmentData {
    DjangoEnvironmentData::assemble(environment_phases().map(|phase| phase.run(db, project)))
}

/// Apply the resolved environment and perform the reload's sole invalidation wave.
///
/// Search roots are registered before becoming active. Every active root is
/// bumped, then all already-known paths are synchronized from the authoritative
/// filesystem view (including editor overlays).
pub fn apply_django_environment(db: &mut dyn ProjectDb, environment: DjangoEnvironmentData) {
    let Some(project) = db.project() else {
        return;
    };
    let DjangoEnvironmentData { search_paths } = environment;

    if project.search_paths(db) != &search_paths {
        search_paths.register_roots(db);
        project.set_search_paths(db).to(search_paths);
    }

    let roots: Vec<_> = project
        .search_paths(db)
        .iter()
        .filter_map(|search_path| db.files().root(db, search_path.path()))
        .collect();
    for root in roots {
        db.bump_file_root_revision(root);
    }

    SourceChanges::new([ChangeEvent::Rescan]).apply(db);
}

/// Project Facts discovery phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFactsPhase {
    SettingsSources,
    ModelModules,
    TemplateLibrarySources,
    TemplateTagCandidates,
}

impl ProjectFactsPhase {
    const fn next(self) -> Option<Self> {
        match self {
            Self::SettingsSources => Some(Self::ModelModules),
            Self::ModelModules => Some(Self::TemplateLibrarySources),
            Self::TemplateLibrarySources => Some(Self::TemplateTagCandidates),
            Self::TemplateTagCandidates => None,
        }
    }

    #[must_use]
    pub const fn progress(self) -> DjangoDiscoveryProgress {
        match self {
            Self::SettingsSources => DjangoDiscoveryProgress {
                message: "Reading settings sources",
                count_label: CountLabel {
                    singular: "settings file",
                    plural: "settings files",
                },
            },
            Self::ModelModules => DjangoDiscoveryProgress {
                message: "Discovering model modules",
                count_label: CountLabel {
                    singular: "model module",
                    plural: "model modules",
                },
            },
            Self::TemplateLibrarySources => DjangoDiscoveryProgress {
                message: "Discovering template libraries",
                count_label: CountLabel {
                    singular: "template library source",
                    plural: "template library sources",
                },
            },
            Self::TemplateTagCandidates => DjangoDiscoveryProgress {
                message: "Discovering template tag candidates",
                count_label: CountLabel {
                    singular: "template tag candidate",
                    plural: "template tag candidates",
                },
            },
        }
    }

    #[must_use]
    pub fn run(self, db: &dyn ProjectDb, project: Project) -> ProjectFactsPart {
        let file_paths = match self {
            Self::SettingsSources => {
                let sources: DjangoSettingsSources = settings_sources(db, project);
                sources
                    .root()
                    .into_iter()
                    .chain(sources.files().iter().copied().skip(1))
                    .map(|file| file.path(db).to_path_buf())
                    .collect()
            }
            Self::ModelModules => model_modules(db, project)
                .iter()
                .map(|module| module.path().to_path_buf())
                .collect(),
            Self::TemplateLibrarySources => template_libraries(db, project)
                .resolved_libraries()
                .filter_map(TemplateLibrary::source_file)
                .map(|file| file.path(db).to_path_buf())
                .collect(),
            Self::TemplateTagCandidates => discover_templatetag_candidate_paths(db, project),
        };
        ProjectFactsPart {
            phase: self,
            file_paths,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFactsPart {
    phase: ProjectFactsPhase,
    file_paths: Vec<Utf8PathBuf>,
}

impl ProjectFactsPart {
    #[must_use]
    pub const fn phase(&self) -> ProjectFactsPhase {
        self.phase
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.file_paths.len()
    }
}

pub fn project_facts_phases() -> impl Iterator<Item = ProjectFactsPhase> {
    std::iter::successors(Some(ProjectFactsPhase::SettingsSources), |phase| {
        phase.next()
    })
}

impl ProjectFactsData {
    /// Build Project Facts from the closed set of phase results.
    ///
    /// # Panics
    ///
    /// Panics if the parts do not contain exactly one result for each phase.
    #[must_use]
    pub fn assemble(parts: impl IntoIterator<Item = ProjectFactsPart>) -> Self {
        let mut seen = Vec::new();
        let mut file_paths = Vec::new();

        for part in parts {
            assert!(
                !seen.contains(&part.phase),
                "Project Facts must not include duplicate phase results"
            );
            seen.push(part.phase);
            file_paths.extend(part.file_paths);
        }
        for phase in project_facts_phases() {
            assert!(
                seen.contains(&phase),
                "Project Facts must include every Project Facts phase result"
            );
        }

        file_paths.sort();
        file_paths.dedup();
        Self { file_paths }
    }

    #[must_use]
    pub fn discovered_file_count(&self) -> usize {
        self.file_paths.len()
    }

    #[must_use]
    pub const fn discovered_file_count_label() -> CountLabel {
        CountLabel {
            singular: "discovered file",
            plural: "discovered files",
        }
    }

    #[must_use]
    pub fn file_paths(&self) -> &[Utf8PathBuf] {
        &self.file_paths
    }
}

#[must_use]
pub fn compute_project_facts(db: &dyn ProjectDb, project: Project) -> ProjectFactsData {
    ProjectFactsData::assemble(project_facts_phases().map(|phase| phase.run(db, project)))
}
