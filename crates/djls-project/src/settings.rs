mod extraction;
mod sources;
pub(crate) mod types;

use djls_source::File;
pub(crate) use sources::DjangoSettingsSources;
pub(crate) use sources::settings_sources;
pub(crate) use types::DjangoSettings;
pub(crate) use types::EvaluatedPath;
pub(crate) use types::TemplateContextProcessorPath;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;

fn settings_module(db: &dyn ProjectDb, project: Project) -> Option<PythonModule> {
    let django_settings_module = project.django_settings_module(db).as_ref()?.clone();
    PythonModule::resolve(db, project, django_settings_module)
}

#[salsa::tracked]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    settings_module(db, project).map(|module| module.file())
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
