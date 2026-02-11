use djls_source::Span;

use crate::filters::Filter;
use crate::nodelist::Node;
use crate::parser::ParseError;

/// Trait for visiting nodes in a Django template AST.
pub trait Visitor {
    fn visit_node(&mut self, node: &Node) {
        walk_node(self, node);
    }

    fn visit_tag(&mut self, _name: &str, _bits: &[String], _span: Span) {}
    fn visit_variable(&mut self, _var: &str, _filters: &[Filter], _span: Span) {}
    fn visit_comment(&mut self, _content: &str, _span: Span) {}
    fn visit_text(&mut self, _span: Span) {}
    fn visit_error(&mut self, _span: Span, _full_span: Span, _error: &ParseError) {}
}

/// Recursively walk a single node, calling the appropriate visitor methods.
pub fn walk_node<V: Visitor + ?Sized>(visitor: &mut V, node: &Node) {
    match node {
        Node::Tag { name, bits, span } => visitor.visit_tag(name, bits, *span),
        Node::Variable { var, filters, span } => visitor.visit_variable(var, filters, *span),
        Node::Comment { content, span } => visitor.visit_comment(content, *span),
        Node::Text { span } => visitor.visit_text(*span),
        Node::Error {
            span,
            full_span,
            error,
        } => visitor.visit_error(*span, *full_span, error),
    }
}

/// Walk a list of nodes, visiting each one in sequence.
pub fn walk_nodelist<V: Visitor + ?Sized>(visitor: &mut V, nodes: &[Node]) {
    for node in nodes {
        visitor.visit_node(node);
    }
}
