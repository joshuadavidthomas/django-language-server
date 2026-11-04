//! Semantic coordinates omit the widths of presentation-only regions. Within a tag or
//! variable they retain byte offsets, including gaps used by closing-tag validation.
//! Only this module translates those coordinates back to the current source.

use djls_source::Span;
use djls_templates::Filter;
use djls_templates::Node;
use djls_templates::NodeList;
use djls_templates::TagBit;

use crate::db::Db;
use crate::errors::ValidationError;

#[salsa::tracked]
pub(crate) struct TemplateLayout<'db> {
    #[returns(copy)]
    pub(crate) nodes: NodeList<'db>,
    #[tracked]
    #[returns(ref)]
    pub(crate) positions: SourcePositions,
}

/// Ordered anchors between semantic coordinates and source bytes. Ordinary syntax keeps
/// its width; a text run occupies one semantic byte regardless of its source length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourcePositions(Vec<(u32, u32)>);

impl SourcePositions {
    fn anchor(&mut self, semantic: u32, source: u32) {
        // Syntax keeps its width between anchors. Tag-only files need a single anchor,
        // rather than a growing position table on the deep-nesting path.
        if self.0.last().is_none_or(|&(previous, previous_source)| {
            previous_source + (semantic - previous) != source
        }) {
            self.0.push((semantic, source));
        }
    }

    fn offset(&self, position: u32) -> u32 {
        let index = self
            .0
            .partition_point(|&(semantic, _)| semantic <= position);
        let (semantic, source) = self.0[index.saturating_sub(1)];
        source + (position - semantic)
    }

    /// The inverse of `offset`. A source position inside a text run or comment body maps
    /// into that run's single semantic byte, so it never lands on a neighbouring node.
    pub(crate) fn semantic_offset(&self, source: u32) -> u32 {
        let index = self.0.partition_point(|&(_, anchor)| anchor <= source);
        let (semantic, anchor) = self.0[index.saturating_sub(1)];
        let position = semantic + (source - anchor);
        match self.0.get(index) {
            Some(&(next, _)) if position >= next => next.saturating_sub(1).max(semantic),
            _ => position,
        }
    }

    pub(crate) fn span(&self, span: Span) -> Span {
        let start = self.offset(span.start());
        let end = self.offset(span.end());
        Span::new(start, end - start)
    }

    pub(crate) fn error(&self, mut error: ValidationError) -> ValidationError {
        error.map_spans(|span| self.span(span));
        error
    }
}

#[salsa::tracked(returns(copy))]
pub(crate) fn template_layout<'db>(db: &'db dyn Db, source: NodeList<'db>) -> TemplateLayout<'db> {
    let source_nodes = source.nodelist(db);
    let mut nodes = Vec::with_capacity(source_nodes.len());
    let mut positions = SourcePositions(Vec::with_capacity(source_nodes.len() + 1));
    positions.anchor(0, 0);
    let mut next = 0;
    for source_node in source_nodes {
        let full_span = source_node.full_span();
        let start = next;
        positions.anchor(start, full_span.start());
        let node = match source_node {
            // Nothing downstream of the layout reads text, so a run keeps only its anchors.
            Node::Text { .. } => {
                next += 1;
                None
            }
            Node::Comment { span, .. } => {
                positions.anchor(start + 2, span.start());
                positions.anchor(start + 3, span.end());
                next += 5;
                Some(Node::Comment {
                    span: Span::new(start + 2, 1),
                })
            }
            Node::Tag { .. } | Node::Variable { .. } | Node::Error { .. } => {
                let mut node = source_node.clone();
                map_node_spans(&mut node, |span| {
                    Span::new(start + span.start() - full_span.start(), span.length())
                });
                if let Node::Error { error, .. } = &mut node {
                    use djls_templates::ParseError;

                    let (ParseError::EmptyTag { position }
                    | ParseError::MalformedConstruct { position, .. }
                    | ParseError::MalformedFilterExpression { position, .. }) = error;
                    *position = start as usize + (*position - full_span.start_usize());
                }
                next += full_span.length();
                Some(node)
            }
        };
        positions.anchor(next, full_span.end());
        nodes.extend(node);
    }
    // Both are reserved for every source node, but text runs add no node and contiguous
    // syntax shares anchors. They are kept for the life of the layout.
    nodes.shrink_to_fit();
    positions.0.shrink_to_fit();
    TemplateLayout::new(db, NodeList::new(db, nodes), positions)
}

fn map_bits(bits: &mut [TagBit], mut map: impl FnMut(Span) -> Span) {
    for bit in bits {
        bit.span = map(bit.span);
    }
}

fn map_filters(filters: &mut [Filter], mut map: impl FnMut(Span) -> Span) {
    for filter in filters {
        filter.span = map(filter.span);
        if let Some(argument) = &mut filter.arg {
            argument.span = map(argument.span);
        }
    }
}

fn map_node_spans(node: &mut Node, mut map: impl FnMut(Span) -> Span) {
    match node {
        Node::Tag {
            name_span,
            bits,
            span,
            ..
        } => {
            *name_span = map(*name_span);
            map_bits(bits, &mut map);
            *span = map(*span);
        }
        Node::Variable {
            var_span,
            filters,
            span,
            ..
        } => {
            *var_span = map(*var_span);
            map_filters(filters, &mut map);
            *span = map(*span);
        }
        Node::Error {
            span, full_span, ..
        } => {
            *span = map(*span);
            *full_span = map(*full_span);
        }
        Node::Comment { span, .. } | Node::Text { span } => *span = map(*span),
    }
}
