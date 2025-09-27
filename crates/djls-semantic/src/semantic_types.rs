//! Tracked semantic types with Salsa optimization
//! These types use interned strings and #[salsa::no_eq] spans for optimal caching

use djls_source::Span;

use crate::ids::SemanticId;
use crate::interned::{ArgumentList, FilterChain, TagName, VariablePath};

/// Tracked semantic tag with position-independent equality
/// Spans are excluded from equality checks to preserve cache on reformatting
#[salsa::tracked]
pub struct SemanticTag<'db> {
    /// Unique identifier for this tag
    pub id: SemanticId,
    
    /// Interned tag name (e.g., "block", "for", "extends")
    pub name: TagName<'db>,
    
    /// Interned arguments for this tag
    pub arguments: ArgumentList<'db>,
    
    /// Position in source - excluded from equality
    #[no_eq]
    pub span: Span,
    
    /// End tag position for block tags - excluded from equality
    #[no_eq]
    pub closing_span: Option<Span>,
}

/// Tracked semantic variable with filters
/// Spans are excluded from equality checks
#[salsa::tracked]
pub struct SemanticVariable<'db> {
    /// Interned variable path (e.g., ["user", "profile", "name"])
    pub path: VariablePath<'db>,
    
    /// Interned filter chain applied to this variable
    pub filters: FilterChain<'db>,
    
    /// Position in source - excluded from equality
    #[no_eq]
    pub span: Span,
}

/// Text node in template
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextNode {
    pub text: String,
    pub span: Span,
}

/// Block node for template blocks
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockNode<'db> {
    pub name: TagName<'db>,
    pub span: Span,
    pub content: Vec<Span>, // Just store spans, not full elements
}

/// Validation error for semantic elements
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    pub span: Span,
    pub message: String,
}

/// Type representation for variables
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Any,
    None,
    String,
    Int,
    Float,
    Bool,
    List(Box<Type>),
    Dict,
    Object(String),
    Union(Vec<Type>),
}

// Implementation of tracked methods as separate queries
// This follows the Salsa 0.23.0 pattern for tracked functions

/// Validate a semantic tag - expensive operation
#[salsa::tracked]
pub fn validate_tag<'db>(_db: &'db dyn crate::Db, _tag: SemanticTag<'db>) -> Vec<ValidationError> {
    // TODO: Implement actual validation logic
    // This is a placeholder for the expensive validation computation
    vec![]
}

/// Get documentation for a tag - expensive operation
#[salsa::tracked]
pub fn tag_documentation<'db>(_db: &'db dyn crate::Db, _tag: SemanticTag<'db>) -> Option<String> {
    // TODO: Load documentation from external source
    // This is a placeholder for the expensive doc lookup
    None
}

/// Infer the type of a variable - expensive operation
#[salsa::tracked]
pub fn infer_variable_type<'db>(_db: &'db dyn crate::Db, _var: SemanticVariable<'db>) -> Option<Type> {
    // TODO: Implement type inference logic
    // This is a placeholder for the expensive type inference
    Some(Type::Any)
}