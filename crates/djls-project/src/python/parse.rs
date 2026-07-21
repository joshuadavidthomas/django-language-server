use std::cmp::Ordering;

use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileKind;
use djls_source::FileReadError;
use djls_source::Span;
use ruff_python_ast::ModModule;
use ruff_python_ast::PySourceType;
use ruff_python_ast::Stmt;

use crate::ast::RangedExt;
use crate::python::evaluation::StructuralOrd;

#[derive(Clone, PartialEq, Eq, salsa::SalsaValue)]
enum PythonParseResult<'db> {
    Parsed(PythonParse<'db>),
    NotPython,
    Unreadable(FileReadError),
}

/// The internal product of one recovered Ruff parse.
#[salsa::tracked]
struct PythonParse<'db> {
    #[tracked]
    #[returns(ref)]
    body: Vec<Stmt>,
    #[tracked]
    #[returns(ref)]
    syntax_errors: Vec<PythonSyntaxError>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveredPythonModule<'db> {
    parse: PythonParse<'db>,
}

impl<'db> RecoveredPythonModule<'db> {
    pub(crate) fn from_file(
        db: &'db dyn SourceDb,
        file: File,
    ) -> Result<Option<Self>, FileReadError> {
        match parse_python_file(db, file) {
            PythonParseResult::Parsed(parse) => Ok(Some(Self { parse })),
            PythonParseResult::NotPython => Ok(None),
            PythonParseResult::Unreadable(error) => Err(error),
        }
    }

    pub(crate) fn body(self, db: &'db dyn SourceDb) -> &'db [Stmt] {
        self.parse.body(db)
    }

    pub(crate) fn syntax_errors(self, db: &'db dyn SourceDb) -> &'db [PythonSyntaxError] {
        self.parse.syntax_errors(db)
    }

    pub(crate) fn has_ordinary_syntax_errors(self, db: &'db dyn SourceDb) -> bool {
        self.syntax_errors(db)
            .iter()
            .any(|error| error.class == PythonSyntaxErrorClass::Ordinary)
    }
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

impl StructuralOrd for PythonSyntaxError {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.class
            .cmp(&other.class)
            .then_with(|| self.span.start().cmp(&other.span.start()))
            .then_with(|| self.span.length().cmp(&other.span.length()))
            .then_with(|| self.message.cmp(&other.message))
    }
}

struct PythonParseOutput {
    module: ModModule,
    syntax_errors: Vec<PythonSyntaxError>,
}

/// Convert Ruff's recovered parser output into project-owned syntax evidence.
///
/// Keeping this pure gives tracked parsing one error-normalization policy.
fn parse_python_source(source: &str) -> PythonParseOutput {
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

    PythonParseOutput {
        module: parsed.into_syntax(),
        syntax_errors,
    }
}

pub(crate) fn python_syntax_errors(db: &dyn SourceDb, file: File) -> Option<&[PythonSyntaxError]> {
    match parse_python_file(db, file) {
        PythonParseResult::Parsed(parse) => Some(parse.syntax_errors(db)),
        PythonParseResult::NotPython | PythonParseResult::Unreadable(_) => None,
    }
}

#[salsa::tracked(returns(clone))]
fn parse_python_file(db: &dyn SourceDb, file: File) -> PythonParseResult<'_> {
    let source = match file.try_source(db) {
        Ok(source) => source,
        Err(error) => return PythonParseResult::Unreadable(error),
    };
    if *source.kind() != FileKind::Python {
        return PythonParseResult::NotPython;
    }

    let parsed = parse_python_source(source.as_ref());
    PythonParseResult::Parsed(PythonParse::new(
        db,
        parsed.module.body,
        parsed.syntax_errors,
    ))
}
