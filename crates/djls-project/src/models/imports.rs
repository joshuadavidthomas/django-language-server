use std::collections::BTreeMap;
use std::collections::BTreeSet;

use thiserror::Error;

use crate::python::PythonModuleName;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::import::ModuleKind;
use crate::python::module::relative_import_source;

/// Occurrence-local symbolic import state for Django model resolution.
///
/// This maps a local name to the qualified module path it refers to, as of a
/// particular point in a module's source order. It is maintained by the model
/// extraction scanner, which adds/replaces entries at import statements and
/// invalidates roots at writes and control-flow boundaries. It deliberately
/// approximates: external dotted targets are allowed, star imports contribute
/// no known alias, and module bodies are never evaluated. It is not the general
/// Python import model; it exists only to qualify Model symbolic references.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelImportAliases {
    aliases: BTreeMap<String, PythonModuleName>,
    shadowed: BTreeSet<String>,
}

impl ModelImportAliases {
    /// Add/replace the aliases introduced by an `import ...` statement.
    ///
    /// Each bound root is invalidated first so that an unusable spelling leaves
    /// no stale alias behind for later occurrences.
    pub(crate) fn apply_direct_import(&mut self, clauses: &[DirectImportClause<'_>]) {
        for clause in clauses {
            self.invalidate_root(clause.bound());
            if let Ok(target) = PythonModuleName::parse(clause.target()) {
                self.bind(clause.bound(), target);
            }
        }
    }

    /// Add/replace the aliases introduced by a `from ... import ...` statement.
    ///
    /// Star imports contribute no known alias and leave existing aliases in
    /// place. Each named bound root is invalidated first so that an unusable
    /// spelling or an out-of-range relative level leaves no stale alias behind.
    pub(crate) fn apply_from_import(
        &mut self,
        syntax: &FromImportSyntax<'_>,
        module_name: &PythonModuleName,
        module_kind: ModuleKind,
    ) {
        // Relative-name construction is owned by `python::module`; derive the
        // containing package from the importer identity and kind, never from
        // the dotted name alone.
        let package = match module_kind {
            ModuleKind::PackageInit => Some(module_name.clone()),
            ModuleKind::Module => module_name.parent(),
        };
        let base = relative_import_source(package.as_ref(), syntax.level(), syntax.module());

        for member in syntax.named_members() {
            self.invalidate_root(member.bound());
            let Some(base) = base.as_deref() else {
                continue;
            };
            let target = if base.is_empty() {
                member.imported().to_string()
            } else {
                format!("{base}.{}", member.imported())
            };
            if let Ok(target) = PythonModuleName::parse(&target) {
                self.bind(member.bound(), target);
            }
        }
    }

    /// Remove the alias bound to `root` and record a non-class shadowing write.
    pub(crate) fn invalidate_root(&mut self, root: &str) {
        self.aliases.remove(root);
        self.shadowed.insert(root.to_string());
    }

    /// Record a local class binding. It is no longer an import alias, but later
    /// bare Model references may resolve it through the same-module graph.
    pub(crate) fn bind_local_class(&mut self, root: &str) {
        self.aliases.remove(root);
        self.shadowed.remove(root);
    }

    fn bind(&mut self, root: &str, target: PythonModuleName) {
        self.shadowed.remove(root);
        self.aliases.insert(root.to_string(), target);
    }

    /// Resolve a dotted source spelling into the qualified module path the
    /// current aliases imply.
    pub(crate) fn resolve_qualified_path(
        &self,
        root: &str,
        tail: &[String],
    ) -> Result<PythonModuleName, ModelImportPathResolutionError> {
        let Some(target) = self.aliases.get(root) else {
            return Err(if self.shadowed.contains(root) {
                ModelImportPathResolutionError::ShadowedBinding
            } else {
                ModelImportPathResolutionError::MissingBinding
            });
        };

        let resolved = if tail.is_empty() {
            target.as_str().to_string()
        } else {
            format!("{}.{}", target.as_str(), tail.join("."))
        };

        PythonModuleName::parse(&resolved)
            .map_err(|_| ModelImportPathResolutionError::InvalidTarget(resolved))
    }

    /// Resolve a Model base/relation spelling into an occurrence-local
    /// reference: either the qualified module path it names, or the reason it
    /// cannot be symbolically qualified.
    pub(crate) fn resolve_reference(&self, root: &str, tail: &[String]) -> ModelImportReference {
        match self.resolve_qualified_path(root, tail) {
            Ok(target) => ModelImportReference::Qualified(target),
            Err(error) => ModelImportReference::Unresolved(error),
        }
    }
}

/// A resolved-at-occurrence symbolic reference for a Model base or relation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ModelImportReference {
    /// The spelling resolved to a qualified module path via the aliases in
    /// scope at the occurrence.
    Qualified(PythonModuleName),
    /// The spelling could not be symbolically qualified; the reason is
    /// preserved for downstream resolution and diagnostics.
    Unresolved(ModelImportPathResolutionError),
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Hash)]
pub(crate) enum ModelImportPathResolutionError {
    #[error("no import binding exists")]
    MissingBinding,
    #[error("the import binding was shadowed")]
    ShadowedBinding,
    #[error("resolved import target `{0}` is not a valid module name")]
    InvalidTarget(String),
}

#[cfg(test)]
mod tests {
    use djls_testing::TestDatabase;
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::db::Db as ProjectDb;
    use crate::project::Project;

    // The dev-dependency cycle gives `TestDatabase` an impl for the dependency
    // build of this crate, not this lib-test build. Keep the single bridge next
    // to the moved import tests that historically supplied it.
    #[salsa::db]
    impl ProjectDb for TestDatabase {
        fn project(&self) -> Option<Project> {
            None
        }
    }

    fn module_name(name: &str) -> PythonModuleName {
        PythonModuleName::parse(name).unwrap()
    }

    /// Build occurrence-local aliases by applying every top-level import in
    /// source order, mirroring the model extraction scanner without
    /// invalidation. Adequate for the import-spelling unit tests here.
    fn aliases(source: &str, module: &str, module_kind: ModuleKind) -> ModelImportAliases {
        let name = module_name(module);
        let parsed = parse_module(source).unwrap().into_syntax();
        let mut state = ModelImportAliases::default();
        for stmt in &parsed.body {
            match stmt {
                Stmt::Import(import) => {
                    state.apply_direct_import(&DirectImportClause::lower(import));
                }
                Stmt::ImportFrom(import) => {
                    state.apply_from_import(&FromImportSyntax::lower(import), &name, module_kind);
                }
                _ => {}
            }
        }
        state
    }

    fn target(state: &ModelImportAliases, name: &str) -> String {
        state
            .aliases
            .get(name)
            .expect("alias should exist")
            .as_str()
            .to_string()
    }

    #[test]
    fn plain_import_binds_top_level_module() {
        let state = aliases("import os\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(target(&state, "os"), "os");
    }

    #[test]
    fn aliased_import_binds_alias_to_full_target() {
        let state = aliases("import a.b as c\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(target(&state, "c"), "a.b");
    }

    #[test]
    fn submodule_import_binds_only_top_level_module() {
        let state = aliases("import os.path\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(state.aliases.len(), 1);
        assert_eq!(target(&state, "os"), "os");
    }

    #[test]
    fn from_import_binds_imported_name_to_qualified_target() {
        let state = aliases("from m import x\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(target(&state, "x"), "m.x");
    }

    #[test]
    fn aliased_from_import_binds_alias_to_qualified_target() {
        let state = aliases("from m import x as y\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(target(&state, "y"), "m.x");
    }

    #[test]
    fn relative_import_level_one_uses_containing_package() {
        let state = aliases("from . import x\n", "pkg.sub.mod", ModuleKind::Module);
        assert_eq!(target(&state, "x"), "pkg.sub.x");
    }

    #[test]
    fn relative_import_from_package_init_uses_package_as_base() {
        let state = aliases("from . import x\n", "pkg.sub", ModuleKind::PackageInit);
        assert_eq!(target(&state, "x"), "pkg.sub.x");
    }

    #[test]
    fn star_import_records_no_alias() {
        let state = aliases("from m import *\n", "pkg.mod", ModuleKind::Module);
        assert!(state.aliases.is_empty());
    }

    #[test]
    fn duplicate_aliases_shadow_with_last_assignment_wins() {
        let state = aliases(
            "from a import x\nfrom b import x\n",
            "pkg.mod",
            ModuleKind::Module,
        );
        assert_eq!(target(&state, "x"), "b.x");
    }

    #[test]
    fn invalidate_root_removes_alias() {
        let mut state = aliases("import a.b as c\n", "pkg.mod", ModuleKind::Module);
        state.invalidate_root("c");
        assert!(state.aliases.is_empty());
        assert_eq!(
            state.resolve_reference("c", &[]),
            ModelImportReference::Unresolved(ModelImportPathResolutionError::ShadowedBinding)
        );
    }

    #[test]
    fn resolve_reference_qualifies_known_alias_and_reports_missing() {
        let state = aliases("import package as alias\n", "pkg.mod", ModuleKind::Module);
        assert_eq!(
            state.resolve_reference("alias", &["nested".to_string()]),
            ModelImportReference::Qualified(module_name("package.nested"))
        );
        assert_eq!(
            state.resolve_reference("missing", &[]),
            ModelImportReference::Unresolved(ModelImportPathResolutionError::MissingBinding)
        );
    }
}
