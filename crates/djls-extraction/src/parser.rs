use ruff_python_ast::ModModule;
use ruff_python_parser::parse_module as ruff_parse;
use ruff_python_parser::Parsed;

use crate::ExtractionError;

#[allow(dead_code)]
pub struct ParsedModule {
    pub(crate) parsed: Parsed<ModModule>,
}

#[allow(dead_code)]
impl ParsedModule {
    pub fn ast(&self) -> &ModModule {
        self.parsed.syntax()
    }
}

pub fn parse_module(source: &str) -> Result<ParsedModule, ExtractionError> {
    match ruff_parse(source) {
        Ok(parsed) => Ok(ParsedModule { parsed }),
        Err(error) => {
            let message = error.to_string();
            Err(ExtractionError::ParseError { message })
        }
    }
}
