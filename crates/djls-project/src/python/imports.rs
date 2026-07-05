use std::collections::BTreeMap;

use djls_source::File;
use djls_source::Span;
use ruff_python_ast::Alias;
use ruff_python_ast::Stmt;

use crate::ast::AliasExt;
use crate::ast::IdentifierExt;
use crate::python::PythonModuleName;
use crate::python::parse_python_module;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportTable {
    bindings: BTreeMap<String, ImportBinding>,
}

impl ImportTable {
    pub(crate) fn resolve_qualified_path<'a>(
        &self,
        path: impl IntoIterator<Item = &'a str>,
    ) -> Result<PythonModuleName, ImportPathResolutionError> {
        let mut parts = path.into_iter();
        let Some(root) = parts.next() else {
            return Err(ImportPathResolutionError::EmptyPath);
        };
        let Some(binding) = self.bindings.get(root) else {
            return Err(ImportPathResolutionError::MissingBinding {
                binding: root.to_string(),
            });
        };

        let tail: Vec<&str> = parts.collect();
        let target = if tail.is_empty() {
            binding.target.as_str().to_string()
        } else {
            format!("{}.{}", binding.target.as_str(), tail.join("."))
        };

        PythonModuleName::parse(&target)
            .map_err(|_| ImportPathResolutionError::InvalidTarget { target })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportBinding {
    target: PythonModuleName,
    binding_range: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImportPathResolutionError {
    EmptyPath,
    MissingBinding { binding: String },
    InvalidTarget { target: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    Module,
    PackageInit,
}

// Salsa tracked-query keys are by-value; `module_name` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(returns(ref))]
pub(crate) fn import_table(
    db: &dyn djls_source::Db,
    file: File,
    module_name: PythonModuleName,
) -> ImportTable {
    let Some(parsed) = parse_python_module(db, file) else {
        return ImportTable::default();
    };

    let module_kind = if file.path(db).file_name() == Some("__init__.py") {
        ModuleKind::PackageInit
    } else {
        ModuleKind::Module
    };
    extract_import_table_impl(parsed.body(db), &module_name, module_kind)
}

#[cfg(test)]
pub(crate) fn extract_import_table_for_source(
    source: &str,
    module_name: &PythonModuleName,
    module_kind: ModuleKind,
) -> ImportTable {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ImportTable::default();
    };

    let module = parsed.into_syntax();
    extract_import_table_impl(&module.body, module_name, module_kind)
}

fn extract_import_table_impl(
    stmts: &[Stmt],
    module_name: &PythonModuleName,
    module_kind: ModuleKind,
) -> ImportTable {
    let mut table = ImportTable::default();
    for stmt in stmts {
        match stmt {
            Stmt::Import(import) => record_import(&mut table, &import.names),
            Stmt::ImportFrom(import_from) => record_import_from(
                &mut table,
                module_name,
                module_kind,
                import_from.level,
                import_from
                    .module
                    .as_ref()
                    .map(ruff_python_ast::Identifier::as_str),
                &import_from.names,
            ),
            _ => {}
        }
    }

    table
}

fn record_import(table: &mut ImportTable, aliases: &[Alias]) {
    for alias in aliases {
        let imported_name = alias.name.as_str();
        let (local_name, target, binding_range) = if let Some(asname) = &alias.asname {
            (
                asname.as_str().to_string(),
                imported_name.to_string(),
                asname.span(),
            )
        } else {
            let local_name = imported_name.split('.').next().unwrap_or(imported_name);
            (
                local_name.to_string(),
                local_name.to_string(),
                alias.unaliased_binding_span(local_name),
            )
        };

        let Ok(target) = PythonModuleName::parse(&target) else {
            continue;
        };
        table.bindings.insert(
            local_name,
            ImportBinding {
                target,
                binding_range,
            },
        );
    }
}

fn record_import_from(
    table: &mut ImportTable,
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

        table.bindings.insert(
            local_name,
            ImportBinding {
                target,
                binding_range,
            },
        );
    }
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

    fn table(source: &str, module_name: &str, module_kind: ModuleKind) -> ImportTable {
        let module_name = PythonModuleName::parse(module_name).unwrap();
        extract_import_table_for_source(source, &module_name, module_kind)
    }

    fn binding<'a>(table: &'a ImportTable, name: &str) -> &'a ImportBinding {
        table.bindings.get(name).expect("binding should exist")
    }

    #[test]
    fn plain_import_binds_top_level_module() {
        let table = table("import os\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "os").target.as_str(), "os");
        assert_eq!(binding(&table, "os").binding_range, Span::new(7, 2));
    }

    #[test]
    fn aliased_import_binds_alias_to_full_target() {
        let table = table("import a.b as c\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "c").target.as_str(), "a.b");
    }

    #[test]
    fn submodule_import_binds_only_top_level_module() {
        let table = table("import os.path\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(table.bindings.len(), 1);
        assert_eq!(binding(&table, "os").target.as_str(), "os");
    }

    #[test]
    fn from_import_binds_imported_name_to_qualified_target() {
        let table = table("from m import x\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "x").target.as_str(), "m.x");
    }

    #[test]
    fn aliased_from_import_binds_alias_to_qualified_target() {
        let table = table("from m import x as y\n", "pkg.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "y").target.as_str(), "m.x");
    }

    #[test]
    fn relative_import_level_one_uses_containing_package() {
        let table = table("from . import x\n", "pkg.sub.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "x").target.as_str(), "pkg.sub.x");
    }

    #[test]
    fn relative_import_level_two_strips_one_package_segment() {
        let table = table("from ..m import y\n", "pkg.sub.mod", ModuleKind::Module);

        assert_eq!(binding(&table, "y").target.as_str(), "pkg.m.y");
    }

    #[test]
    fn relative_import_from_package_init_uses_package_as_base() {
        let table = table("from . import x\n", "pkg.sub", ModuleKind::PackageInit);

        assert_eq!(binding(&table, "x").target.as_str(), "pkg.sub.x");
    }

    #[test]
    fn relative_import_level_overflow_records_no_binding() {
        let table = table("from ..m import y\n", "pkg.mod", ModuleKind::Module);

        assert!(table.bindings.is_empty());
    }

    #[test]
    fn star_import_records_no_binding() {
        let table = table("from m import *\n", "pkg.mod", ModuleKind::Module);

        assert!(table.bindings.is_empty());
    }

    #[test]
    fn duplicate_bindings_shadow_with_last_assignment_wins() {
        let table = table(
            "from a import x\nfrom b import x\n",
            "pkg.mod",
            ModuleKind::Module,
        );

        assert_eq!(binding(&table, "x").target.as_str(), "b.x");
    }

    #[test]
    fn import_table_query_reads_python_file_source() {
        let db = TestDatabase::new();
        db.add_file("/project/pkg/mod.py", "import os\n");
        let file = db.get_or_create_file(camino::Utf8Path::new("/project/pkg/mod.py"));
        let table = import_table(&db, file, PythonModuleName::parse("pkg.mod").unwrap());

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
