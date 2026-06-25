//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::templates::templatetag_candidate_paths;
use crate::project::Project;
use crate::resolve::SearchPaths;
use crate::resolve::model_modules;
use crate::resolve::templatetag_modules;
use crate::settings::settings_source_files;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshQueryResult {
    query: RefreshQuery,
    part: RefreshDataPart,
}

impl RefreshQueryResult {
    #[must_use]
    pub fn query(&self) -> RefreshQuery {
        self.query
    }

    #[must_use]
    pub fn item_count(&self) -> usize {
        match &self.part {
            RefreshDataPart::SearchPaths(search_paths) => search_paths.iter().count(),
            RefreshDataPart::FilePaths(paths) => paths.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshQuery {
    SearchPaths,
    SettingsSources,
    ModelModules,
    TemplateLibraryModules,
    TemplateTagCandidates,
}

impl RefreshQuery {
    pub const ALL: &'static [Self] = &[
        Self::SearchPaths,
        Self::SettingsSources,
        Self::ModelModules,
        Self::TemplateLibraryModules,
        Self::TemplateTagCandidates,
    ];

    #[must_use]
    pub fn compute(self, db: &dyn ProjectDb, project: Project) -> RefreshQueryResult {
        let part = match self {
            Self::SearchPaths => RefreshDataPart::SearchPaths(SearchPaths::from_project_settings(
                db.file_system(),
                project.root(db),
                project.interpreter(db),
                project.pythonpath(db),
            )),
            Self::SettingsSources => RefreshDataPart::FilePaths(
                settings_source_files(db, project)
                    .into_iter()
                    .map(|file| file.path(db).to_path_buf())
                    .collect(),
            ),
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
                RefreshDataPart::FilePaths(templatetag_candidate_paths(db, project))
            }
        };

        RefreshQueryResult { query: self, part }
    }
}

impl RefreshData {
    /// Build refresh data from the closed set of refresh query results.
    ///
    /// # Panics
    ///
    /// Panics if the results do not contain exactly one result for each
    /// [`RefreshQuery::ALL`] member.
    #[must_use]
    pub fn from_query_results(results: impl IntoIterator<Item = RefreshQueryResult>) -> Self {
        let mut seen = Vec::new();
        let mut search_paths = None;
        let mut file_paths = Vec::new();

        for result in results {
            assert!(
                !seen.contains(&result.query),
                "refresh data must not include duplicate query results"
            );
            seen.push(result.query);

            match result.part {
                RefreshDataPart::SearchPaths(paths) => {
                    search_paths = Some(paths);
                }
                RefreshDataPart::FilePaths(paths) => file_paths.extend(paths),
            }
        }

        for query in RefreshQuery::ALL {
            assert!(
                seen.contains(query),
                "refresh data must include every refresh query result"
            );
        }
        let Some(search_paths) = search_paths else {
            unreachable!("RefreshQuery::SearchPaths result was marked as seen")
        };

        file_paths.sort();
        file_paths.dedup();

        Self {
            search_paths,
            file_paths,
        }
    }

    #[must_use]
    pub fn file_paths(&self) -> &[Utf8PathBuf] {
        &self.file_paths
    }
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
