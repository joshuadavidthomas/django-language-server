use djls_source::File;
use djls_source::FileKind;
use ruff_python_ast::Stmt;

/// Parsed Python module AST, cached by Salsa.
///
/// Wraps Ruff's statement list in a tracked struct. The parsed AST is
/// invalidated when the source file changes.
#[salsa::tracked]
pub struct ParsedPythonModule<'db> {
    #[tracked]
    #[returns(ref)]
    pub body: Vec<Stmt>,
}

/// Parse a Python source file into a cached AST.
///
/// Returns `None` for non-Python files or files that fail to parse.
/// The parsed AST is cached by Salsa and invalidated when
/// `file.source(db)` changes.
#[salsa::tracked]
pub fn parse_python_module(db: &dyn djls_source::Db, file: File) -> Option<ParsedPythonModule<'_>> {
    let source = file.source(db);
    if *source.kind() != FileKind::Python {
        return None;
    }

    let parsed = ruff_python_parser::parse_module(source.as_ref());
    let module = match parsed {
        Ok(parsed) => parsed.into_syntax(),
        Err(_) => return None,
    };

    Some(ParsedPythonModule::new(db, module.body))
}
