//! Discover external Project Facts and synchronize them into Salsa inputs.
//!
//! This module owns the closed set of Django Discovery phases, domain progress
//! metadata, and the validated discovery payload. The server owns runtime
//! orchestration and LSP presentation: concurrency, cancellation, progress
//! transport, and when to apply the payload.

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::models::model_modules;
use crate::project::Project;
use crate::python::SearchPaths;
use crate::settings::DjangoSettingsSources;
use crate::settings::settings_sources;
use crate::templates::discover_templatetag_candidate_paths;
use crate::templates::template_libraries;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DjangoDiscoveryData {
    search_paths: SearchPaths,
    file_paths: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DjangoDiscoveryDataPart {
    SearchPaths(SearchPaths),
    FilePaths(Vec<Utf8PathBuf>),
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
    fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::SearchPaths), |phase| (*phase).next())
    }

    const fn next(self) -> Option<Self> {
        match self {
            Self::SearchPaths => None,
        }
    }

    #[must_use]
    const fn progress(self) -> DjangoDiscoveryProgress {
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

    fn compute(self, db: &dyn ProjectDb, project: Project) -> DjangoDiscoveryDataPart {
        match self {
            Self::SearchPaths => {
                DjangoDiscoveryDataPart::SearchPaths(SearchPaths::from_project_settings(
                    db.file_system(),
                    project.root(db),
                    project.interpreter(db),
                    project.pythonpath(db),
                ))
            }
        }
    }
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
    fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::SettingsSources), |phase| (*phase).next())
    }

    const fn next(self) -> Option<Self> {
        match self {
            Self::SettingsSources => Some(Self::ModelModules),
            Self::ModelModules => Some(Self::TemplateLibrarySources),
            Self::TemplateLibrarySources => Some(Self::TemplateTagCandidates),
            Self::TemplateTagCandidates => None,
        }
    }

    #[must_use]
    const fn progress(self) -> DjangoDiscoveryProgress {
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

    fn compute(self, db: &dyn ProjectDb, project: Project) -> DjangoDiscoveryDataPart {
        match self {
            Self::SettingsSources => {
                let sources: DjangoSettingsSources = settings_sources(db, project);
                DjangoDiscoveryDataPart::FilePaths(
                    sources
                        .root()
                        .into_iter()
                        .chain(sources.files().iter().copied().skip(1))
                        .map(|file| file.path(db).to_path_buf())
                        .collect(),
                )
            }
            Self::ModelModules => DjangoDiscoveryDataPart::FilePaths(
                model_modules(db, project)
                    .iter()
                    .map(|module| module.path().to_path_buf())
                    .collect(),
            ),
            Self::TemplateLibrarySources => DjangoDiscoveryDataPart::FilePaths(
                template_libraries(db, project)
                    .active_libraries()
                    .map(|library| library.file().path(db).to_path_buf())
                    .collect(),
            ),
            Self::TemplateTagCandidates => DjangoDiscoveryDataPart::FilePaths(
                discover_templatetag_candidate_paths(db, project),
            ),
        }
    }
}

/// Closed set of Django Discovery phases, with phase membership encoded by the
/// variant that carries it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryPhase {
    Environment(EnvironmentPhase),
    ProjectFacts(ProjectFactsPhase),
}

impl DiscoveryPhase {
    #[must_use]
    pub fn environment_phase_count() -> usize {
        EnvironmentPhase::all().count()
    }

    #[must_use]
    pub fn project_facts_phase_count() -> usize {
        ProjectFactsPhase::all().count()
    }

    #[must_use]
    pub const fn progress(self) -> DjangoDiscoveryProgress {
        match self {
            Self::Environment(phase) => phase.progress(),
            Self::ProjectFacts(phase) => phase.progress(),
        }
    }

    #[must_use]
    pub fn run(self, db: &dyn ProjectDb, project: Project) -> DjangoDiscoveryPart {
        DjangoDiscoveryPart {
            phase: self,
            part: self.compute(db, project),
        }
    }

    fn compute(self, db: &dyn ProjectDb, project: Project) -> DjangoDiscoveryDataPart {
        match self {
            Self::Environment(phase) => phase.compute(db, project),
            Self::ProjectFacts(phase) => phase.compute(db, project),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DjangoDiscoveryPart {
    phase: DiscoveryPhase,
    part: DjangoDiscoveryDataPart,
}

impl DjangoDiscoveryPart {
    #[must_use]
    pub const fn phase(&self) -> DiscoveryPhase {
        self.phase
    }

    #[must_use]
    pub fn count(&self) -> usize {
        match &self.part {
            DjangoDiscoveryDataPart::SearchPaths(search_paths) => search_paths.iter().count(),
            DjangoDiscoveryDataPart::FilePaths(paths) => paths.len(),
        }
    }
}

pub fn django_discovery_phases() -> impl Iterator<Item = DiscoveryPhase> {
    EnvironmentPhase::all()
        .map(DiscoveryPhase::Environment)
        .chain(ProjectFactsPhase::all().map(DiscoveryPhase::ProjectFacts))
}

impl DjangoDiscoveryData {
    /// Build discovery data from the closed set of Django Discovery phase results.
    ///
    /// # Panics
    ///
    /// Panics if the parts do not contain exactly one result for each Django
    /// Discovery phase.
    #[must_use]
    pub fn assemble(parts: impl IntoIterator<Item = DjangoDiscoveryPart>) -> Self {
        let mut seen = Vec::new();
        let mut search_paths = None;
        let mut file_paths = Vec::new();

        for part in parts {
            assert!(
                !seen.contains(&part.phase),
                "discovery data must not include duplicate phase results"
            );
            seen.push(part.phase);

            match part.part {
                DjangoDiscoveryDataPart::SearchPaths(paths) => {
                    search_paths = Some(paths);
                }
                DjangoDiscoveryDataPart::FilePaths(paths) => file_paths.extend(paths),
            }
        }

        for phase in django_discovery_phases() {
            assert!(
                seen.contains(&phase),
                "discovery data must include every Django Discovery phase result"
            );
        }
        let Some(search_paths) = search_paths else {
            unreachable!("EnvironmentPhase::SearchPaths result was marked as seen")
        };

        file_paths.sort();
        file_paths.dedup();

        Self {
            search_paths,
            file_paths,
        }
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
pub fn compute_django_discovery(db: &dyn ProjectDb, project: Project) -> DjangoDiscoveryData {
    DjangoDiscoveryData::assemble(django_discovery_phases().map(|phase| phase.run(db, project)))
}

pub fn apply_django_discovery(db: &mut dyn ProjectDb, discovery: DjangoDiscoveryData) {
    let Some(project) = db.project() else {
        return;
    };
    let DjangoDiscoveryData {
        search_paths,
        file_paths,
    } = discovery;

    let search_paths_changed = project.search_paths(db) != &search_paths;
    if search_paths_changed {
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

    for path in file_paths {
        let file = db.get_or_create_file(&path);
        let current = file.source(db);
        let latest = db.read_file(&path).unwrap_or_default();

        if current.as_str() != latest {
            db.bump_file_revision(file);
        }
    }
}
