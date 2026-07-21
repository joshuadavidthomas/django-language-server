use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonSourceModule;
use crate::python::evaluation::python_import_trace;
use crate::python::evaluation::python_module_facts;
use crate::settings::DjangoSettings;
use crate::settings::extraction::settings_from_values;
use crate::settings::settings_module;

pub(super) fn django_settings_from_module(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
) -> DjangoSettings {
    let file = module.file();
    match python_module_facts(db, project, module) {
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

#[salsa::tracked(returns(clone))]
pub(crate) fn settings_sources(db: &dyn ProjectDb, project: Project) -> DjangoSettingsSources {
    let Some(module) = settings_module(db, project) else {
        return DjangoSettingsSources(Vec::new());
    };

    DjangoSettingsSources(python_import_trace(db, project, module).files().collect())
}
