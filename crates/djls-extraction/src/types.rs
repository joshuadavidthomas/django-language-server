use serde::Deserialize;
use serde::Serialize;

/// Key for addressing extracted rules — includes registration module to avoid collisions.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct SymbolKey {
    /// Module path where registration occurs (e.g., "django.templatetags.i18n")
    pub registration_module: String,
    /// Tag/filter name as used in templates
    pub name: String,
    /// Whether this is a tag or filter
    pub kind: SymbolKind,
}

impl SymbolKey {
    #[must_use]
    pub fn tag(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: SymbolKind::Tag,
        }
    }

    #[must_use]
    pub fn filter(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: SymbolKind::Filter,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum SymbolKind {
    Tag,
    Filter,
}

/// Result of extracting rules from a Python module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub tags: Vec<ExtractedTag>,
    pub filters: Vec<ExtractedFilter>,
}

impl ExtractionResult {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.filters.is_empty()
    }
}

/// Extracted validation data for a template tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedTag {
    /// Tag name as registered
    pub name: String,
    /// Kind of registration decorator (tag, `simple_tag`, `inclusion_tag`)
    pub decorator_kind: DecoratorKind,
    /// Validation rules extracted from `TemplateSyntaxError` guards
    pub rules: Vec<ExtractedRule>,
    /// Block structure (end tag, intermediates) if any
    pub block_spec: Option<BlockTagSpec>,
    /// Extracted argument structure for completions/snippets
    pub extracted_args: Vec<ExtractedArg>,
}

/// Extracted argument specification for a template tag.
///
/// Derived from Python AST — either directly from function parameters
/// (`simple_tag`/`inclusion_tag`) or reconstructed from `ExtractedRule`
/// conditions and AST patterns (manual `@register.tag`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedArg {
    /// Argument name (from parameter name or reconstructed)
    pub name: String,
    /// Kind of argument
    pub kind: ExtractedArgKind,
    /// Whether this argument is required
    pub required: bool,
}

/// Kind of extracted argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtractedArgKind {
    /// Fixed literal keyword (e.g., "in", "as")
    Literal { value: String },
    /// Choice from specific values (e.g., "on"/"off")
    Choice { values: Vec<String> },
    /// Positional variable/expression
    Variable,
    /// Variable number of positional arguments
    VarArgs,
    /// Keyword arguments (`**kwargs`)
    KeywordArgs,
}

/// Kind of tag registration decorator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoratorKind {
    /// `@register.tag` — manual tag function `(parser, token) -> Node`
    Tag,
    /// `@register.simple_tag` — automatic argument parsing
    SimpleTag,
    /// `@register.inclusion_tag` — renders a template
    InclusionTag,
    /// `@register.simple_block_tag` — block tag with automatic end tag.
    /// Django hardcodes `end_name = f"end{function_name}"` when not explicitly provided
    /// (`library.py:190`). This is a Django-defined semantic default for THIS decorator only.
    SimpleBlockTag,
    /// Helper/wrapper decorator (e.g., `@register_simple_block_tag`)
    /// These are NOT `register.<method>` but are recognized registration signals
    HelperWrapper(String),
    /// Unknown decorator name
    Custom(String),
}

/// A validation rule extracted from Python source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedRule {
    /// Condition that triggers the error
    pub condition: RuleCondition,
    /// Error message from the raise statement (if extractable)
    pub message: Option<String>,
}

/// Condition types for validation rules.
///
/// These do NOT reference a specific variable name. The extraction process
/// identifies the split-contents variable dynamically and these conditions are
/// expressed in terms of "the split result" abstractly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleCondition {
    /// `len(<split>) == N` or `len(<split>) != N`
    ExactArgCount { count: usize, negated: bool },
    /// `len(<split>) < N` or `len(<split>) > N`
    ArgCountComparison { count: usize, op: ComparisonOp },
    /// `len(<split>) >= N` (minimum args)
    MinArgCount { min: usize },
    /// `len(<split>) <= N` (maximum args)
    MaxArgCount { max: usize },
    /// `<split>[N] == "keyword"` or `<split>[N] != "keyword"`
    LiteralAt {
        index: usize,
        value: String,
        negated: bool,
    },
    /// `<split>[N] in ("opt1", "opt2", ...)`
    ChoiceAt {
        index: usize,
        choices: Vec<String>,
        negated: bool,
    },
    /// `"keyword" in <split>` or `"keyword" not in <split>`
    ContainsLiteral { value: String, negated: bool },
    /// Complex condition we couldn't simplify
    Opaque { description: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonOp {
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// Block structure specification extracted from `parser.parse((...))` calls.
///
/// **Important**: End tags are inferred from control flow patterns, NOT from
/// string heuristics like `starts_with("end")`. If we cannot confidently
/// identify the closer, `end_tag` is `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockTagSpec {
    /// End tag name (e.g., "endif" for "if") — inferred from control flow, not name patterns
    pub end_tag: Option<String>,
    /// Intermediate tags (e.g., "else", "elif" for "if")
    pub intermediate_tags: Vec<IntermediateTagSpec>,
    /// Whether this is an opaque block (like verbatim/comment)
    pub opaque: bool,
}

/// Specification for an intermediate tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntermediateTagSpec {
    /// Tag name (e.g., "else", "elif")
    pub name: String,
    /// Can this tag repeat? (e.g., elif can, else cannot)
    pub repeatable: bool,
}

/// Extracted validation data for a template filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedFilter {
    /// Filter name as registered
    pub name: String,
    /// Argument arity
    pub arity: FilterArity,
}

/// Filter argument arity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterArity {
    None,
    Optional,
    Required,
    Unknown,
}
