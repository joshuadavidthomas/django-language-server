use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleValuesOutcome;
use crate::python::python_module_dependencies;
use crate::python::python_module_values;
use crate::settings::DjangoSettings;
use crate::settings::extraction::settings_from_values;

pub(super) fn django_settings_from_file(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> DjangoSettings {
    match python_module_values(db, project, file) {
        PythonModuleValuesOutcome::Readable(values) => settings_from_values(db, file, values),
        PythonModuleValuesOutcome::Unreadable(_) => DjangoSettings::unreadable(),
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
    let Some(file) = crate::settings::settings_module_file(db, project) else {
        return DjangoSettingsSources(Vec::new());
    };

    DjangoSettingsSources(python_module_dependencies(db, project, file).files.clone())
}
