mod extraction;
mod paths;
mod sources;
mod types;

use djls_source::File;
pub use extraction::extract_settings;
pub(crate) use sources::installed_app_package_module;
pub(crate) use sources::module_file;
pub(crate) use sources::package_dir;
pub(crate) use sources::settings_source_files;
pub use types::DjangoSettings;
pub use types::InstalledAppsSetting;
pub use types::SettingsSource;
pub use types::SettingsSourceResolver;
pub use types::SettingsStarImport;
pub use types::StaticKnowledge;
pub use types::TemplateBackend;
pub use types::TemplateDirPath;
pub use types::TemplateSettings;

use crate::db::Db as ProjectDb;
use crate::project::Project;

#[salsa::tracked]
pub fn settings_module_file(db: &dyn ProjectDb, project: Project) -> Option<File> {
    let django_settings_module = project.django_settings_module(db).as_deref()?;
    module_file(db, project, django_settings_module)
}

#[salsa::tracked(returns(ref))]
pub fn django_settings(db: &dyn ProjectDb, project: Project) -> DjangoSettings {
    let Some(file) = settings_module_file(db, project) else {
        return DjangoSettings::default();
    };

    sources::django_settings_from_file(db, project, file)
}
