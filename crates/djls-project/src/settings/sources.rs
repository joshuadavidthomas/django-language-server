use std::collections::BTreeSet;

use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::ProjectImportSourceResolver;
use crate::python::PythonSemanticModel;
use crate::settings::DjangoSettings;
use crate::settings::extraction::extract_settings;

pub(super) fn django_settings_from_file(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> DjangoSettings {
    let mut resolver = ProjectImportSourceResolver::tracked(db, project);
    let Some(source) = resolver.read_source(file) else {
        return DjangoSettings::default();
    };
    extract_settings(&source, &mut resolver)
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DjangoSettingsSources(Vec<File>);

impl DjangoSettingsSources {
    fn from_files(db: &dyn ProjectDb, files: impl IntoIterator<Item = File>) -> Self {
        let mut seen = BTreeSet::new();
        let mut deduped = Vec::new();
        for file in files {
            if seen.insert(file.path(db).to_path_buf()) {
                deduped.push(file);
            }
        }

        Self(deduped)
    }

    pub(crate) fn files(&self) -> &[File] {
        &self.0
    }

    pub(crate) fn root(&self) -> Option<File> {
        self.0.first().copied()
    }
}

pub(crate) fn settings_sources(db: &dyn ProjectDb, project: Project) -> DjangoSettingsSources {
    let Some(file) = crate::settings::settings_module_file(db, project) else {
        return DjangoSettingsSources::from_files(db, []);
    };

    // The Django Discovery bump set must cover the same settings source graph
    // the extractor would read against current disk content.
    let mut resolver = ProjectImportSourceResolver::discovery(db, project);
    let Some(source) = resolver.read_source(file) else {
        return DjangoSettingsSources::from_files(db, [file]);
    };
    let model = PythonSemanticModel::analyze(&source, &mut resolver);

    DjangoSettingsSources::from_files(db, model.files_read().iter().copied())
}
