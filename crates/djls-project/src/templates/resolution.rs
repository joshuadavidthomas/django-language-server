use std::fmt;

use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Utf8PathClean;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use djls_source::safe_join;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonPackage;
use crate::settings::TemplateDirPath;
use crate::settings::django_settings;
use crate::templates::guess_package_module_name_from_installed_app_entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateDirStatus {
    Complete,
    Incomplete,
}

impl TemplateDirStatus {
    const fn from_complete(complete: bool) -> Self {
        if complete {
            Self::Complete
        } else {
            Self::Incomplete
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateDirResolution {
    dirs: Vec<Utf8PathBuf>,
    status: TemplateDirStatus,
}

#[salsa::tracked(returns(ref))]
pub(crate) fn template_dirs(db: &dyn ProjectDb, project: Project) -> TemplateDirResolution {
    project.touch_search_path_roots(db);

    let settings = django_settings(db, project);
    let mut dirs = Vec::new();
    let mut complete = settings.templates.is_fully_extracted();
    let backend_count = settings.templates.backends.len();

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        complete &= backend.is_fully_extracted();

        for dir in &backend.dirs {
            match dir {
                TemplateDirPath::Resolved(path) => dirs.push(path.clone()),
                TemplateDirPath::Unknown => complete = false,
            }
        }

        if backend.app_dirs == Some(true) {
            complete &= settings.installed_apps.is_fully_extracted();
            for app in &settings.installed_apps.values {
                let Some(package_module) = guess_package_module_name_from_installed_app_entry(app)
                else {
                    complete = false;
                    continue;
                };
                let Some(package) = PythonPackage::resolve(db, project, package_module) else {
                    complete = false;
                    continue;
                };

                let templates_dir = package.dir().join("templates");
                if db.path_is_dir(&templates_dir) {
                    dirs.push(templates_dir);
                }
            }
        }
    }

    TemplateDirResolution {
        dirs,
        status: TemplateDirStatus::from_complete(complete),
    }
}

#[salsa::interned]
#[derive(Debug)]
pub struct TemplateName {
    #[returns(ref)]
    pub name: String,
}

#[salsa::tracked]
pub struct TemplateOrigin<'db> {
    resolved_template_name: TemplateName<'db>,
    template_file: File,
}

impl<'db> TemplateOrigin<'db> {
    pub fn template_name(self, db: &'db dyn ProjectDb) -> TemplateName<'db> {
        self.resolved_template_name(db)
    }

    pub fn file(self, db: &'db dyn ProjectDb) -> File {
        self.template_file(db)
    }

    pub fn path_buf(self, db: &'db dyn ProjectDb) -> &'db Utf8PathBuf {
        self.file(db).path(db)
    }
}

#[salsa::tracked]
pub struct TemplateResolution<'db> {
    project: Project,
    #[tracked]
    #[returns(ref)]
    template_dirs: Vec<Utf8PathBuf>,
    #[tracked]
    status: TemplateDirStatus,
}

impl<'db> TemplateResolution<'db> {
    pub fn origins(
        self,
        db: &'db dyn ProjectDb,
    ) -> impl Iterator<Item = TemplateOrigin<'db>> + 'db {
        template_resolution_index(db, self)
            .ordered(db)
            .iter()
            .copied()
    }

    #[must_use]
    pub fn known_template_dirs(self, db: &'db dyn ProjectDb) -> Option<Vec<Utf8PathBuf>> {
        matches!(self.status(db), TemplateDirStatus::Complete)
            .then(|| self.template_dirs(db).clone())
    }

    #[must_use]
    pub fn resolve(
        self,
        db: &'db dyn ProjectDb,
        template_name: TemplateName<'db>,
    ) -> FindTemplateResult<'db> {
        let index = template_resolution_index(db, self);
        if let Some(origin) = index.first_by_template_name(db).get(&template_name) {
            return FindTemplateResult::Found(*origin);
        }

        let name = template_name.name(db);
        let tried = self
            .template_dirs(db)
            .iter()
            .filter_map(|dir| safe_join(dir, name).ok())
            .map(|path| TriedTemplateSource { path })
            .collect();

        FindTemplateResult::DoesNotExist(TemplateDoesNotExist {
            template_name,
            tried,
        })
    }
}

#[salsa::tracked]
pub fn template_resolution(db: &dyn ProjectDb, project: Project) -> TemplateResolution<'_> {
    let resolution = template_dirs(db, project);
    TemplateResolution::new(db, project, resolution.dirs.clone(), resolution.status)
}

#[salsa::tracked]
struct TemplateResolutionIndex<'db> {
    #[tracked]
    #[returns(ref)]
    ordered: Vec<TemplateOrigin<'db>>,
    #[tracked]
    #[returns(ref)]
    first_by_template_name: FxHashMap<TemplateName<'db>, TemplateOrigin<'db>>,
}

#[salsa::tracked]
fn template_resolution_index<'db>(
    db: &'db dyn ProjectDb,
    resolution: TemplateResolution<'db>,
) -> TemplateResolutionIndex<'db> {
    let project = resolution.project(db);
    let mut ordered = Vec::new();
    let mut first_by_template_name = FxHashMap::default();

    for template in project_template_files(db, project).iter() {
        let template_name = TemplateName::new(db, template.name().to_string());
        let origin = TemplateOrigin::new(db, template_name, template.file());

        first_by_template_name
            .entry(template_name)
            .or_insert(origin);
        ordered.push(origin);
    }

    tracing::debug!("Discovered {} total template origins", ordered.len());

    TemplateResolutionIndex::new(db, ordered, first_by_template_name)
}

#[derive(Clone, PartialEq)]
pub enum FindTemplateResult<'db> {
    Found(TemplateOrigin<'db>),
    DoesNotExist(TemplateDoesNotExist<'db>),
}

#[derive(Clone, PartialEq)]
pub struct TemplateDoesNotExist<'db> {
    pub template_name: TemplateName<'db>,
    pub tried: Vec<TriedTemplateSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriedTemplateSource {
    pub path: Utf8PathBuf,
}

/// First-party template files in Django loader precedence order.
///
/// Duplicate template names are kept because shadowed templates can still be
/// opened, inspected, and used as reference sources.
#[derive(Clone, Default, PartialEq, Eq)]
struct ProjectTemplateFiles(Vec<ProjectTemplateFile>);

impl ProjectTemplateFiles {
    fn from_ordered_paths(db: &dyn ProjectDb, templates: Vec<(String, Utf8PathBuf)>) -> Self {
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

    fn iter(&self) -> impl Iterator<Item = &ProjectTemplateFile> {
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
struct ProjectTemplateFile {
    name: String,
    path: Utf8PathBuf,
    file: File,
}

impl ProjectTemplateFile {
    fn new(name: String, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    #[must_use]
    fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    fn file(&self) -> File {
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
fn project_template_files(db: &dyn ProjectDb, project: Project) -> ProjectTemplateFiles {
    // Freshness boundary: template discovery re-runs when any search-path root
    // revision is bumped during Django Discovery. Template dirs that live
    // outside every registered root are still re-walked then, because this query
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

    let resolution = template_dirs(db, project);
    let mut templates = Vec::new();
    let walk_options = WalkOptions::unrestricted();

    for dir in &resolution.dirs {
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
