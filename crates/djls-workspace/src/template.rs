//! Django template context detection for completions
//!
//! Detects cursor position context within Django template tags to provide
//! appropriate completions and auto-closing behavior.

// TODO: is this module in the right spot or even needed?

/// Tracks what closing characters are needed to complete a template tag.
///
/// Used to determine whether the completion system needs to insert
/// closing braces when completing a Django template tag.
#[derive(Debug)]
pub enum ClosingBrace {
    /// No closing brace present - need to add full `%}` or `}}`
    None,
    /// Partial close present (just `}`) - need to add `%` or second `}`
    PartialClose,
    /// Full close present (`%}` or `}}`) - no closing needed
    FullClose,
}

/// Cursor context within a Django template tag for completion support.
///
/// Captures the state around the cursor position to provide intelligent
/// completions and determine what text needs to be inserted.
#[derive(Debug)]
pub struct TemplateTagContext {
    /// The partial tag text before the cursor (e.g., "loa" for "{% loa|")
    pub partial_tag: String,
    /// What closing characters are already present after the cursor
    pub closing_brace: ClosingBrace,
    /// Whether a space is needed before the completion (true if cursor is right after `{%`)
    pub needs_leading_space: bool,
}
