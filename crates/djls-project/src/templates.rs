use std::fmt;

use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::db::Db as ProjectDb;
use crate::project::Project;

/// First-party template files in Django loader precedence order.
///
/// Duplicate template names are kept because shadowed templates can still be
/// opened, inspected, and used as reference sources.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProjectTemplateFiles(Vec<ProjectTemplateFile>);

impl ProjectTemplateFiles {
    pub(crate) fn from_ordered_paths(
        db: &dyn ProjectDb,
        templates: Vec<(String, Utf8PathBuf)>,
    ) -> Self {
        Self(
            templates
                .into_iter()
                .map(|(name, path)| {
                    let file = db.get_or_create_file(&path);
                    ProjectTemplateFile::new(name, path, file)
                })
                .collect(),
        )
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProjectTemplateFile> {
        self.0.iter()
    }
}

impl fmt::Debug for ProjectTemplateFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProjectTemplateFiles")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProjectTemplateFile {
    name: String,
    path: Utf8PathBuf,
    file: File,
}

impl ProjectTemplateFile {
    pub(crate) fn new(name: String, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for ProjectTemplateFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectTemplateFile")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[salsa::tracked(returns(ref))]
pub fn project_template_files(db: &dyn ProjectDb, project: Project) -> ProjectTemplateFiles {
    // Freshness boundary: template discovery re-runs when any search-path root
    // revision is bumped (refresh_external_data does this), matching the
    // previous imperative refresh cadence. Template dirs that live outside
    // every registered root are still re-walked then, because this query
    // invalidates as a whole.
    for search_path in project.search_paths(db).iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }
    }

    let (search_dirs, _knowledge) = crate::settings::template_dirs(db, project);
    let mut templates = Vec::new();
    let walk_options = WalkOptions::unrestricted();

    for dir in search_dirs {
        if !db.path_is_dir(dir) {
            tracing::warn!("Template directory does not exist: {}", dir);
            continue;
        }

        let mut dir_templates = Vec::new();
        let entries = match db.walk_entries(dir, &walk_options) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!("Failed to walk template directory {}: {}", dir, err);
                continue;
            }
        };
        for entry in entries {
            if entry.kind != WalkEntryKind::File {
                continue;
            }
            let name = entry.relative.clean().to_string();
            dir_templates.push((name, entry.path));
        }

        dir_templates.sort_by(|(a_name, a_path), (b_name, b_path)| {
            a_name.cmp(b_name).then_with(|| a_path.cmp(b_path))
        });
        templates.extend(dir_templates);
    }

    ProjectTemplateFiles::from_ordered_paths(db, templates)
}
