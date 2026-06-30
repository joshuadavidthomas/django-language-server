use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

use crate::python::PythonModuleName;

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    name: PythonModuleName,
    path: Utf8PathBuf,
    file: File,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PythonPackage {
    name: PythonModuleName,
    dir: Utf8PathBuf,
    init_file: Option<File>,
}

impl PythonModule {
    pub(crate) fn new(name: PythonModuleName, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    #[must_use]
    pub fn name(&self) -> &PythonModuleName {
        &self.name
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PythonPackage {
    pub(crate) fn new(name: PythonModuleName, dir: Utf8PathBuf, init_file: Option<File>) -> Self {
        Self {
            name,
            dir,
            init_file,
        }
    }

    pub(crate) fn name(&self) -> &PythonModuleName {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Utf8Path {
        &self.dir
    }
}
