use djls_source::Span;

use crate::templatetags::TagSpec;

/// Stable identifier for a semantic node (tag)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemanticId(pub(crate) u32);

impl SemanticId {
    pub(crate) fn new(id: u32) -> Self {
        Self(id)
    }
}

/// Stable identifier for a segment within a tag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SegmentId {
    pub semantic_id: SemanticId,
    pub segment_index: u32,
}

impl SegmentId {
    pub(crate) fn new(semantic_id: SemanticId, segment_index: u32) -> Self {
        Self {
            semantic_id,
            segment_index,
        }
    }
}

/// Element found at a position
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticElement {
    Tag {
        id: SemanticId,
        name: String,
        span: Span,
        arguments: Vec<String>,
    },
    Segment {
        id: SegmentId,
        tag_name: String,
        segment_name: Option<String>,
        span: Span,
    },
    Variable {
        name: String,
        span: Span,
    },
    Text {
        content: String,
        span: Span,
    },
    None,
}

/// Reference to a tag for hover/goto operations
#[derive(Debug, Clone, PartialEq)]
pub struct TagReference {
    pub id: SemanticId,
    pub name: String,
    pub opening_span: Span,
    pub closing_span: Option<Span>,
    pub arguments: Vec<String>,
    pub spec: Option<TagSpec>,
}

/// Variable reference information
#[derive(Debug, Clone, PartialEq)]
pub struct VariableReference {
    pub name: String,
    pub span: Span,
}

/// Variable information for hover/goto
#[derive(Debug, Clone, PartialEq)]
pub struct VariableInfo {
    pub name: String,
    pub span: Span,
    pub definition_span: Option<Span>,
}

/// Block definition for template inheritance
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDefinition {
    pub name: String,
    pub span: Span,
}

/// Template dependency (extends/includes)
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateDependency {
    Extends { path: String, span: Span },
    Include { path: String, span: Span },
}

/// Internal element ID for offset index
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ElementId {
    Tag(SemanticId),
    Segment(SegmentId),
    Variable(u32),
    Text(u32),
}