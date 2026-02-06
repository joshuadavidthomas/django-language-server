//! Python AST extraction for Django template tag/filter validation rules.
//!
//! This crate provides a pure API for extracting validation semantics from
//! Python template tag/filter registration modules. It does NOT:
//! - Resolve module paths to file paths (caller's responsibility)
//! - Read files from disk (takes source text)
//! - Know about sys.path or Python environments
//!
//! # Key Design Decisions
//!
//! 1. **No hardcoded variable names**: The split-contents variable (often `bits`,
//!    but could be `args`, `parts`, etc.) is detected by finding the assignment
//!    `<var> = token.split_contents()` and threading that name through rule extraction.
//!
//! 2. **No string-based end-tag heuristics**: End tags are NOT inferred from names
//!    like `starts_with("end")`. Instead, we analyze control flow patterns:
//!    - Singleton `parser.parse((<single>,))` calls indicate the closer
//!    - If ambiguous, we emit `None` rather than guess
//!
//! # Example
//!
//! ```rust,ignore
//! use djls_extraction::extract_rules;
//!
//! let source = r#"
//! @register.tag
//! def my_tag(parser, token):
//!     args = token.split_contents()  // Note: 'args', not 'bits'
//!     if len(args) != 2:
//!         raise TemplateSyntaxError("...")
//!     return MyNode(args[1])
//! "#;
//!
//! let result = extract_rules(source)?;
//! // result.tags contains ExtractedTag for "my_tag" with rules keyed to 'args'
//! ```

mod context;
mod error;
mod filters;
mod parser;
mod patterns;
mod registry;
mod rules;
mod structural;
mod types;

pub use error::ExtractionError;
pub use types::BlockTagSpec;
pub use types::DecoratorKind;
pub use types::ExtractedFilter;
pub use types::ExtractedRule;
pub use types::ExtractedTag;
pub use types::ExtractionResult;
pub use types::FilterArity;
pub use types::IntermediateTagSpec;
pub use types::RuleCondition;
pub use types::SymbolKey;
pub use types::SymbolKind;

/// Extract validation rules from a Python template registration module.
///
/// This is a pure function: source text in, extraction result out.
/// Module-to-path resolution is NOT this crate's responsibility.
pub fn extract_rules(source: &str) -> Result<ExtractionResult, ExtractionError> {
    let parsed = parser::parse_module(source)?;
    let found = registry::find_registrations(&parsed)?;

    let mut tags = Vec::new();
    let mut filters = Vec::new();

    for reg in &found.tags {
        // Build function context (detects split_var name, parser_var, token_var)
        let ctx = context::FunctionContext::from_registration(&parsed, reg);

        let rules = rules::extract_tag_rules(&parsed, reg, &ctx)?;
        let block_spec = structural::extract_block_spec(&parsed, reg, &ctx)?;

        tags.push(ExtractedTag {
            name: reg.name.clone(),
            decorator_kind: reg.decorator_kind.clone(),
            rules,
            block_spec,
        });
    }

    for reg in &found.filters {
        let arity = filters::extract_filter_arity(&parsed, reg)?;

        filters.push(ExtractedFilter {
            name: reg.name.clone(),
            arity,
        });
    }

    Ok(ExtractionResult { tags, filters })
}
