use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;

/// Identifies a specific tag or filter registration within a module.
///
/// Keyed by both the registration module path and the symbol name to avoid
/// collisions when different libraries register identically-named symbols.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey {
    pub registration_module: String,
    pub name: String,
    pub kind: TemplateSymbolKind,
}

impl SymbolKey {
    #[must_use]
    pub fn tag(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: TemplateSymbolKind::Tag,
        }
    }

    #[must_use]
    pub fn filter(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: TemplateSymbolKind::Filter,
        }
    }
}

/// Whether a symbol is a template tag or a template filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateSymbolKind {
    Tag,
    Filter,
}

pub type TagRuleMap = FxHashMap<SymbolKey, Arc<TagRule>>;
pub type FilterArityMap = FxHashMap<SymbolKey, FilterArity>;
pub type BlockSpecMap = FxHashMap<SymbolKey, BlockSpec>;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct BlockSpecs(pub BlockSpecMap);

impl Serialize for BlockSpecs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sorted = BTreeMap::new();
        for (key, value) in &self.0 {
            let kind = match key.kind {
                TemplateSymbolKind::Tag => "tag",
                TemplateSymbolKind::Filter => "filter",
            };
            sorted.insert(
                format!("{}::{kind}::{}", key.registration_module, key.name),
                value,
            );
        }

        let mut map = serializer.serialize_map(Some(sorted.len()))?;
        for (key, value) in sorted {
            map.serialize_entry(&key, value)?;
        }
        map.end()
    }
}

impl BlockSpecs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn as_map(&self) -> &BlockSpecMap {
        &self.0
    }

    pub fn insert(&mut self, key: SymbolKey, value: BlockSpec) {
        self.0.insert(key, value);
    }

    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }
}

/// Result of extracting rules from a Python registration module.
///
/// Maps each discovered symbol to its extracted validation rules. This remains
/// as a convenience aggregate for non-Salsa callers and snapshots; the Salsa
/// pipeline uses domain-specific query results so downstream consumers only
/// depend on the extracted data they read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub tag_rules: TagRuleMap,
    pub filter_arities: FilterArityMap,
    pub block_specs: BlockSpecs,
}

impl ExtractionResult {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tag_rules.is_empty() && self.filter_arities.is_empty() && self.block_specs.is_empty()
    }

    /// Merge another extraction result into this one. The other takes precedence.
    pub fn merge(&mut self, other: Self) {
        self.tag_rules.extend(other.tag_rules);
        self.filter_arities.extend(other.filter_arities);
        self.block_specs.extend(other.block_specs);
    }
}

/// How to treat trailing `as <varname>` in tag arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsVar {
    #[default]
    Keep,
    Strip,
}

impl AsVar {
    #[must_use]
    pub const fn strips_suffix(self) -> bool {
        matches!(self, Self::Strip)
    }
}

/// Validation rules extracted from a tag's compile function.
///
/// Captures the conditions under which exceptions are raised in guards,
/// expressed as structured constraints on token count, keyword positions,
/// and option values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TagRule {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
    pub choice_at_constraints: Vec<ChoiceAt>,
    pub known_options: Option<KnownOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_messages: Option<Vec<ExtractedDiagnosticMessage>>,
    pub extracted_args: Vec<TagArgument>,
    /// Support for Django's `{% tag args... as varname %}` form.
    ///
    /// When supported, the evaluator strips trailing `as <varname>` from the
    /// argument list before checking constraints. Set for `simple_tag`
    /// registrations where Django handles the `as` syntax automatically.
    #[serde(default)]
    pub as_var: AsVar,
}

impl TagRule {
    /// Returns `true` if this rule contains any meaningful constraints or arguments.
    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.arg_constraints.is_empty()
            || !self.required_keywords.is_empty()
            || !self.choice_at_constraints.is_empty()
            || self.known_options.is_some()
            || self
                .diagnostic_messages
                .as_ref()
                .is_some_and(|messages| !messages.is_empty())
            || !self.extracted_args.is_empty()
    }
}

/// A diagnostic message extracted from a raised exception in a tag parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedDiagnosticMessage {
    pub constraint: ExtractedDiagnosticConstraint,
    pub message: ExtractedMessageTemplate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtractedDiagnosticConstraint {
    ArgumentCount(ArgumentCountConstraint),
    RequiredKeyword {
        position: SplitPosition,
        value: String,
    },
    ChoiceAt {
        position: SplitPosition,
        values: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtractedMessageTemplate {
    Static(String),
    PercentFormat {
        template: String,
        args: Vec<ExtractedMessageArg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractedMessageArg {
    SplitElement(SplitPosition),
    String(String),
    Int(i64),
}

/// Constraint on the number of tokens in a tag's argument list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ArgumentCountConstraint {
    /// `len(bits) == N`
    Exact(usize),
    /// `len(bits) >= N`
    Min(usize),
    /// `len(bits) <= N`
    Max(usize),
    /// `len(bits) in {a, b, c}`
    OneOf(Vec<usize>),
}

/// Position within a `token.split_contents()` result.
///
/// In Django, `split_contents()` returns the tag name at index 0 followed by
/// arguments. This type makes that invariant explicit:
/// - `Forward(0)` is always the tag name
/// - `Forward(1)` is the first argument
/// - `Backward(1)` is the last element
///
/// The evaluator in `djls-semantic` works with `bits` (arguments only, tag name
/// excluded). Use `arg_index()` to convert to the 0-based argument index, or
/// `to_bits_index(bits_len)` to resolve backward positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SplitPosition {
    /// Absolute position from start (0 = tag name, 1 = first arg, ...)
    Forward(usize),
    /// Position from end (1 = last element, 2 = second-to-last, ...)
    Backward(usize),
}

impl SplitPosition {
    /// Convert to a 0-based argument index (in `bits` coordinates where tag
    /// name is excluded).
    ///
    /// Returns `None` for the tag name position (`Forward(0)`) and for backward
    /// positions (which require knowing the total length to resolve).
    #[must_use]
    pub fn arg_index(&self) -> Option<usize> {
        match self {
            Self::Forward(0) | Self::Backward(_) => None,
            Self::Forward(n) => Some(n - 1),
        }
    }

    /// Resolve this position to a `bits` index given the `bits` length
    /// (arguments only, tag name excluded).
    ///
    /// Returns `None` if:
    /// - This is the tag name position (`Forward(0)`)
    /// - The resolved index is out of bounds
    #[must_use]
    pub fn to_bits_index(self, bits_len: usize) -> Option<usize> {
        match self {
            Self::Forward(0) => None,
            Self::Forward(n) => {
                let idx = n - 1;
                if idx < bits_len { Some(idx) } else { None }
            }
            Self::Backward(n) => {
                if n == 0 || n > bits_len {
                    None
                } else {
                    Some(bits_len - n)
                }
            }
        }
    }
}

impl std::fmt::Display for SplitPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forward(n) => write!(f, "{n}"),
            Self::Backward(n) => write!(f, "-{n}"),
        }
    }
}

/// A keyword that must appear at a specific position in the argument list.
///
/// For example, `{% cycle ... as name %}` requires `"as"` at a specific position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequiredKeyword {
    pub position: SplitPosition,
    pub value: String,
}

/// A constraint that a specific position must hold one of a fixed set of values.
///
/// For example, `{% autoescape on %}` requires `args[1]` to be `"on"` or `"off"`.
/// Extracted from patterns like `if arg not in ("on", "off"): raise SomeException(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAt {
    pub position: SplitPosition,
    pub values: Vec<String>,
}

/// Constraints on option-style arguments parsed in a while loop.
///
/// Some Django tags (e.g., `{% include %}`, `{% url %}`) accept options
/// like `with key=value` or `only`, parsed in a `while remaining_bits:` loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnownOptions {
    pub values: Vec<String>,
    pub allow_duplicates: bool,
    pub rejects_unknown: bool,
}

/// Block structure extracted from `parser.parse((...))` control flow patterns.
///
/// Describes the end-tag and intermediate tags for a block tag, inferred
/// exclusively from `parser.parse()` call patterns and control flow — never
/// from string prefix heuristics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockSpec {
    /// The closing tag name (e.g., `"endfor"`), or `None` if inference was
    /// ambiguous and we couldn't determine the closer with confidence.
    pub end_tag: Option<String>,
    /// Intermediate tags that cause `parser.parse()` to stop and resume
    /// (e.g., `"else"`, `"elif"` for `{% if %}`).
    pub intermediates: Vec<String>,
    /// Whether the block is opaque (content should not be parsed).
    /// Detected from `parser.skip_past(...)` patterns.
    pub opaque: bool,
}

/// Argument structure extracted from a tag's registration.
///
/// Represents a single positional or keyword argument that a template tag
/// accepts, derived from the Python function signature (for simple/inclusion
/// tags) or from AST analysis of the compile function (for manual tags).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagArgument {
    /// Argument name (from parameter name or AST analysis, or generic `arg1`/`arg2`)
    pub name: String,
    /// Whether this argument is required (no default value)
    pub required: bool,
    /// The kind of argument
    pub kind: TagArgumentKind,
    /// Zero-based position index in the argument list (excluding tag name)
    pub position: usize,
}

/// The kind of an extracted argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagArgumentKind {
    /// A template variable or expression
    Variable,
    /// A literal keyword that must appear exactly as specified
    Literal(String),
    /// A choice between specific literal values
    Choice(Vec<String>),
    /// Consumes all remaining arguments (`*args`)
    VarArgs,
    /// A keyword argument (`**kwargs` or keyword-only)
    Keyword,
}

/// Filter argument arity extracted from the filter function's signature.
///
/// Django filters receive the value being filtered as their first argument.
/// Some filters accept an additional argument (e.g., `{{ value|default:"nothing" }}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterArity {
    /// Whether the filter expects an argument after the colon.
    pub expects_arg: bool,
    /// Whether the argument is optional (has a default value).
    pub arg_optional: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_key_tag_constructor() {
        let key = SymbolKey::tag("django.template.defaulttags", "for");
        assert_eq!(key.registration_module, "django.template.defaulttags");
        assert_eq!(key.name, "for");
        assert_eq!(key.kind, TemplateSymbolKind::Tag);
    }

    #[test]
    fn symbol_key_filter_constructor() {
        let key = SymbolKey::filter("django.template.defaultfilters", "title");
        assert_eq!(key.registration_module, "django.template.defaultfilters");
        assert_eq!(key.name, "title");
        assert_eq!(key.kind, TemplateSymbolKind::Filter);
    }

    #[test]
    fn extraction_result_empty() {
        let result = ExtractionResult::default();
        assert!(result.is_empty());
    }

    #[test]
    fn extraction_result_merge() {
        let mut result1 = ExtractionResult::default();
        result1.tag_rules.insert(
            SymbolKey::tag("mod1", "tag1"),
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Exact(3)],
                ..Default::default()
            }
            .into(),
        );

        let mut result2 = ExtractionResult::default();
        result2.filter_arities.insert(
            SymbolKey::filter("mod2", "filter1"),
            FilterArity {
                expects_arg: true,
                arg_optional: false,
            },
        );

        result1.merge(result2);
        assert!(!result1.is_empty());
        assert_eq!(result1.tag_rules.len(), 1);
        assert_eq!(result1.filter_arities.len(), 1);
    }

    #[test]
    fn extraction_result_merge_overwrites() {
        let mut result1 = ExtractionResult::default();
        let key = SymbolKey::tag("mod1", "tag1");
        result1.tag_rules.insert(
            key.clone(),
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Exact(3)],
                ..Default::default()
            }
            .into(),
        );

        let mut result2 = ExtractionResult::default();
        result2.tag_rules.insert(
            key.clone(),
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Min(2)],
                ..Default::default()
            }
            .into(),
        );

        result1.merge(result2);
        assert_eq!(result1.tag_rules.len(), 1);

        let rule = result1.tag_rules.get(&key).unwrap();
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn block_tag_spec_opaque() {
        let spec = BlockSpec {
            end_tag: Some("endverbatim".to_string()),
            intermediates: vec![],
            opaque: true,
        };
        assert!(spec.opaque);
        assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
    }

    #[test]
    fn block_tag_spec_with_intermediates() {
        let spec = BlockSpec {
            end_tag: Some("endif".to_string()),
            intermediates: vec!["elif".to_string(), "else".to_string()],
            opaque: false,
        };
        assert!(!spec.opaque);
        assert_eq!(spec.intermediates.len(), 2);
    }

    #[test]
    fn filter_arity_no_arg() {
        let arity = FilterArity {
            expects_arg: false,
            arg_optional: false,
        };
        assert!(!arity.expects_arg);
    }

    #[test]
    fn filter_arity_required_arg() {
        let arity = FilterArity {
            expects_arg: true,
            arg_optional: false,
        };
        assert!(arity.expects_arg);
        assert!(!arity.arg_optional);
    }

    #[test]
    fn filter_arity_optional_arg() {
        let arity = FilterArity {
            expects_arg: true,
            arg_optional: true,
        };
        assert!(arity.expects_arg);
        assert!(arity.arg_optional);
    }
}
