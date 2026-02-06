use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ExtractionError {
    #[error("Failed to parse Python source: {message}")]
    ParseError { message: String },

    #[error("Unsupported Python syntax at offset {offset}: {description}")]
    UnsupportedSyntax { offset: usize, description: String },

    #[error("Could not resolve reference: {name}")]
    UnresolvedReference { name: String },
}
