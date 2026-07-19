use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::evaluation::python_module_dependencies;
use crate::python::evaluation::python_module_values;
use crate::settings::DjangoSettings;
use crate::settings::extraction::settings_from_values;
use crate::settings::settings_module;

pub(super) fn django_settings_from_module(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> DjangoSettings {
    let file = module.file();
    match python_module_values(db, project, module) {
        Ok(values) => settings_from_values(db, file, values),
        Err(_) => DjangoSettings::unreadable(),
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DjangoSettingsSources(Vec<File>);

impl DjangoSettingsSources {
    pub(crate) fn files(&self) -> &[File] {
        &self.0
    }

    pub(crate) fn root(&self) -> Option<File> {
        self.0.first().copied()
    }
}

#[salsa::tracked]
pub(crate) fn settings_sources(db: &dyn ProjectDb, project: Project) -> DjangoSettingsSources {
    let Some(module) = settings_module(db, project) else {
        return DjangoSettingsSources(Vec::new());
    };

    DjangoSettingsSources(
        python_module_dependencies(db, project, module)
            .files
            .clone()
            .into_iter()
            .collect(),
    )
}
