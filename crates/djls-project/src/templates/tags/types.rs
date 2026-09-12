use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;

use crate::templates::SymbolKey;
use crate::templates::TemplateSymbolKind;

pub type TagRuleMap = FxHashMap<SymbolKey, Arc<TagRule>>;
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
    /// Accepted argument syntax in template source order.
    #[serde(default)]
    pub argument_syntax: TagArgumentSyntax,
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
    pub(crate) fn has_content(&self) -> bool {
        !self.arg_constraints.is_empty()
            || !self.required_keywords.is_empty()
            || !self.choice_at_constraints.is_empty()
            || self.known_options.is_some()
            || self
                .diagnostic_messages
                .as_ref()
                .is_some_and(|messages| !messages.is_empty())
            || match &self.argument_syntax {
                TagArgumentSyntax::Signature { .. } => true,
                TagArgumentSyntax::Parameters(parameters) => !parameters.is_empty(),
                TagArgumentSyntax::Forms { forms, .. } => !forms.is_empty(),
                TagArgumentSyntax::Unknown => false,
            }
    }
}

/// A diagnostic message extracted from a raised exception in a tag parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedDiagnosticMessage {
    pub constraint: ExtractedDiagnosticConstraint,
    pub message: ExtractedMessageTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The source token reconstructed from the semantic tag name and Tag Bits.
    ///
    /// Rendering joins the tag name and each bit with one ASCII space. This
    /// preserves bit spellings, including quotes, but normalizes source whitespace.
    TokenContents,
    String(String),
    Int(i64),
}

/// Constraint on the number of tokens in a tag's argument list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// excluded). Use `to_bits_index(bits_len)` to convert to a 0-based argument
/// index and resolve backward positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SplitPosition {
    /// Absolute position from start (0 = tag name, 1 = first arg, ...)
    Forward(usize),
    /// Position from end (1 = last element, 2 = second-to-last, ...)
    Backward(usize),
}

impl SplitPosition {
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
    pub duplicate_rejection: OptionRejection,
    pub unknown_rejection: OptionRejection,
}

/// Whether extraction recognized a rejection guard in an option-parsing loop.
/// Non-detection does not establish that the tag accepts the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionRejection {
    NotDetected,
    Detected,
}

/// Evidence about how a tag's compile function consumes its body.
///
/// This records source observations only. Template semantics decide whether the
/// body should be analyzed after combining this evidence with builtin and
/// configured fallback meaning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyAnalysisEvidence {
    /// No `parser.skip_past(...)` call was found.
    #[default]
    NotDetected,
    /// `parser.skip_past(...)` was found without a `parser.parse(...)` call.
    SkipPast,
    /// Both `parser.skip_past(...)` and `parser.parse(...)` were found.
    Mixed,
}

/// Block structure extracted from template parser calls.
///
/// Describes the end-tag and intermediate tags inferred from parser call
/// patterns without deriving semantic body policy from absence of evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockSpec {
    /// The closing tag name (e.g., `"endfor"`), or `None` if inference was
    /// ambiguous and we couldn't determine the closer with confidence.
    pub end_tag: Option<String>,
    /// Intermediate tags that cause `parser.parse()` to stop and resume
    /// (e.g., `"else"`, `"elif"` for `{% if %}`).
    pub intermediates: Vec<String>,
    /// Source evidence about whether the compile function skips or parses its body.
    pub body_analysis_evidence: BodyAnalysisEvidence,
}

/// Argument syntax known for a tag definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagArgumentSyntax {
    /// No useful argument syntax was found.
    #[default]
    Unknown,
    /// A trusted callable contract consumed by Django's `parse_bits()`.
    ///
    /// `parameters` is the single source for both completion presentation and
    /// binding. Positional parameters come first, followed by an optional
    /// `VarArgs` parameter and keyword-only parameters. `positional_only`
    /// preserves the Python source distinction even though `parse_bits()`
    /// accepts those names as template keywords.
    Signature {
        parameters: Vec<TagArgument>,
        positional_only: usize,
        variadic_keyword: Option<String>,
    },
    /// A configured or manually inferred parameter sequence used as a hint.
    Parameters(Vec<TagArgument>),
    /// Correlated fixed-length forms found in a manual compile function.
    Forms {
        forms: Vec<TagArgumentForm>,
        coverage: ArgumentFormCoverage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length_mismatch_message: Option<ExtractedMessageTemplate>,
    },
}

impl TagArgumentSyntax {
    #[must_use]
    pub fn parameters(&self) -> Option<&[TagArgument]> {
        match self {
            Self::Signature { parameters, .. } | Self::Parameters(parameters) => Some(parameters),
            Self::Unknown | Self::Forms { .. } => None,
        }
    }

    #[must_use]
    pub fn forms(&self) -> Option<(&[TagArgumentForm], ArgumentFormCoverage)> {
        match self {
            Self::Forms {
                forms, coverage, ..
            } => Some((forms, *coverage)),
            Self::Unknown | Self::Signature { .. } | Self::Parameters(_) => None,
        }
    }
}

/// Whether the known forms cover every successful syntax path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentFormCoverage {
    Complete,
    Partial,
}

/// One correlated argument form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagArgumentForm {
    pattern: Vec<TagArgumentPattern>,
}

impl<'de> Deserialize<'de> for TagArgumentForm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializedForm {
            pattern: Vec<TagArgumentPattern>,
        }

        let serialized = SerializedForm::deserialize(deserializer)?;
        Self::new(serialized.pattern).map_err(serde::de::Error::custom)
    }
}

impl TagArgumentForm {
    /// Build a form with at most one variable-width section.
    pub fn new(pattern: Vec<TagArgumentPattern>) -> Result<Self, TagArgumentFormError> {
        if pattern
            .iter()
            .filter(|argument| {
                matches!(argument.kind, TagArgumentPatternKind::VariableWidth { .. })
            })
            .count()
            > 1
        {
            return Err(TagArgumentFormError::MultipleVariableWidthSections);
        }
        Ok(Self { pattern })
    }

    #[must_use]
    pub fn pattern(&self) -> &[TagArgumentPattern] {
        &self.pattern
    }

    /// Merge diagnostic messages when `other` has the same pattern kinds.
    ///
    /// A message remains attached only when both forms agree on it. Returns
    /// `false` without changing this form when the pattern shapes differ.
    pub(crate) fn merge_messages_if_same_shape(&mut self, other: &Self) -> bool {
        if self.pattern.len() != other.pattern.len()
            || self
                .pattern
                .iter()
                .zip(&other.pattern)
                .any(|(left, right)| left.kind != right.kind)
        {
            return false;
        }

        for (left, right) in self.pattern.iter_mut().zip(&other.pattern) {
            if left.mismatch_message != right.mismatch_message {
                left.mismatch_message = None;
            }
        }
        true
    }

    #[must_use]
    pub fn minimum_len(&self) -> usize {
        self.pattern
            .iter()
            .map(|argument| match argument.kind {
                TagArgumentPatternKind::VariableWidth { minimum } => minimum,
                TagArgumentPatternKind::Variable
                | TagArgumentPatternKind::Literal(_)
                | TagArgumentPatternKind::Choice(_)
                | TagArgumentPatternKind::VariableExcept(_) => 1,
            })
            .sum()
    }

    #[must_use]
    pub fn exact_len(&self) -> Option<usize> {
        (!self
            .pattern
            .iter()
            .any(|argument| matches!(argument.kind, TagArgumentPatternKind::VariableWidth { .. })))
        .then_some(self.pattern.len())
    }

    /// Match a complete argument list against this form.
    pub fn match_full<S: AsRef<str>>(&self, bits: &[S]) -> Result<(), TagArgumentFormMismatch<'_>> {
        let variable_width = self.pattern.iter().position(|argument| {
            matches!(argument.kind, TagArgumentPatternKind::VariableWidth { .. })
        });
        let repeated = if variable_width.is_some() {
            if bits.len() < self.minimum_len() {
                return Err(TagArgumentFormMismatch::Length);
            }
            bits.len() - (self.pattern.len() - 1)
        } else {
            if bits.len() != self.pattern.len() {
                return Err(TagArgumentFormMismatch::Length);
            }
            0
        };

        for (argument_index, bit) in bits.iter().enumerate() {
            let pattern_index = match variable_width {
                Some(variable_index) if argument_index < variable_index => argument_index,
                Some(variable_index) if argument_index < variable_index + repeated => {
                    variable_index
                }
                Some(_) => argument_index - repeated + 1,
                None => argument_index,
            };
            let argument = &self.pattern[pattern_index];
            if let Some(expected) = argument.kind.mismatch_expectation(bit.as_ref()) {
                return Err(TagArgumentFormMismatch::Atom(FormAtomMismatch {
                    argument_index,
                    expected,
                    message: argument.mismatch_message.as_ref(),
                }));
            }
        }
        Ok(())
    }

    /// Return every atom that may consume the next argument after `completed`.
    #[must_use]
    pub fn prefix_continuations<S: AsRef<str>>(
        &self,
        completed: &[S],
    ) -> Vec<FormContinuation<'_>> {
        let mut states = vec![(0usize, 0usize)];
        for bit in completed {
            let mut next = Vec::new();
            for (pattern_index, repeated) in states {
                self.consume_prefix_bit(pattern_index, repeated, bit.as_ref(), &mut next);
            }
            next.sort_unstable();
            next.dedup();
            states = next;
            if states.is_empty() {
                return Vec::new();
            }
        }

        let mut continuations = Vec::new();
        for (pattern_index, repeated) in states {
            self.collect_continuations(pattern_index, repeated, &mut continuations);
        }
        continuations.dedup();
        continuations
    }

    fn consume_prefix_bit(
        &self,
        pattern_index: usize,
        repeated: usize,
        bit: &str,
        next: &mut Vec<(usize, usize)>,
    ) {
        let Some(argument) = self.pattern.get(pattern_index) else {
            return;
        };
        if let TagArgumentPatternKind::VariableWidth { minimum } = argument.kind {
            if argument.kind.matches(bit) {
                next.push((pattern_index, repeated + 1));
            }
            if repeated >= minimum {
                self.consume_prefix_bit(pattern_index + 1, 0, bit, next);
            }
        } else if argument.kind.matches(bit) {
            next.push((pattern_index + 1, 0));
        }
    }

    fn collect_continuations<'a>(
        &'a self,
        pattern_index: usize,
        repeated: usize,
        continuations: &mut Vec<FormContinuation<'a>>,
    ) {
        let Some(argument) = self.pattern.get(pattern_index) else {
            return;
        };
        if let TagArgumentPatternKind::VariableWidth { minimum } = argument.kind {
            let continuation = FormContinuation { argument };
            if !continuations.contains(&continuation) {
                continuations.push(continuation);
            }
            if repeated >= minimum {
                self.collect_continuations(pattern_index + 1, 0, continuations);
            }
        } else {
            let continuation = FormContinuation { argument };
            if !continuations.contains(&continuation) {
                continuations.push(continuation);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagArgumentFormError {
    MultipleVariableWidthSections,
}

impl std::fmt::Display for TagArgumentFormError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MultipleVariableWidthSections => formatter
                .write_str("an argument form may contain at most one variable-width section"),
        }
    }
}

impl std::error::Error for TagArgumentFormError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagArgumentPattern {
    pub name: String,
    pub kind: TagArgumentPatternKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mismatch_message: Option<ExtractedMessageTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagArgumentPatternKind {
    Variable,
    Literal(String),
    Choice(Vec<String>),
    VariableWidth { minimum: usize },
    VariableExcept(String),
}

impl TagArgumentPatternKind {
    #[must_use]
    pub fn matches(&self, bit: &str) -> bool {
        self.mismatch_expectation(bit).is_none()
    }

    fn mismatch_expectation<'a>(&'a self, bit: &str) -> Option<FormAtomExpectation<'a>> {
        match self {
            Self::Literal(value) if bit != value => Some(FormAtomExpectation::Literal(value)),
            Self::Choice(values) if !values.iter().any(|value| value == bit) => {
                Some(FormAtomExpectation::Choice(values))
            }
            Self::VariableExcept(value) if bit == value => {
                Some(FormAtomExpectation::Excluded(value))
            }
            Self::Variable
            | Self::Literal(_)
            | Self::Choice(_)
            | Self::VariableWidth { .. }
            | Self::VariableExcept(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagArgumentFormMismatch<'a> {
    Length,
    Atom(FormAtomMismatch<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormAtomMismatch<'a> {
    pub argument_index: usize,
    pub expected: FormAtomExpectation<'a>,
    pub message: Option<&'a ExtractedMessageTemplate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormAtomExpectation<'a> {
    Literal(&'a str),
    Choice(&'a [String]),
    Excluded(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormContinuation<'a> {
    pub argument: &'a TagArgumentPattern,
}

/// Whether a parameter must be present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterRequirement {
    Required,
    Optional,
}

impl ParameterRequirement {
    #[must_use]
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
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
    /// Whether this parameter must be present.
    pub requirement: ParameterRequirement,
    /// The kind of argument
    pub kind: TagArgumentKind,
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

    fn pattern(name: &str, kind: TagArgumentPatternKind) -> TagArgumentPattern {
        TagArgumentPattern {
            name: name.to_string(),
            kind,
            mismatch_message: None,
        }
    }

    #[test]
    fn argument_form_rejects_multiple_variable_width_sections() {
        let result = TagArgumentForm::new(vec![
            pattern(
                "first",
                TagArgumentPatternKind::VariableWidth { minimum: 0 },
            ),
            pattern(
                "second",
                TagArgumentPatternKind::VariableWidth { minimum: 1 },
            ),
        ]);
        assert_eq!(
            result,
            Err(TagArgumentFormError::MultipleVariableWidthSections)
        );
    }

    #[test]
    fn argument_form_deserialization_enforces_variable_width_invariant() {
        let serialized = r#"{"pattern":[{"name":"first","kind":{"VariableWidth":{"minimum":0}}},{"name":"second","kind":{"VariableWidth":{"minimum":1}}}]}"#;
        let result = serde_json::from_str::<TagArgumentForm>(serialized);
        assert!(result.is_err());
    }

    #[test]
    fn argument_form_merges_only_messages_for_the_same_shape() {
        let first_message = ExtractedMessageTemplate::Static("first".to_string());
        let second_message = ExtractedMessageTemplate::Static("second".to_string());
        let mut form = TagArgumentForm::new(vec![TagArgumentPattern {
            name: "mode".to_string(),
            kind: TagArgumentPatternKind::Literal("safe".to_string()),
            mismatch_message: Some(first_message.clone()),
        }])
        .expect("one fixed-width atom is valid");
        let same_shape = TagArgumentForm::new(vec![TagArgumentPattern {
            name: "other_name".to_string(),
            kind: TagArgumentPatternKind::Literal("safe".to_string()),
            mismatch_message: Some(second_message),
        }])
        .expect("one fixed-width atom is valid");

        assert!(form.merge_messages_if_same_shape(&same_shape));
        assert_eq!(form.pattern[0].mismatch_message, None);

        form.pattern[0].mismatch_message = Some(first_message);
        let before_different_shape = form.clone();
        let different_shape = TagArgumentForm::new(vec![pattern(
            "mode",
            TagArgumentPatternKind::Literal("unsafe".to_string()),
        )])
        .expect("one fixed-width atom is valid");

        assert!(!form.merge_messages_if_same_shape(&different_shape));
        assert_eq!(form, before_different_shape);
    }

    #[test]
    fn variable_width_form_aligns_fixed_suffix_from_end() {
        let form = TagArgumentForm::new(vec![
            pattern(
                "loopvars",
                TagArgumentPatternKind::VariableWidth { minimum: 1 },
            ),
            pattern("in", TagArgumentPatternKind::Literal("in".to_string())),
            pattern(
                "sequence",
                TagArgumentPatternKind::VariableExcept("reversed".to_string()),
            ),
        ])
        .expect("one variable-width section is valid");

        assert_eq!(form.match_full(&["x,", "y", "in", "items"]), Ok(()));
        assert!(matches!(
            form.match_full(&["x", "in", "reversed"]),
            Err(TagArgumentFormMismatch::Atom(FormAtomMismatch {
                argument_index: 2,
                expected: FormAtomExpectation::Excluded("reversed"),
                message: None,
            }))
        ));
    }

    #[test]
    fn variable_width_prefix_offers_repeat_and_suffix() {
        let form = TagArgumentForm::new(vec![
            pattern(
                "loopvars",
                TagArgumentPatternKind::VariableWidth { minimum: 1 },
            ),
            pattern("in", TagArgumentPatternKind::Literal("in".to_string())),
            pattern("sequence", TagArgumentPatternKind::Variable),
        ])
        .expect("one variable-width section is valid");

        let initial = form.prefix_continuations::<&str>(&[]);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].argument.name, "loopvars");

        let after_one = form.prefix_continuations(&["x"]);
        assert_eq!(
            after_one
                .iter()
                .map(|continuation| continuation.argument.name.as_str())
                .collect::<Vec<_>>(),
            vec!["loopvars", "in"]
        );
        let after_suffix = form.prefix_continuations(&["x", "in"]);
        assert_eq!(
            after_suffix
                .iter()
                .map(|continuation| continuation.argument.name.as_str())
                .collect::<Vec<_>>(),
            vec!["loopvars", "in", "sequence"]
        );
    }
}
