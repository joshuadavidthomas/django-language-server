use ruff_python_ast::ModModule;
use ruff_python_parser::Parsed;

use crate::ExtractionError;

#[allow(dead_code)]
pub struct ParsedModule {
    pub(crate) parsed: Parsed<ModModule>,
}

impl ParsedModule {
    #[allow(dead_code)]
    #[must_use]
    pub fn ast(&self) -> &ModModule {
        self.parsed.syntax()
    }
}

pub fn parse_module(source: &str) -> Result<ParsedModule, ExtractionError> {
    ruff_python_parser::parse_module(source).map_or_else(
        |error| {
            Err(ExtractionError::ParseError {
                message: error.to_string(),
            })
        },
        |parsed| Ok(ParsedModule { parsed }),
    )
}
