//! Tag meaning beyond structure and argument rules.
//!
//! Values here are built only inside this crate, from extraction or from a Fallback Rule in
//! [`crate::templates::tags::fallback`], so downstream crates consume Django facts without being
//! able to author them.

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::templates::SymbolKey;

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

/// Meaning of one Tag Definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
pub struct TagMeaning {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<(TagRole, FactProvenance)>,
    #[serde(skip_serializing_if = "is_template_lexing")]
    body_lexing: BodyLexing,
    #[serde(skip_serializing_if = "Option::is_none")]
    condition: Option<ConditionArguments>,
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

    /// Always a Fallback Rule until extraction records argument consumption.
    #[must_use]
    pub fn condition(&self) -> Option<&ConditionArguments> {
        self.condition.as_ref()
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

    pub(crate) fn cite_fallback(&mut self, citation: FallbackCitation) {
        self.fallbacks.push(citation);
    }
}
