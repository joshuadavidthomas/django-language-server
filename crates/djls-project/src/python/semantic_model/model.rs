use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use rustc_hash::FxHashMap;
use serde::Serialize;

use super::bindings::PythonBinding;
use super::bindings::PythonBindings;
use super::mutations::PythonMutation;
use super::mutations::PythonMutations;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ParseStatus {
    #[default]
    Parsed,
    Unparseable,
}

impl ParseStatus {
    pub(super) const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unparseable, _) | (_, Self::Unparseable) => Self::Unparseable,
            (Self::Parsed, Self::Parsed) => Self::Parsed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSemanticModel {
    pub(super) bindings: PythonBindings,
    pub(super) files_read: Vec<File>,
    pub(super) source_paths: FxHashMap<File, Utf8PathBuf>,
    pub(super) read_failures: Vec<(File, Utf8PathBuf)>,
    pub(super) mutations: PythonMutations,
    pub(super) status: ParseStatus,
}

impl PythonSemanticModel {
    pub(crate) fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    pub(crate) fn files_read(&self) -> &[File] {
        &self.files_read
    }

    pub(crate) fn mutations(&self) -> &[PythonMutation] {
        self.mutations.as_slice()
    }

    pub(super) const fn mutation_set(&self) -> &PythonMutations {
        &self.mutations
    }

    pub(crate) fn read_failures(&self) -> &[(File, Utf8PathBuf)] {
        &self.read_failures
    }

    pub(crate) fn source_path(&self, file: File) -> Option<&Utf8Path> {
        self.source_paths.get(&file).map(Utf8PathBuf::as_path)
    }

    pub(crate) fn parse_status(&self) -> ParseStatus {
        self.status
    }
}
