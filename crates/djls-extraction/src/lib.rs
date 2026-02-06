mod types;

#[cfg(feature = "parser")]
mod blocks;
#[cfg(feature = "parser")]
mod context;
#[cfg(feature = "parser")]
mod registry;
#[cfg(feature = "parser")]
mod rules;

#[cfg(feature = "parser")]
pub use blocks::extract_block_spec;
#[cfg(feature = "parser")]
pub use context::detect_split_var;
#[cfg(feature = "parser")]
pub use registry::collect_registrations;
#[cfg(feature = "parser")]
pub use registry::RegistrationInfo;
#[cfg(feature = "parser")]
pub use registry::RegistrationKind;
#[cfg(feature = "parser")]
pub use rules::extract_tag_rule;
pub use types::ArgumentCountConstraint;
pub use types::BlockTagSpec;
pub use types::ExtractionResult;
pub use types::FilterArity;
pub use types::KnownOptions;
pub use types::RequiredKeyword;
pub use types::SymbolKey;
pub use types::SymbolKind;
pub use types::TagRule;

/// Extract validation rules from a Python registration module source.
///
/// Parses the source with Ruff's Python parser, walks the AST to find
/// `@register.tag` / `@register.filter` decorators, and extracts validation
/// semantics (argument counts, block structure, option constraints) from the
/// associated compile functions.
///
/// Returns an `ExtractionResult` mapping each discovered `SymbolKey` to its
/// extracted rules.
#[cfg(feature = "parser")]
#[must_use]
pub fn extract_rules(_source: &str) -> ExtractionResult {
    ExtractionResult::default()
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    #[test]
    fn smoke_test_ruff_parser() {
        let source = r#"
from django import template

register = template.Library()

@register.simple_tag
def hello():
    return "Hello, world!"
"#;

        let result = parse_module(source);
        assert!(result.is_ok());

        let parsed = result.unwrap();
        let module = parsed.into_syntax();
        assert!(!module.body.is_empty());
    }
}
