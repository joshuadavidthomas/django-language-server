//! Tag meaning beyond structure and argument rules.
//!
//! Values here are built only inside this crate, from extraction or from a Fallback Rule in
//! [`crate::templates::tags::fallback`], so downstream crates consume Django facts without being
//! able to author them.

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::templates::SymbolKey;
use crate::templates::tags::types::ExtractedMessageTemplate;

pub type TagMeaningMap = FxHashMap<SymbolKey, TagMeaning>;

/// Durable meaning of a Tag Definition that downstream features use instead of tag names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagRole {
    TemplateReference(TemplateReferenceKind),
    TemplateLibraryLoader,
    TemplateBlock,
    /// Defines a Template Partial.
    TemplatePartial,
    /// Renders a Template Partial defined elsewhere.
    TemplatePartialReference,
    StaticAssetReference,
    RouteReference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateReferenceKind {
    Extends,
    Include,
}

/// Where a Tag fact came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactProvenance {
    Extracted,
    Fallback,
}

/// A Fallback Rule that contributed to a [`TagMeaning`], with its Extraction Gap and evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FallbackCitation {
    gap: &'static str,
    /// Path inside the bundled Django 5.2 archive.
    path: &'static str,
    line: usize,
}

impl FallbackCitation {
    pub(crate) const fn new(gap: &'static str, path: &'static str, line: usize) -> Self {
        Self { gap, path, line }
    }
}

/// How Django's Lexer tokenizes the body of a block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyLexing {
    #[default]
    Template,
    /// The body is text until a tag whose contents equal `end` followed by the opener's contents.
    Verbatim,
}

/// Tag Bits that Django parses as an `if` condition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct ConditionArguments {
    /// The opener's own bits are always a condition; these intermediates carry one too.
    intermediates: Vec<String>,
}

impl ConditionArguments {
    pub(crate) fn new(intermediates: Vec<String>) -> Self {
        Self { intermediates }
    }

    #[must_use]
    pub fn intermediates(&self) -> &[String] {
        &self.intermediates
    }
}

/// Per-template state on Django's `Parser` where a compile function records names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRegistry {
    /// An attribute of the parser, such as `parser.__loaded_blocks`.
    ParserAttribute(String),
    /// A key of `parser.extra_data`.
    ParserExtraData(String),
}

/// An opener argument that names something once per template: the compile function records it
/// in a [`NameRegistry`] and raises when the name is already there.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct UniqueName {
    /// Split position of the argument; 0 is the tag name.
    position: usize,
    registry: NameRegistry,
    /// The message the compile function raises for a repeated name.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<ExtractedMessageTemplate>,
}

impl UniqueName {
    pub(crate) fn new(
        position: usize,
        registry: NameRegistry,
        message: Option<ExtractedMessageTemplate>,
    ) -> Self {
        Self {
            position,
            registry,
            message,
        }
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    /// Tags that share a registry reject each other's names.
    #[must_use]
    pub fn registry(&self) -> &NameRegistry {
        &self.registry
    }

    #[must_use]
    pub fn message(&self) -> Option<&ExtractedMessageTemplate> {
        self.message.as_ref()
    }
}

/// Meaning of one Tag Definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct TagMeaning {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<(TagRole, FactProvenance)>,
    #[serde(skip_serializing_if = "is_template_lexing")]
    body_lexing: BodyLexing,
    #[serde(skip_serializing_if = "Option::is_none")]
    condition: Option<ConditionArguments>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unique_name: Option<UniqueName>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fallbacks: Vec<FallbackCitation>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes fields by reference"
)]
fn is_template_lexing(lexing: &BodyLexing) -> bool {
    *lexing == BodyLexing::Template
}

impl TagMeaning {
    #[must_use]
    pub fn role(&self) -> Option<TagRole> {
        self.role.map(|(role, _)| role)
    }

    #[must_use]
    pub fn role_provenance(&self) -> Option<FactProvenance> {
        self.role.map(|(_, provenance)| provenance)
    }

    /// Always a Fallback Rule: the Lexer is not a compile function extraction can read.
    #[must_use]
    pub fn body_lexing(&self) -> BodyLexing {
        self.body_lexing
    }

    /// Always a Fallback Rule: extraction does not follow `TemplateIfParser`, so it records no
    /// Condition consumption.
    #[must_use]
    pub fn condition(&self) -> Option<&ConditionArguments> {
        self.condition.as_ref()
    }

    /// Always extracted from the compile function's membership check.
    #[must_use]
    pub fn unique_name(&self) -> Option<&UniqueName> {
        self.unique_name.as_ref()
    }

    /// Fallback Rules that contributed facts to this meaning.
    #[must_use]
    pub fn fallbacks(&self) -> &[FallbackCitation] {
        &self.fallbacks
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn set_role(&mut self, role: TagRole, provenance: FactProvenance) {
        self.role = Some((role, provenance));
    }

    pub(crate) fn set_body_lexing(&mut self, lexing: BodyLexing) {
        self.body_lexing = lexing;
    }

    pub(crate) fn set_condition(&mut self, condition: ConditionArguments) {
        self.condition = Some(condition);
    }

    pub(crate) fn set_unique_name(&mut self, unique_name: UniqueName) {
        self.unique_name = Some(unique_name);
    }

    pub(crate) fn cite_fallback(&mut self, citation: FallbackCitation) {
        self.fallbacks.push(citation);
    }
}
