use djls_source::Span;
use ruff_python_ast as ast;

use crate::ast::RangedExt;

/// Whether a Python source file is an ordinary module or a package's
/// `__init__.py`. Relative-import base construction differs between the two:
/// a module strips its own final segment, while a package init does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    Module,
    PackageInit,
}

/// The local root name bound by `import a.b.c` (`a`). Pure source-name rule
/// used only by the syntax lowering below.
fn first_import_segment(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

enum DirectImportBinding<'ast> {
    Root,
    Alias(&'ast str),
}

/// A single clause of an `import ...` statement.
///
/// The requested spelling and binding form are lossless syntax facts. Local
/// binding and symbolic-target spelling are derived source-name rules rather
/// than duplicated strings.
pub(crate) struct DirectImportClause<'ast> {
    requested: &'ast str,
    binding: DirectImportBinding<'ast>,
    binding_span: Span,
}

impl<'ast> DirectImportClause<'ast> {
    pub(crate) fn lower(import: &'ast ast::StmtImport) -> Vec<Self> {
        import.names.iter().map(Self::from_alias).collect()
    }

    fn from_alias(alias: &'ast ast::Alias) -> Self {
        Self {
            requested: alias.name.as_str(),
            binding: alias
                .asname
                .as_ref()
                .map_or(DirectImportBinding::Root, |alias| {
                    DirectImportBinding::Alias(alias.as_str())
                }),
            binding_span: alias.span(),
        }
    }

    /// The dotted spelling exactly as written (`a.b` in `import a.b as c`).
    fn requested(&self) -> &'ast str {
        self.requested
    }

    /// The local name introduced into scope.
    pub(crate) fn bound(&self) -> &'ast str {
        match self.binding {
            DirectImportBinding::Root => first_import_segment(self.requested()),
            DirectImportBinding::Alias(alias) => alias,
        }
    }

    /// The module the local name refers to (the top package for unaliased
    /// dotted imports, the full requested spelling for aliased imports).
    pub(crate) fn target(&self) -> &'ast str {
        match self.binding {
            DirectImportBinding::Root => first_import_segment(self.requested()),
            DirectImportBinding::Alias(_) => self.requested(),
        }
    }

    /// Span of the whole alias clause, for consumers that record binding
    /// origins.
    pub(crate) fn binding_span(&self) -> Span {
        self.binding_span
    }
}

/// A single member of a `from ... import a as b` statement.
pub(crate) struct FromImportClause<'ast> {
    imported: &'ast str,
    bound: &'ast str,
    binding_span: Span,
}

impl<'ast> FromImportClause<'ast> {
    fn from_alias(alias: &'ast ast::Alias) -> Self {
        let imported = alias.name.as_str();
        let bound = alias
            .asname
            .as_ref()
            .map_or(imported, ast::Identifier::as_str);
        Self {
            imported,
            bound,
            binding_span: alias.span(),
        }
    }

    /// The name imported from the source module.
    pub(crate) fn imported(&self) -> &'ast str {
        self.imported
    }

    /// The local name introduced into scope.
    pub(crate) fn bound(&self) -> &'ast str {
        self.bound
    }

    /// Span of the whole alias clause, for consumers that record binding
    /// origins.
    pub(crate) fn binding_span(&self) -> Span {
        self.binding_span
    }
}

/// A `from [.]module import ...` statement, lowered into its relative level,
/// optional source module, explicit star, and named members.
pub(crate) struct FromImportSyntax<'ast> {
    level: u32,
    module: Option<&'ast str>,
    has_star: bool,
    members: Vec<FromImportClause<'ast>>,
}

impl<'ast> FromImportSyntax<'ast> {
    pub(crate) fn lower(import: &'ast ast::StmtImportFrom) -> Self {
        let mut has_star = false;
        let mut members = Vec::new();
        for alias in &import.names {
            if alias.name.as_str() == "*" {
                has_star = true;
            } else {
                members.push(FromImportClause::from_alias(alias));
            }
        }
        Self {
            level: import.level,
            module: import.module.as_ref().map(ast::Identifier::as_str),
            has_star,
            members,
        }
    }

    pub(crate) fn level(&self) -> u32 {
        self.level
    }

    pub(crate) fn module(&self) -> Option<&'ast str> {
        self.module
    }

    /// Whether the statement contains an explicit `*` member.
    pub(crate) fn has_star(&self) -> bool {
        self.has_star
    }

    /// The explicitly-named members (never the `*`).
    pub(crate) fn named_members(&self) -> &[FromImportClause<'ast>] {
        &self.members
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::PySourceType;
    use ruff_python_ast::Stmt;
    use ruff_python_parser::parse_module;

    use super::*;

    fn direct(source: &str) -> Vec<(String, String, String)> {
        let module = parse_module(source).unwrap().into_syntax();
        let Some(Stmt::Import(import)) = module.body.first() else {
            panic!("expected a direct import");
        };
        DirectImportClause::lower(import)
            .iter()
            .map(|clause| {
                (
                    clause.requested().to_string(),
                    clause.bound().to_string(),
                    clause.target().to_string(),
                )
            })
            .collect()
    }

    fn from_import(source: &str) -> FromImportSyntax<'_> {
        // Leak the parsed module so the borrowed facts can outlive this helper
        // within a single test; acceptable in test-only code.
        let module = Box::leak(Box::new(parse_module(source).unwrap().into_syntax()));
        let Some(Stmt::ImportFrom(import)) = module.body.first() else {
            panic!("expected a from import");
        };
        FromImportSyntax::lower(import)
    }

    #[test]
    fn direct_import_preserves_requested_bound_and_target() {
        // (requested, bound, target)
        assert_eq!(
            direct("import os\n"),
            vec![("os".into(), "os".into(), "os".into())]
        );
        assert_eq!(
            direct("import os.path\n"),
            vec![("os.path".into(), "os".into(), "os".into())]
        );
        assert_eq!(
            direct("import a.b as c\n"),
            vec![("a.b".into(), "c".into(), "a.b".into())]
        );
    }

    #[test]
    fn direct_import_preserves_clause_order_and_spans() {
        let source = "import alpha.beta as first, gamma.delta\n";
        let module = parse_module(source).unwrap().into_syntax();
        let Some(Stmt::Import(import)) = module.body.first() else {
            panic!("expected a direct import");
        };
        let clauses = DirectImportClause::lower(import);

        assert_eq!(clauses[0].requested(), "alpha.beta");
        assert_eq!(clauses[0].bound(), "first");
        assert_eq!(clauses[0].binding_span(), Span::new(7, 19));
        assert_eq!(clauses[1].requested(), "gamma.delta");
        assert_eq!(clauses[1].bound(), "gamma");
        assert_eq!(clauses[1].binding_span(), Span::new(28, 11));
    }

    #[test]
    fn from_import_preserves_relative_source_syntax() {
        let syntax = from_import("from ...parent.child import value as local\n");
        assert_eq!(syntax.level(), 3);
        assert_eq!(syntax.module(), Some("parent.child"));
        assert!(!syntax.has_star());
        assert_eq!(syntax.named_members()[0].imported(), "value");
        assert_eq!(syntax.named_members()[0].bound(), "local");
    }

    #[test]
    fn recovered_from_import_preserves_star_and_named_members() {
        let parsed = ruff_python_parser::parse_unchecked_source(
            "from module import *, named as alias\n",
            PySourceType::Python,
        );
        assert!(!parsed.errors().is_empty());
        let module = parsed.into_syntax();
        let Some(Stmt::ImportFrom(import)) = module.body.first() else {
            panic!("expected a recovered from import");
        };
        let syntax = FromImportSyntax::lower(import);

        assert!(syntax.has_star());
        let members: Vec<_> = syntax
            .named_members()
            .iter()
            .map(|clause| (clause.imported(), clause.bound(), clause.binding_span()))
            .collect();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0, "named");
        assert_eq!(members[0].1, "alias");
        assert_eq!(members[0].2, Span::new(22, 14));
    }

    #[test]
    fn from_import_names_and_star() {
        let plain = from_import("from m import x, y as z\n");
        assert!(!plain.has_star());
        let members: Vec<_> = plain
            .named_members()
            .iter()
            .map(|clause| (clause.imported(), clause.bound()))
            .collect();
        assert_eq!(members, vec![("x", "x"), ("y", "z")]);

        let star = from_import("from m import *\n");
        assert!(star.has_star());
        assert!(star.named_members().is_empty());
    }
}
