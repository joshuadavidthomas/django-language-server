mod extraction;
mod sources;
pub(crate) mod types;

use djls_source::File;
pub(crate) use extraction::extract_settings;
pub(crate) use sources::DjangoSettingsSources;
pub(crate) use sources::settings_sources;
pub(crate) use types::DjangoSettings;
pub(crate) use types::SettingsSource;
pub(crate) use types::SettingsSourceResolver;
pub(crate) use types::SettingsStarImport;
pub(crate) use types::StaticKnowledge;
pub(crate) use types::TemplateDirPath;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;

#[salsa::tracked]
pub(crate) fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let django_settings_module = project.django_settings_module(db).as_ref()?.clone();
    PythonModule::resolve(db, project, django_settings_module).map(|module| module.file())
}

#[salsa::tracked(returns(ref))]
pub(crate) fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    let Some(file) = settings_module_file(db, project) else {
        return DjangoSettings::default();
    };

    sources::django_settings_from_file(db, project, file)
}
