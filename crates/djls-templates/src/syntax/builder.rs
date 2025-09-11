use serde::Serialize;
use thiserror::Error;

use crate::ast::Node;
use crate::ast::Span;
use crate::db::Db as TemplateDb;
use crate::syntax::meta::TagMeta;
use crate::syntax::meta::TagShape;
use crate::syntax::tree::CommentNode;
use crate::syntax::tree::FilterName;
use crate::syntax::tree::SyntaxNode;
use crate::syntax::tree::SyntaxNodeId;
use crate::syntax::tree::TagName;
use crate::syntax::tree::TagNode;
use crate::syntax::tree::TextNode;
use crate::syntax::tree::VariableName;
use crate::syntax::tree::VariableNode;
use crate::templatetags::TagSpecs;
use crate::templatetags::TagType;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum StructuralError {
    #[error("Unclosed block '{tag}' starting at position {opener_span:?}")]
    UnclosedBlock { tag: String, opener_span: Span },
    
    #[error("Orphaned intermediate tag '{tag}' at position {span:?} - valid parents: {valid_parents:?}")]
    OrphanedIntermediate {
        tag: String,
        span: Span,
        valid_parents: Vec<String>,
    },
    
    #[error("Mismatched closer: expected '{expected}', found '{found}' (opener at {opener_span:?}, closer at {closer_span:?})")]
    MismatchedCloser {
        expected: String,
        found: String,
        opener_span: Span,
        closer_span: Span,
    },
    
    #[error("Unexpected closer '{tag}' at position {span:?} with no matching opener")]
    UnexpectedCloser { tag: String, span: Span },
    
    #[error("Duplicate branch '{tag}' at position {span:?} (first occurrence at {first_span:?})")]
    DuplicateBranch {
        tag: String,
        span: Span,
        first_span: Span,
    },
}

pub struct TreeBuilder<'db> {
    db: &'db dyn TemplateDb,
    tag_specs: std::sync::Arc<TagSpecs>,
    stack: Vec<BlockFrame<'db>>,
    root_children: Vec<SyntaxNodeId<'db>>,
    raw_mode: Option<String>, // If Some, contains the expected closer tag name
}

/// Represents an open block tag and its accumulated children
struct BlockFrame<'db> {
    tag_node: TagNode<'db>,
    children: Vec<SyntaxNodeId<'db>>,
    branches: Vec<BranchFrame<'db>>,
    current_branch: Option<BranchFrame<'db>>,
}

/// Represents a branch within a block (elif, else, empty)
struct BranchFrame<'db> {
    tag_node: Option<TagNode<'db>>, // None for implicit first branch
    children: Vec<SyntaxNodeId<'db>>,
}

impl<'db> TreeBuilder<'db> {
    pub fn new(db: &'db dyn TemplateDb) -> Self {
        Self {
            db,
            tag_specs: db.tag_specs(),
            stack: Vec::new(),
            root_children: Vec::new(),
            raw_mode: None,
        }
    }

    pub fn add_node(&mut self, node: Node<'db>) {
        // Handle raw mode processing
        if self.handle_raw_mode(&node) {
            return;
        }

        let syntax_node = match node {
            Node::Text(text_node) => SyntaxNode::Text(TextNode {
                content: text_node.content.clone(),
                span: text_node.span,
            }),
            Node::Variable(var_node) => SyntaxNode::Variable(VariableNode {
                var: VariableName::new(self.db, var_node.var.text(self.db).to_string()),
                filters: var_node
                    .filters
                    .iter()
                    .map(|f| FilterName::new(self.db, f.text(self.db).to_string()))
                    .collect(),
                span: var_node.span,
            }),
            Node::Comment(comment_node) => SyntaxNode::Comment(CommentNode {
                content: comment_node.content.clone(),
                span: comment_node.span,
            }),
            Node::Tag(tag_node) => {
                let name_str = tag_node.name.text(self.db);
                let tag_type = TagType::for_name(&name_str, &self.tag_specs);

                let meta = TagMeta::from_tag(self.db, &name_str, &tag_node.bits, &self.tag_specs);

                let syntax_tag_node = TagNode {
                    name: TagName::new(self.db, name_str.to_string()),
                    bits: tag_node.bits.clone(),
                    span: tag_node.span,
                    meta,
                    children: Vec::new(), // Will be populated by tree building
                };

                match tag_type {
                    TagType::Opener => {
                        self.handle_opener(syntax_tag_node);
                        return;
                    }
                    TagType::Intermediate => {
                        self.handle_intermediate(syntax_tag_node);
                        return;
                    }
                    TagType::Closer => {
                        self.handle_closer(syntax_tag_node);
                        return;
                    }
                    TagType::Standalone => {
                        // Standalone tags are added as regular nodes
                    }
                }

                SyntaxNode::Tag(syntax_tag_node)
            }
        };

        let node_id = SyntaxNodeId::new(self.db, syntax_node);
        self.add_to_current_context(node_id);
    }

    fn handle_opener(&mut self, tag_node: TagNode<'db>) {
        // Check if this is a raw block
        if let TagShape::RawBlock { ender } = &tag_node.meta.shape {
            // Enter raw mode - content will be accumulated without structural parsing
            self.raw_mode = Some(ender.clone());
        }

        let frame = BlockFrame {
            tag_node,
            children: Vec::new(),
            branches: Vec::new(),
            current_branch: Some(BranchFrame {
                tag_node: None, // First branch is implicit
                children: Vec::new(),
            }),
        };
        self.stack.push(frame);
    }

    /// Handle nodes when in raw mode. Returns true if the node was handled in raw mode.
    fn handle_raw_mode(&mut self, node: &Node<'db>) -> bool {
        if let Some(expected_closer) = &self.raw_mode.clone() {
            if let Node::Tag(tag_node) = node {
                let name_str = tag_node.name.text(self.db);
                if name_str == *expected_closer {
                    // Found the closer, exit raw mode and continue normal processing
                    self.raw_mode = None;
                    return false;
                }
            }

            // We're in raw mode and this isn't the closer, so add as raw content
            let syntax_node = match node {
                Node::Text(text_node) => SyntaxNode::Text(TextNode {
                    content: text_node.content.clone(),
                    span: text_node.span,
                }),
                Node::Variable(var_node) => SyntaxNode::Variable(VariableNode {
                    var: VariableName::new(self.db, var_node.var.text(self.db).to_string()),
                    filters: var_node
                        .filters
                        .iter()
                        .map(|f| FilterName::new(self.db, f.text(self.db).to_string()))
                        .collect(),
                    span: var_node.span,
                }),
                Node::Comment(comment_node) => SyntaxNode::Comment(CommentNode {
                    content: comment_node.content.clone(),
                    span: comment_node.span,
                }),
                Node::Tag(tag_node) => {
                    // In raw mode, treat all tags as regular text-like nodes
                    let name_str = tag_node.name.text(self.db);
                    let meta =
                        TagMeta::from_tag(self.db, &name_str, &tag_node.bits, &self.tag_specs);
                    SyntaxNode::Tag(TagNode {
                        name: TagName::new(self.db, name_str.to_string()),
                        bits: tag_node.bits.clone(),
                        span: tag_node.span,
                        meta,
                        children: Vec::new(),
                    })
                }
            };
            let node_id = SyntaxNodeId::new(self.db, syntax_node);
            self.add_to_current_context(node_id);
            true
        } else {
            false
        }
    }

    fn handle_intermediate(&mut self, tag_node: TagNode<'db>) {
        if let Some(frame) = self.stack.last_mut() {
            // Close current branch and start new one
            if let Some(current_branch) = frame.current_branch.take() {
                frame.branches.push(current_branch);
            }

            frame.current_branch = Some(BranchFrame {
                tag_node: Some(tag_node),
                children: Vec::new(),
            });
        } else {
            // Orphaned intermediate tag - add as regular node
            let node_id = SyntaxNodeId::new(self.db, SyntaxNode::Tag(tag_node));
            self.add_to_current_context(node_id);
        }
    }

    fn handle_closer(&mut self, closer_tag: TagNode<'db>) {
        let closer_name = closer_tag.name.text(self.db);

        // Find matching opener
        let expected_opener = self.tag_specs.find_opener_for_closer(&closer_name);

        if let Some(opener_name) = expected_opener {
            if let Some(frame_index) = self.find_matching_frame(&opener_name) {
                let mut frame = self.stack.remove(frame_index);

                // Add current branch to branches list
                if let Some(current_branch) = frame.current_branch.take() {
                    frame.branches.push(current_branch);
                }

                // Build hierarchical children from branches
                let mut all_children = Vec::new();
                for branch in frame.branches {
                    if let Some(branch_tag) = branch.tag_node {
                        // Add intermediate tag as a node
                        let branch_node_id =
                            SyntaxNodeId::new(self.db, SyntaxNode::Tag(branch_tag));
                        all_children.push(branch_node_id);
                    }
                    // Add branch children
                    all_children.extend(branch.children);
                }

                // Create the block tag with its children
                let block_tag = TagNode {
                    name: frame.tag_node.name,
                    bits: frame.tag_node.bits,
                    span: frame.tag_node.span,
                    meta: frame.tag_node.meta,
                    children: all_children,
                };

                let block_node_id = SyntaxNodeId::new(self.db, SyntaxNode::Tag(block_tag));

                self.add_to_current_context(block_node_id);
                return;
            }
        }

        // No matching opener found - add closer as regular node
        let node_id = SyntaxNodeId::new(self.db, SyntaxNode::Tag(closer_tag));
        self.add_to_current_context(node_id);
    }

    fn find_matching_frame(&self, opener_name: &str) -> Option<usize> {
        self.stack
            .iter()
            .enumerate()
            .rev()
            .find(|(_, frame)| frame.tag_node.name.text(self.db) == opener_name)
            .map(|(i, _)| i)
    }

    fn add_to_current_context(&mut self, node_id: SyntaxNodeId<'db>) {
        if let Some(frame) = self.stack.last_mut() {
            if let Some(current_branch) = &mut frame.current_branch {
                current_branch.children.push(node_id);
            } else {
                frame.children.push(node_id);
            }
        } else {
            self.root_children.push(node_id);
        }
    }

    pub fn finish(mut self) -> Vec<SyntaxNodeId<'db>> {
        // Handle any unclosed blocks
        while let Some(frame) = self.stack.pop() {
            // Add current branch to branches
            let mut frame = frame;
            if let Some(current_branch) = frame.current_branch.take() {
                frame.branches.push(current_branch);
            }

            // Build partial block with available children
            let mut all_children = Vec::new();
            for branch in frame.branches {
                if let Some(branch_tag) = branch.tag_node {
                    let branch_node_id = SyntaxNodeId::new(self.db, SyntaxNode::Tag(branch_tag));
                    all_children.push(branch_node_id);
                }
                all_children.extend(branch.children);
            }

            // Mark this block as unclosed since we're handling it in finish()
            let mut unclosed_meta = frame.tag_node.meta.clone();
            unclosed_meta.unclosed = true;

            let block_tag = TagNode {
                name: frame.tag_node.name,
                bits: frame.tag_node.bits,
                span: frame.tag_node.span,
                meta: unclosed_meta,
                children: all_children,
            };

            let block_node_id = SyntaxNodeId::new(self.db, SyntaxNode::Tag(block_tag));

            self.root_children.push(block_node_id);
        }

        self.root_children
    }
}
