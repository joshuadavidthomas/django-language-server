use super::ast::{Ast, AstError, Block, Node, Span, Tag};

pub struct Validator<'a> {
    ast: &'a Ast,
    errors: Vec<AstError>,
}

impl<'a> Validator<'a> {
    pub fn new(ast: &'a Ast) -> Self {
        Self {
            ast,
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
        match block {
            Block::Container { tag, nodes, closing } => {
                self.validate_container(tag, nodes, closing);
            }
            Block::Branch { tag, nodes } => {
                self.validate_branch(tag, nodes);
            }
            _ => {}
        }
    }

    fn validate_container(&mut self, tag: &Tag, nodes: &[Node], closing: &Option<Box<Block>>) {
        match tag.name.as_str() {
            "if" => {
                if closing.is_none() {
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: tag.name.clone(),
                        expected_closing: "endif".to_string(),
                        opening_span: tag.span,
                        closing_span: None,
                    });
                }
            }
            "for" => {
                if closing.is_none() {
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: tag.name.clone(),
                        expected_closing: "endfor".to_string(),
                        opening_span: tag.span,
                        closing_span: None,
                    });
                }
            }
            _ => {}
        }

        self.validate_nodes(nodes);
    }

    fn validate_branch(&mut self, tag: &Tag, nodes: &[Node]) {
        match tag.name.as_str() {
            "elif" | "else" => {
                // We can check parent context by walking up the AST
                if !self.has_parent_if(tag) {
                    self.errors.push(AstError::InvalidTagStructure {
                        tag: tag.name.clone(),
                        reason: format!("{} without preceding if", tag.name),
                        span: tag.span,
                    });
                }
            }
            _ => {}
        }

        self.validate_nodes(nodes);
    }

    fn has_parent_if(&self, tag: &Tag) -> bool {
        // Implementation would walk up the AST to find parent if block
        // This is just a placeholder implementation
        true
    }
}
