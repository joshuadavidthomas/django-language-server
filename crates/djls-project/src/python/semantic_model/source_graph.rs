mod import_reachability;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use ruff_python_ast as ast;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;

use super::model::ParseStatus;
use crate::python::PythonImportResolver;
use crate::python::PythonSource;

#[derive(Debug, Clone)]
pub(super) struct PythonSourceGraph {
    pub(super) root: File,
    pub(super) modules: FxHashMap<File, PythonModuleRecord>,
    pub(super) imports: FxHashMap<File, Vec<PythonImportEdge>>,
}

impl PythonSourceGraph {
    pub(super) fn collect(source: &PythonSource, resolver: &mut dyn PythonImportResolver) -> Self {
        let mut graph = Self::from_root_source(source);
        import_reachability::collect_imports(&mut graph, resolver);
        graph
    }

    fn from_root_source(source: &PythonSource) -> Self {
        let root = source.file();
        let mut modules = FxHashMap::default();
        modules.insert(root, PythonModuleRecord::parse(source.clone()));

        Self {
            root,
            modules,
            imports: FxHashMap::default(),
        }
    }

    pub(super) fn root(&self) -> File {
        self.root
    }

    pub(super) fn module(&self, file: File) -> Option<&PythonModuleRecord> {
        self.modules.get(&file)
    }

    pub(super) fn source(&self, file: File) -> Option<&PythonSource> {
        self.module(file)?.source()
    }

    pub(super) fn imports(&self, file: File) -> &[PythonImportEdge] {
        self.imports.get(&file).map_or(&[], Vec::as_slice)
    }

    pub(super) fn import_edge(
        &self,
        file: File,
        import: &ast::StmtImportFrom,
    ) -> Option<&PythonImportEdge> {
        let source = self.source(file)?;
        let key = PythonImportKey::from_import(source.path(), import);
        self.imports(file).iter().find(|edge| edge.import() == &key)
    }
}

#[derive(Debug, Clone)]
pub(super) enum PythonModuleRecord {
    Parsed {
        source: PythonSource,
        module: Box<ast::ModModule>,
    },
    Unparseable {
        source: PythonSource,
    },
    ReadFailed {
        file: File,
        path: Utf8PathBuf,
    },
}

impl PythonModuleRecord {
    pub(super) fn parse(source: PythonSource) -> Self {
        match parse_module(source.source()) {
            Ok(parsed) => Self::Parsed {
                source,
                module: Box::new(parsed.into_syntax()),
            },
            Err(_) => Self::Unparseable { source },
        }
    }

    pub(super) fn file(&self) -> File {
        match self {
            Self::Parsed { source, .. } | Self::Unparseable { source } => source.file(),
            Self::ReadFailed { file, .. } => *file,
        }
    }

    pub(super) fn path(&self) -> &Utf8Path {
        match self {
            Self::Parsed { source, .. } | Self::Unparseable { source } => source.path(),
            Self::ReadFailed { path, .. } => path,
        }
    }

    pub(super) fn parse_status(&self) -> Option<ParseStatus> {
        match self {
            Self::Parsed { .. } => Some(ParseStatus::Parsed),
            Self::Unparseable { .. } => Some(ParseStatus::Unparseable),
            Self::ReadFailed { .. } => None,
        }
    }

    fn source(&self) -> Option<&PythonSource> {
        match self {
            Self::Parsed { source, .. } | Self::Unparseable { source } => Some(source),
            Self::ReadFailed { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PythonImportKey {
    importer: Utf8PathBuf,
    level: u32,
    module: Option<String>,
    names: Vec<PythonImportedName>,
}

impl PythonImportKey {
    fn from_import(importer: &Utf8Path, import: &ast::StmtImportFrom) -> Self {
        Self {
            importer: importer.to_path_buf(),
            level: import.level,
            module: import
                .module
                .as_ref()
                .map(ruff_python_ast::Identifier::to_string),
            names: import
                .names
                .iter()
                .map(|alias| PythonImportedName {
                    name: alias.name.to_string(),
                    asname: alias.asname.as_ref().map(ToString::to_string),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PythonImportedName {
    name: String,
    asname: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PythonImportKind {
    Star,
    Named,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PythonImportEdge {
    Resolved {
        import: PythonImportKey,
        file: File,
        kind: PythonImportKind,
    },
    Unresolved {
        import: PythonImportKey,
        kind: PythonImportKind,
    },
    SkippedExternal {
        import: PythonImportKey,
        kind: PythonImportKind,
    },
    ReadFailed {
        import: PythonImportKey,
        file: File,
        path: Utf8PathBuf,
        kind: PythonImportKind,
    },
}

impl PythonImportEdge {
    pub(super) const fn import(&self) -> &PythonImportKey {
        match self {
            Self::Resolved { import, .. }
            | Self::Unresolved { import, .. }
            | Self::SkippedExternal { import, .. }
            | Self::ReadFailed { import, .. } => import,
        }
    }

    pub(super) const fn resolved_file(&self) -> Option<File> {
        match self {
            Self::Resolved { file, .. } => Some(*file),
            Self::Unresolved { .. } | Self::SkippedExternal { .. } | Self::ReadFailed { .. } => {
                None
            }
        }
    }
}
