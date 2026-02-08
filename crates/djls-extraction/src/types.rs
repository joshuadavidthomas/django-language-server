use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;

/// Identifies a specific tag or filter registration within a module.
///
/// Keyed by both the registration module path and the symbol name to avoid
/// collisions when different libraries register identically-named symbols.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey {
    pub registration_module: String,
    pub name: String,
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

/// Whether a symbol is a template tag or a template filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Tag,
    Filter,
}

/// Result of extracting rules from a Python registration module.
///
/// Maps each discovered symbol to its extracted validation rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub tag_rules: FxHashMap<SymbolKey, TagRule>,
    pub filter_arities: FxHashMap<SymbolKey, FilterArity>,
    pub block_specs: FxHashMap<SymbolKey, BlockTagSpec>,
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

    /// Replace the `registration_module` in all `SymbolKey`s.
    ///
    /// Used when extraction was performed without a known module path
    /// (e.g., from a Salsa tracked function that only has a `File` input),
    /// and the caller needs to stamp in the correct module path afterwards.
    pub fn rekey_module(&mut self, module_path: &str) {
        fn rekey_map<V>(map: &mut FxHashMap<SymbolKey, V>, module_path: &str) {
            let original_len = map.len();
            let entries: Vec<(SymbolKey, V)> = map
                .drain()
                .map(|(mut k, v)| {
                    k.registration_module = module_path.to_string();
                    (k, v)
                })
                .collect();
            map.extend(entries);
            debug_assert_eq!(
                map.len(),
                original_len,
                "rekey_module produced duplicate keys for module '{module_path}' — data was lost"
            );
        }

        rekey_map(&mut self.tag_rules, module_path);
        rekey_map(&mut self.filter_arities, module_path);
        rekey_map(&mut self.block_specs, module_path);
    }
}

/// Validation rules extracted from a tag's compile function.
///
/// Captures the conditions under which `TemplateSyntaxError` is raised,
/// expressed as structured constraints on token count, keyword positions,
/// and option values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TagRule {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
    pub choice_at_constraints: Vec<ChoiceAt>,
    pub known_options: Option<KnownOptions>,
    pub extracted_args: Vec<ExtractedArg>,
    /// Whether this tag supports Django's `{% tag args... as varname %}` syntax.
    ///
    /// When true, the evaluator strips trailing `as <varname>` from the argument
    /// list before checking constraints. This is set for `simple_tag` registrations
    /// where Django's framework handles the `as` syntax automatically.
    #[serde(default)]
    pub supports_as_var: bool,
}

impl TagRule {
    /// Returns `true` if this rule contains any meaningful constraints or arguments.
    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.arg_constraints.is_empty()
            || !self.required_keywords.is_empty()
            || !self.choice_at_constraints.is_empty()
            || self.known_options.is_some()
            || !self.extracted_args.is_empty()
    }
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

/// A keyword that must appear at a specific position in the argument list.
///
/// For example, `{% cycle ... as name %}` requires `"as"` at a specific position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequiredKeyword {
    pub position: i64,
    pub value: String,
}

/// A constraint that a specific position must hold one of a fixed set of values.
///
/// For example, `{% autoescape on %}` requires `args[1]` to be `"on"` or `"off"`.
/// Extracted from patterns like `if arg not in ("on", "off"): raise TemplateSyntaxError(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAt {
    pub position: i64,
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
pub struct BlockTagSpec {
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedArg {
    /// Argument name (from parameter name or AST analysis, or generic `arg1`/`arg2`)
    pub name: String,
    /// Whether this argument is required (no default value)
    pub required: bool,
    /// The kind of argument
    pub kind: ExtractedArgKind,
    /// Zero-based position index in the argument list (excluding tag name)
    pub position: usize,
}

/// The kind of an extracted argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtractedArgKind {
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
        assert_eq!(key.kind, SymbolKind::Tag);
    }

    #[test]
    fn symbol_key_filter_constructor() {
        let key = SymbolKey::filter("django.template.defaultfilters", "title");
        assert_eq!(key.registration_module, "django.template.defaultfilters");
        assert_eq!(key.name, "title");
        assert_eq!(key.kind, SymbolKind::Filter);
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
            },
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
            },
        );

        let mut result2 = ExtractionResult::default();
        result2.tag_rules.insert(
            key.clone(),
            TagRule {
                arg_constraints: vec![ArgumentCountConstraint::Min(2)],
                ..Default::default()
            },
        );

        result1.merge(result2);
        assert_eq!(result1.tag_rules.len(), 1);

        let rule = result1.tag_rules.get(&key).unwrap();
        assert_eq!(rule.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn block_tag_spec_opaque() {
        let spec = BlockTagSpec {
            end_tag: Some("endverbatim".to_string()),
            intermediates: vec![],
            opaque: true,
        };
        assert!(spec.opaque);
        assert_eq!(spec.end_tag.as_deref(), Some("endverbatim"));
    }

    #[test]
    fn block_tag_spec_with_intermediates() {
        let spec = BlockTagSpec {
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
