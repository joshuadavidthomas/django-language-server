mod extraction;
mod sources;
pub(crate) mod types;

use djls_source::File;
pub(crate) use sources::DjangoSettingsSources;
pub(crate) use sources::settings_sources;
pub(crate) use types::DjangoSettings;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonSourceModule;

fn settings_module(db: &dyn ProjectDb, project: Project) -> Option<PythonSourceModule> {
    let django_settings_module = project.django_settings_module(db).as_ref()?.clone();
    PythonSourceModule::resolve(db, project, django_settings_module)
}

#[salsa::tracked(returns(copy))]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let module = settings_module(db, project);
    // Every settings consumer checks this query first, so it reports a missing
    // module once per recomputation.
    if module.is_none()
        && let Some(module_name) = project.django_settings_module(db)
    {
        tracing::warn!(
            "Django settings module not found on the Python path; installed apps and template settings are unknown"
        );
        tracing::debug!(
            module = module_name.as_str(),
            "Unresolved Django settings module"
        );
    }
    module.map(|module| module.file())
}

#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    let Some(module) = settings_module(db, project) else {
        return if project.django_settings_module(db).is_some() {
            DjangoSettings::unreadable()
        } else {
            DjangoSettings::default()
        };
    };

    sources::django_settings_from_module(db, project, module)
}
