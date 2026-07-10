use djls_source::File;
use djls_source::FileKind;
use djls_source::Span;
use ruff_python_ast::ModModule;
use ruff_python_ast::PySourceType;
use ruff_python_ast::Stmt;

use crate::ast::RangedExt;

#[derive(Clone, Copy, PartialEq, Eq, salsa::Update)]
pub(crate) enum PythonParseResult<'db> {
    Parsed(ParsedPythonModule<'db>),
    NotPython,
}

/// Parsed Python module AST and syntax errors, cached by Salsa.
#[salsa::tracked]
pub(crate) struct ParsedPythonModule<'db> {
    #[tracked]
    #[returns(ref)]
    pub(crate) body: Vec<Stmt>,
    #[tracked]
    #[returns(ref)]
    pub(crate) syntax_errors: Vec<PythonSyntaxError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PythonSyntaxErrorClass {
    Ordinary,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonSyntaxError {
    pub class: PythonSyntaxErrorClass,
    pub span: Span,
    pub message: String,
}

pub(super) struct ParsedPythonSource {
    pub(super) module: ModModule,
    syntax_errors: Vec<PythonSyntaxError>,
}

impl ParsedPythonSource {
    pub(super) fn has_parse_errors(&self) -> bool {
        self.syntax_errors
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Ordinary)
    }
}

impl ParsedPythonModule<'_> {
    pub(crate) fn has_parse_errors(self, db: &dyn djls_source::Db) -> bool {
        self.syntax_errors(db)
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Ordinary)
    }
}

/// Convert Ruff's recovered parser output into project-owned syntax evidence.
///
/// Keeping this pure lets tracked parsing and the legacy settings graph share
/// exactly the same recovery and error-normalization policy.
pub(super) fn parse_unchecked_source(source: &str) -> ParsedPythonSource {
    let parsed = ruff_python_parser::parse_unchecked_source(source, PySourceType::Python);
    let mut syntax_errors =
        Vec::with_capacity(parsed.errors().len() + parsed.unsupported_syntax_errors().len());

    syntax_errors.extend(parsed.errors().iter().map(|error| PythonSyntaxError {
        class: PythonSyntaxErrorClass::Ordinary,
        span: error.span(),
        message: error.error.to_string(),
    }));
    syntax_errors.extend(parsed.unsupported_syntax_errors().iter().map(|error| {
        PythonSyntaxError {
            class: PythonSyntaxErrorClass::Unsupported,
            span: error.span(),
            message: error.to_string(),
        }
    }));
    syntax_errors.sort_by_key(|error| (error.span.start(), error.span.length(), error.class));
    syntax_errors.dedup_by(|left, right| left.span == right.span && left.class == right.class);

    ParsedPythonSource {
        module: parsed.into_syntax(),
        syntax_errors,
    }
}

/// Parse a Python source file into a cached recovered AST.
#[salsa::tracked]
pub(crate) fn parse_python_module(db: &dyn djls_source::Db, file: File) -> PythonParseResult<'_> {
    let source = file.source(db);
    if *source.kind() != FileKind::Python {
        return PythonParseResult::NotPython;
    }

    let parsed = parse_unchecked_source(source.as_ref());
    PythonParseResult::Parsed(ParsedPythonModule::new(
        db,
        parsed.module.body,
        parsed.syntax_errors,
    ))
}
