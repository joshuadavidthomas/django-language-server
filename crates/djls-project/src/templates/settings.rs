use camino::Utf8PathBuf;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::settings::StaticKnowledge;
use crate::settings::TemplateDirPath;
use crate::settings::django_settings;
use crate::settings::installed_app_package_module;
use crate::settings::package_dir;

#[salsa::tracked(returns(ref))]
pub fn template_dirs(db: &dyn ProjectDb, project: Project) -> (Vec<Utf8PathBuf>, StaticKnowledge) {
    project.touch_search_path_roots(db);

    let settings = django_settings(db, project);
    let mut dirs = Vec::new();
    let mut knowledge = settings.templates.knowledge;
    let backend_count = settings.templates.backends.len();

    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        knowledge = knowledge.weakened_by(backend.knowledge);

        for dir in &backend.dirs {
            match dir {
                TemplateDirPath::Resolved(path) => dirs.push(path.clone()),
                TemplateDirPath::Unknown => knowledge = knowledge.demoted_to_partial(),
            }
        }

        if backend.app_dirs == Some(true) {
            knowledge = knowledge.weakened_by(settings.installed_apps.knowledge);
            for app in &settings.installed_apps.values {
                let Some(app_dir) = package_dir(db, project, installed_app_package_module(app)) else {
                    knowledge = knowledge.demoted_to_partial();
                    continue;
                };

                let templates_dir = app_dir.join("templates");
                if db.path_is_dir(&templates_dir) {
                    dirs.push(templates_dir);
                }
            }
        }
    }

    (dirs, knowledge)
}
