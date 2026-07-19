use std::collections::BTreeMap;

use djls_source::File;
use djls_source::Span;
use ruff_python_ast::Alias;
use ruff_python_ast::Identifier;
use ruff_python_ast::Stmt;
use thiserror::Error;

use crate::ast::AliasExt;
use crate::ast::RangedExt;
use crate::python::PythonModuleName;
use crate::python::RecoveredPythonModule;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportBindings(BTreeMap<String, ImportBinding>);

impl ImportBindings {
    pub(crate) fn resolve_qualified_path<'a>(
        &self,
        path: impl IntoIterator<Item = &'a str>,
    ) -> Result<PythonModuleName, ImportPathResolutionError> {
        let mut parts = path.into_iter();
        let Some(root) = parts.next() else {
            return Err(ImportPathResolutionError::EmptyPath);
        };
        let Some(binding) = self.0.get(root) else {
            return Err(ImportPathResolutionError::MissingBinding(root.to_string()));
        };

        let tail: Vec<&str> = parts.collect();
        let target = if tail.is_empty() {
            binding.target.as_str().to_string()
        } else {
            format!("{}.{}", binding.target.as_str(), tail.join("."))
        };

        PythonModuleName::parse(&target)
            .map_err(|_| ImportPathResolutionError::InvalidTarget(target))
    }

    fn from_statements(
        stmts: &[Stmt],
        module_name: &PythonModuleName,
        module_kind: ModuleKind,
    ) -> Self {
        let mut bindings = Self::default();
        for stmt in stmts {
            match stmt {
                Stmt::Import(import) => bindings.record_import(&import.names),
                Stmt::ImportFrom(import_from) => bindings.record_import_from(
                    module_name,
                    module_kind,
                    import_from.level,
                    import_from.module.as_ref().map(Identifier::as_str),
                    &import_from.names,
                ),
                _ => {}
            }
        }

        bindings
    }

    fn record_import(&mut self, aliases: &[Alias]) {
        for alias in aliases {
            let imported_name = alias.name.as_str();
            let (local_name, target, binding_range) = if let Some(asname) = &alias.asname {
                (
                    asname.as_str().to_string(),
                    imported_name.to_string(),
                    asname.span(),
                )
            } else {
                let local_name = first_import_segment(imported_name);
                (
                    local_name.to_string(),
                    local_name.to_string(),
                    alias.unaliased_binding_span(local_name),
                )
            };

            let Ok(target) = PythonModuleName::parse(&target) else {
                continue;
            };
            self.0.insert(
                local_name,
                ImportBinding {
                    target,
                    binding_range,
                },
            );
        }
    }

    fn record_import_from(
        &mut self,
        module_name: &PythonModuleName,
        module_kind: ModuleKind,
        level: u32,
        imported_from: Option<&str>,
        aliases: &[Alias],
    ) {
        let Some(base) = import_from_base(module_name, module_kind, level, imported_from) else {
            return;
        };

        for alias in aliases {
            let imported_name = alias.name.as_str();
            if imported_name == "*" {
                continue;
            }

            let (local_name, binding_range) = if let Some(asname) = &alias.asname {
                (asname.as_str().to_string(), asname.span())
            } else {
                (imported_name.to_string(), alias.name.span())
            };
            let target = if base.is_empty() {
                imported_name.to_string()
            } else {
                format!("{base}.{imported_name}")
            };
            let Ok(target) = PythonModuleName::parse(&target) else {
                continue;
            };

            self.0.insert(
                local_name,
                ImportBinding {
                    target,
                    binding_range,
                },
            );
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportBinding {
    target: PythonModuleName,
    binding_range: Span,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum ImportPathResolutionError {
    #[error("import path is empty")]
    EmptyPath,
    #[error("no import binding exists for `{0}`")]
    MissingBinding(String),
    #[error("resolved import target `{0}` is not a valid module name")]
    InvalidTarget(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModuleKind {
    Module,
    PackageInit,
}

// Salsa tracked-query keys are by-value; `module_name` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(returns(ref))]
pub(crate) fn import_bindings(
    db: &dyn djls_source::Db,
    file: File,
    module_name: PythonModuleName,
) -> ImportBindings {
    let Ok(Some(module)) = RecoveredPythonModule::from_file(db, file) else {
        return ImportBindings::default();
    };

    let module_kind = if file.path(db).file_name() == Some("__init__.py") {
        ModuleKind::PackageInit
    } else {
        ModuleKind::Module
    };
    ImportBindings::from_statements(module.body(db), &module_name, module_kind)
}

#[cfg(test)]
fn extract_import_bindings_for_source(
    source: &str,
    module_name: &PythonModuleName,
    module_kind: ModuleKind,
) -> ImportBindings {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ImportBindings::default();
    };

    let module = parsed.into_syntax();
    ImportBindings::from_statements(&module.body, module_name, module_kind)
}

pub(super) fn first_import_segment(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

fn import_from_base(
    module_name: &PythonModuleName,
    module_kind: ModuleKind,
    level: u32,
    imported_from: Option<&str>,
) -> Option<String> {
    if level == 0 {
        return imported_from.map(str::to_string);
    }

    let mut parts: Vec<&str> = module_name.as_str().split('.').collect();
    if module_kind == ModuleKind::Module {
        parts.pop();
    }

    if level as usize > parts.len() {
        return None;
    }

    for _ in 1..level {
        parts.pop();
    }

    if let Some(imported_from) = imported_from {
        parts.extend(imported_from.split('.').filter(|part| !part.is_empty()));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

#[cfg(test)]
mod tests {
    use djls_testing::TestDatabase;

    use super::*;
    use crate::db::Db as ProjectDb;
    use crate::project::Project;

    fn bindings(source: &str, module_name: &str, module_kind: ModuleKind) -> ImportBindings {
        let module_name = PythonModuleName::parse(module_name).unwrap();
        extract_import_bindings_for_source(source, &module_name, module_kind)
    }

    fn binding<'a>(table: &'a ImportBindings, name: &str) -> &'a ImportBinding {
        table.0.get(name).expect("binding should exist")
    }

    #[test]
    fn plain_import_binds_top_level_module() {
        let table = bindings("import os\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "os").target.as_str(), "os");
        assert_eq!(binding(&table, "os").binding_range, Span::new(7, 2));
    }

    #[test]
    fn aliased_import_binds_alias_to_full_target() {
        let table = bindings("import a.b as c\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "c").target.as_str(), "a.b");
    }

    #[test]
    fn submodule_import_binds_only_top_level_module() {
        let table = bindings("import os.path\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(table.0.len(), 1);
        assert_eq!(binding(&table, "os").target.as_str(), "os");
    }

    #[test]
    fn from_import_binds_imported_name_to_qualified_target() {
        let table = bindings("from m import x\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "x").target.as_str(), "m.x");
    }

    #[test]
    fn aliased_from_import_binds_alias_to_qualified_target() {
        let table = bindings("from m import x as y\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "y").target.as_str(), "m.x");
    }

    #[test]
    fn relative_import_level_one_uses_containing_package() {
        let table = bindings("from . import x\n", "pkg.sub.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "x").target.as_str(), "pkg.sub.x");
    }

    #[test]
    fn relative_import_level_two_strips_one_package_segment() {
        let table = bindings("from ..m import y\n", "pkg.sub.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "y").target.as_str(), "pkg.m.y");
    }

    #[test]
    fn relative_import_from_package_init_uses_package_as_base() {
        let table = bindings("from . import x\n", "pkg.sub", ModuleKind::PackageInit);

        assert_eq!(binding(&table, "x").target.as_str(), "pkg.sub.x");
    }

    #[test]
    fn relative_import_level_overflow_records_no_binding() {
        let table = bindings("from ..m import y\n", "pkg.mod", ModuleKind::Module);

        assert!(table.0.is_empty());
    }

    #[test]
    fn star_import_records_no_binding() {
        let table = bindings("from m import *\n", "pkg.mod", ModuleKind::Module);

        assert!(table.0.is_empty());
    }

    #[test]
    fn duplicate_bindings_shadow_with_last_assignment_wins() {
        let table = bindings(
            "from a import x\nfrom b import x\n",
            "pkg.mod",
            ModuleKind::Module,
        );

        assert_eq!(binding(&table, "x").target.as_str(), "b.x");
    }

    #[test]
    fn qualified_paths_resolve_from_recorded_bindings() {
        let table = bindings("import package as alias\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(
            table
                .resolve_qualified_path(["alias", "nested"])
                .expect("binding should resolve")
                .as_str(),
            "package.nested"
        );
    }

    #[test]
    fn qualified_path_resolution_errors_are_specific() {
        let table = ImportBindings::default();

        assert_eq!(
            table.resolve_qualified_path([]),
            Err(ImportPathResolutionError::EmptyPath)
        );
        let missing = table
            .resolve_qualified_path(["missing"])
            .expect_err("missing binding should fail");
        assert_eq!(
            missing,
            ImportPathResolutionError::MissingBinding("missing".to_string())
        );
        assert_eq!(
            missing.to_string(),
            "no import binding exists for `missing`"
        );
    }

    #[test]
    fn import_bindings_query_reads_python_file_source() {
        let db = TestDatabase::new();
        db.add_file("/project/pkg/mod.py", "import os\n");
        let file = db.file(camino::Utf8Path::new("/project/pkg/mod.py"));
        let table = import_bindings(&db, file, PythonModuleName::parse("pkg.mod").unwrap());

        assert_eq!(binding(table, "os").target.as_str(), "os");
    }

    // djls-testing's ProjectDb impl is for the dependency-graph copy of this
    // crate, not this test build (dev-dependency cycle), so the trait must be
    // bridged here.
    #[salsa::db]
    impl ProjectDb for TestDatabase {
        fn project(&self) -> Option<Project> {
            None
        }
    }
}
