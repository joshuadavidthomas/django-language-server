//! Refresh external project state into Salsa inputs.
//!
//! This module owns the closed set of refresh tasks, their presentation
//! metadata, and the validated refresh payload. The server owns orchestration:
//! concurrency, cancellation, progress bars, and when to apply the payload.

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::resolve::SearchPaths;
use crate::resolve::model_modules;
use crate::resolve::templatetag_modules;
use crate::settings::DjangoSettingsSources;
use crate::settings::settings_sources;
use crate::templates::refresh_templatetag_candidate_paths;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshData {
    search_paths: SearchPaths,
    file_paths: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RefreshDataPart {
    SearchPaths(SearchPaths),
    FilePaths(Vec<Utf8PathBuf>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshPhase {
    SearchPaths,
    SettingsSources,
    ModelModules,
    TemplateLibraryModules,
    TemplateTagCandidates,
}

impl RefreshPhase {
    fn run(self, db: &dyn ProjectDb, project: Project) -> RefreshDataPart {
        match self {
            Self::SearchPaths => RefreshDataPart::SearchPaths(SearchPaths::from_project_settings(
                db.file_system(),
                project.root(db),
                project.interpreter(db),
                project.pythonpath(db),
            )),
            Self::SettingsSources => {
                let sources: DjangoSettingsSources = settings_sources(db, project);
                RefreshDataPart::FilePaths(
                    sources
                        .root()
                        .into_iter()
                        .chain(sources.files().iter().copied().skip(1))
                        .map(|file| file.path(db).to_path_buf())
                        .collect(),
                )
            }
            Self::ModelModules => RefreshDataPart::FilePaths(
                model_modules(db, project)
                    .iter()
                    .map(|module| module.path().to_path_buf())
                    .collect(),
            ),
            Self::TemplateLibraryModules => RefreshDataPart::FilePaths(
                templatetag_modules(db, project)
                    .iter()
                    .map(|module| module.path().to_path_buf())
                    .collect(),
            ),
            Self::TemplateTagCandidates => {
                RefreshDataPart::FilePaths(refresh_templatetag_candidate_paths(db, project))
            }
        }
    }

    const fn descriptor(self) -> RefreshTaskDescriptor {
        match self {
            Self::SearchPaths => RefreshTaskDescriptor {
                group: RefreshTaskGroup::Environment,
                message: "Resolving environment",
                units: RefreshCountUnits {
                    singular: "search path",
                    plural: "search paths",
                },
            },
            Self::SettingsSources => RefreshTaskDescriptor {
                group: RefreshTaskGroup::Environment,
                message: "Scanning settings",
                units: RefreshCountUnits {
                    singular: "settings file",
                    plural: "settings files",
                },
            },
            Self::ModelModules => RefreshTaskDescriptor {
                group: RefreshTaskGroup::Facts,
                message: "Discovering model modules",
                units: RefreshCountUnits {
                    singular: "model module",
                    plural: "model modules",
                },
            },
            Self::TemplateLibraryModules => RefreshTaskDescriptor {
                group: RefreshTaskGroup::Facts,
                message: "Discovering template libraries",
                units: RefreshCountUnits {
                    singular: "template library module",
                    plural: "template library modules",
                },
            },
            Self::TemplateTagCandidates => RefreshTaskDescriptor {
                group: RefreshTaskGroup::Facts,
                message: "Discovering template tag candidates",
                units: RefreshCountUnits {
                    singular: "template tag candidate",
                    plural: "template tag candidates",
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshTask {
    phase: RefreshPhase,
}

impl RefreshTask {
    #[must_use]
    pub fn descriptor(self) -> RefreshTaskDescriptor {
        self.phase.descriptor()
    }

    #[must_use]
    pub fn run(self, db: &dyn ProjectDb, project: Project) -> RefreshPart {
        RefreshPart {
            phase: self.phase,
            part: self.phase.run(db, project),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshTaskDescriptor {
    pub group: RefreshTaskGroup,
    pub message: &'static str,
    pub units: RefreshCountUnits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshTaskGroup {
    Environment,
    Facts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshCountUnits {
    pub singular: &'static str,
    pub plural: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshPart {
    phase: RefreshPhase,
    part: RefreshDataPart,
}

impl RefreshPart {
    #[must_use]
    pub fn count(&self) -> usize {
        match &self.part {
            RefreshDataPart::SearchPaths(search_paths) => search_paths.iter().count(),
            RefreshDataPart::FilePaths(paths) => paths.len(),
        }
    }

    #[must_use]
    pub fn descriptor(&self) -> RefreshTaskDescriptor {
        self.phase.descriptor()
    }
}

const REFRESH_TASKS: &[RefreshTask] = &[
    RefreshTask {
        phase: RefreshPhase::SearchPaths,
    },
    RefreshTask {
        phase: RefreshPhase::SettingsSources,
    },
    RefreshTask {
        phase: RefreshPhase::ModelModules,
    },
    RefreshTask {
        phase: RefreshPhase::TemplateLibraryModules,
    },
    RefreshTask {
        phase: RefreshPhase::TemplateTagCandidates,
    },
];

#[must_use]
pub const fn refresh_tasks() -> &'static [RefreshTask] {
    REFRESH_TASKS
}

impl RefreshData {
    /// Build refresh data from the closed set of refresh task results.
    ///
    /// # Panics
    ///
    /// Panics if the parts do not contain exactly one result for each refresh
    /// task.
    #[must_use]
    pub fn assemble(parts: impl IntoIterator<Item = RefreshPart>) -> Self {
        let mut seen = Vec::new();
        let mut search_paths = None;
        let mut file_paths = Vec::new();

        for part in parts {
            assert!(
                !seen.contains(&part.phase),
                "refresh data must not include duplicate task results"
            );
            seen.push(part.phase);

            match part.part {
                RefreshDataPart::SearchPaths(paths) => {
                    search_paths = Some(paths);
                }
                RefreshDataPart::FilePaths(paths) => file_paths.extend(paths),
            }
        }

        for task in refresh_tasks() {
            assert!(
                seen.contains(&task.phase),
                "refresh data must include every refresh task result"
            );
        }
        let Some(search_paths) = search_paths else {
            unreachable!("RefreshPhase::SearchPaths result was marked as seen")
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
    pub fn file_paths(&self) -> &[Utf8PathBuf] {
        &self.file_paths
    }
}

#[must_use]
pub fn compute_refresh(db: &dyn ProjectDb, project: Project) -> RefreshData {
    RefreshData::assemble(
        refresh_tasks()
            .iter()
            .copied()
            .map(|task| task.run(db, project)),
    )
}

pub fn apply_refresh(db: &mut dyn ProjectDb, refresh: RefreshData) {
    let Some(project) = db.project() else {
        return;
    };
    let RefreshData {
        search_paths,
        file_paths,
    } = refresh;

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
