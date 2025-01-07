use super::ast::{Ast, AstError, Block, Node, Span, Tag};
use crate::tagspecs::{TagSpec, TagSpecs, TagType};

pub struct Validator<'a> {
    ast: &'a Ast,
    tags: &'a TagSpecs,
    errors: Vec<AstError>,
}

impl<'a> Validator<'a> {
    pub fn new(ast: &'a Ast, tags: &'a TagSpecs) -> Self {
        Self {
            ast,
            tags,
            errors: Vec::new(),
        }
    }

    pub fn validate(&mut self) -> Vec<AstError> {
        if self.ast.nodes().is_empty() {
            self.errors.push(AstError::EmptyAst);
            return self.errors;
        }

        self.validate_nodes(self.ast.nodes());
        self.errors
    }

    fn validate_nodes(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Block(block) => self.validate_block(block),
                _ => {}
            }
        }
    }

    fn validate_block(&mut self, block: &Block) {
        let tag = block.tag();
        if let Some(spec) = self.tags.get(&tag.name) {
            match block {
                Block::Container { tag, nodes, closing } => {
                    self.validate_container(tag, nodes, closing, spec);
                }
                Block::Branch { tag, nodes } => {
                    self.validate_branch(tag, nodes, spec);
                }
                _ => {}
            }
        }
    }

    fn validate_container(
        &mut self,
        tag: &Tag,
        nodes: &[Node],
        closing: &Option<Box<Block>>,
        spec: &TagSpec,
    ) {
        // Check for required closing tag
        if spec.tag_type == TagType::Container && closing.is_none() {
            if let Some(expected_closing) = &spec.closing {
                self.errors.push(AstError::UnbalancedStructure {
                    opening_tag: tag.name.clone(),
                    expected_closing: expected_closing.clone(),
                    opening_span: tag.span,
                    closing_span: None,
                });
            }
        }

        // Validate child nodes
        self.validate_nodes(nodes);
    }

    fn validate_branch(&mut self, tag: &Tag, nodes: &[Node], spec: &TagSpec) {
        // Check if branch is valid for parent tag
        if let Some(branches) = &spec.branches {
            if !branches.contains(&tag.name) {
                self.errors.push(AstError::InvalidTagStructure {
                    tag: tag.name.clone(),
                    reason: format!("{} is not a valid branch for parent tag", tag.name),
                    span: tag.span,
                });
            }
        }

        // Validate child nodes
        self.validate_nodes(nodes);
    }
}
