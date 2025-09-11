//! Django template parsing, validation, and diagnostics.
//!
//! This crate provides comprehensive support for Django template files including:
//! - Lexical analysis and tokenization
//! - Parsing into an Abstract Syntax Tree (AST)
//! - Validation using configurable tag specifications
//! - LSP diagnostic generation with Salsa integration
//!
//! ## Architecture
//!
//! The system uses a multi-stage pipeline:
//!
//! 1. **Lexing**: Template text is tokenized into Django constructs (tags, variables, text)
//! 2. **Parsing**: Tokens are parsed into a structured AST
//! 3. **Validation**: The AST is validated using the visitor pattern
//! 4. **Diagnostics**: Errors are converted to LSP diagnostics via Salsa accumulators
//!
//! ## Key Components
//!
//! - [`ast`]: AST node definitions and visitor pattern implementation
//! - [`db`]: Salsa database integration for incremental computation
//! - [`validation`]: Validation rules using the visitor pattern
//! - [`tagspecs`]: Django tag specifications for validation
//!
//! ## Adding New Validation Rules
//!
//! 1. Add the error variant to [`TemplateError`]
//! 2. Implement the check in the validation module
//! 3. Add corresponding tests
//!
//! ## Migration from `NodeList` to `SyntaxTree`
//!
//! The library is transitioning from flat `NodeList` to hierarchical `SyntaxTree` representation:
//!
//! - **Current (`NodeList`)**: `analyze_template()` returns `Option<NodeList>`
//! - **New (`SyntaxTree`)**: `analyze_syntax_tree()` returns `Option<SyntaxTree>`
//!
//! ### Migration Path:
//!
//! 1. **Phase 1**: Both functions are available - use `analyze_syntax_tree()` for new code
//! 2. **Phase 2**: `analyze_template()` will be updated to return `SyntaxTree` instead of `NodeList`
//! 3. **Phase 3**: `NodeList` becomes internal-only, `SyntaxTree` becomes the primary API
//!

//!
//! ## Example
//!
//! ```ignore
//! // For LSP integration with Salsa (hierarchical, preferred):
//! use djls_templates::db::{analyze_syntax_tree, TemplateDiagnostic};
//!
//! let syntax_tree = analyze_syntax_tree(db, file);
//! let diagnostics = analyze_syntax_tree::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // Legacy flat NodeList (deprecated):
//! use djls_templates::db::{analyze_template, TemplateDiagnostic};
//!
//! let ast = analyze_template(db, file);
//! let diagnostics = analyze_template::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // For direct parsing (testing/debugging):
//! use djls_templates::{Lexer, Parser, build_syntax_tree};
//!
//! let tokens = Lexer::new(source).tokenize()?;
//! let token_stream = TokenStream::new(db, tokens);
//! let mut parser = Parser::new(db, token_stream);
//! let (nodelist, errors) = parser.parse()?;
//! let syntax_tree = build_syntax_tree(db, &nodelist)?;
//! ```

mod ast;
pub mod db;
mod error;
pub mod fragment;
mod lexer;
mod parser;
pub mod syntax_tree;
pub mod templatetags;
mod tokens;
pub mod validation;

use ast::LineOffsets;
use ast::NodeList;
pub use db::Db;
pub use db::TemplateDiagnostic;
use djls_workspace::db::SourceFile;
use djls_workspace::FileKind;
pub use error::TemplateError;
pub use lexer::Lexer;
pub use parser::Parser;
pub use parser::ParserError;
use salsa::Accumulator;
pub use syntax_tree::SyntaxTree;
use tokens::TokenStream;
use validation::TagValidator;

/// Lex a template file into tokens.
///
/// This is the first phase of template processing. It tokenizes the source text
/// into Django-specific tokens (tags, variables, text, etc.).
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: SourceFile) -> TokenStream<'_> {
    if file.kind(db) != FileKind::Template {
        return TokenStream::new(db, vec![]);
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    match Lexer::new(text).tokenize() {
        Ok(tokens) => TokenStream::new(db, tokens),
        Err(err) => {
            // Create error diagnostic
            let error = TemplateError::Lexer(err.to_string());
            let empty_offsets = LineOffsets::default();
            accumulate_error(db, &error, &empty_offsets);

            // Return empty token stream
            TokenStream::new(db, vec![])
        }
    }
}

/// Parse tokens into an AST.
///
/// This is the second phase of template processing. It takes the token stream
/// from lexing and builds an Abstract Syntax Tree.
#[salsa::tracked]
fn parse_template(db: &dyn Db, file: SourceFile) -> NodeList<'_> {
    let token_stream = lex_template(db, file);

    // Check if lexing produced no tokens (likely due to an error)
    if token_stream.stream(db).is_empty() {
        // Return empty AST for error recovery
        let empty_nodelist = Vec::new();
        let empty_offsets = LineOffsets::default();
        return NodeList::new(db, empty_nodelist, empty_offsets);
    }

    // Parser needs the TokenStream<'db>
    match Parser::new(db, token_stream).parse() {
        Ok((ast, errors)) => {
            // Accumulate parser errors
            for error in errors {
                let template_error = TemplateError::Parser(error.to_string());
                accumulate_error(db, &template_error, ast.line_offsets(db));
            }
            ast
        }
        Err(err) => {
            // Critical parser error
            let template_error = TemplateError::Parser(err.to_string());
            let empty_offsets = LineOffsets::default();
            accumulate_error(db, &template_error, &empty_offsets);

            // Return empty AST
            let empty_nodelist = Vec::new();
            let empty_offsets = LineOffsets::default();
            NodeList::new(db, empty_nodelist, empty_offsets)
        }
    }
}

/// Parse tokens into a SyntaxTree.
///
/// This function builds a hierarchical SyntaxTree from the NodeList representation.
/// It reuses the existing parse_template function and builds the tree from it.
#[salsa::tracked]
fn parse_syntax_tree(db: &dyn Db, file: SourceFile) -> syntax_tree::SyntaxTree<'_> {
    let nodelist = parse_template(db, file);

    // Check if AST is empty (likely due to parse errors)
    if nodelist.nodelist(db).is_empty() && lex_template(db, file).stream(db).is_empty() {
        return syntax_tree::SyntaxTree::empty(db);
    }

    // Build syntax tree inline to avoid lifetime issues
    build_syntax_tree_inline(db, nodelist)
}

/// Build `SyntaxTree` inline from `NodeList`
fn build_syntax_tree_inline<'db>(
    db: &'db dyn Db,
    nodelist: NodeList<'db>,
) -> syntax_tree::SyntaxTree<'db> {
    let mut builder = TreeBuilder::new(db);

    for node in nodelist.nodelist(db) {
        builder.add_node(node.clone());
    }

    let root_children = builder.finish();
    let root = syntax_tree::SyntaxNode::Root {
        children: root_children,
    };
    let root_id = syntax_tree::SyntaxNodeId::new(db, root);

    syntax_tree::SyntaxTree::new(db, root_id, nodelist.line_offsets(db).clone())
}

/// `TreeBuilder` manages hierarchical construction of `SyntaxTree`
struct TreeBuilder<'db> {
    db: &'db dyn Db,
    tag_specs: std::sync::Arc<crate::templatetags::TagSpecs>,
    stack: Vec<BlockFrame<'db>>,
    root_children: Vec<syntax_tree::SyntaxNodeId<'db>>,
}

/// Represents an open block tag and its accumulated children
struct BlockFrame<'db> {
    tag_node: syntax_tree::TagNode<'db>,
    children: Vec<syntax_tree::SyntaxNodeId<'db>>,
    branches: Vec<BranchFrame<'db>>,
    current_branch: Option<BranchFrame<'db>>,
}

/// Represents a branch within a block (elif, else, empty)
struct BranchFrame<'db> {
    tag_node: Option<syntax_tree::TagNode<'db>>, // None for implicit first branch
    children: Vec<syntax_tree::SyntaxNodeId<'db>>,
}

impl<'db> TreeBuilder<'db> {
    fn new(db: &'db dyn Db) -> Self {
        Self {
            db,
            tag_specs: db.tag_specs(),
            stack: Vec::new(),
            root_children: Vec::new(),
        }
    }

    fn add_node(&mut self, node: crate::ast::Node<'db>) {
        use crate::ast::Node;
        use crate::syntax_tree::SyntaxNode;
        use crate::templatetags::TagType;

        let syntax_node = match node {
            Node::Text(text_node) => SyntaxNode::Text(syntax_tree::TextNode {
                content: text_node.content.clone(),
                span: text_node.span,
            }),
            Node::Variable(var_node) => SyntaxNode::Variable(syntax_tree::VariableNode {
                var: syntax_tree::VariableName::new(
                    self.db,
                    var_node.var.text(self.db).to_string(),
                ),
                filters: var_node
                    .filters
                    .iter()
                    .map(|f| syntax_tree::FilterName::new(self.db, f.text(self.db).to_string()))
                    .collect(),
                span: var_node.span,
            }),
            Node::Comment(comment_node) => SyntaxNode::Comment(syntax_tree::CommentNode {
                content: comment_node.content.clone(),
                span: comment_node.span,
            }),
            Node::Tag(tag_node) => {
                let name_str = tag_node.name.text(self.db);
                let tag_type = TagType::for_name(&name_str, &self.tag_specs);

                let meta = syntax_tree::TagMeta::from_tag(
                    self.db,
                    &name_str,
                    &tag_node.bits,
                    &self.tag_specs,
                );

                let syntax_tag_node = syntax_tree::TagNode {
                    name: syntax_tree::TagName::new(self.db, name_str.to_string()),
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

        let node_id = syntax_tree::SyntaxNodeId::new(self.db, syntax_node);
        self.add_to_current_context(node_id);
    }

    fn handle_opener(&mut self, tag_node: syntax_tree::TagNode<'db>) {
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

    fn handle_intermediate(&mut self, tag_node: syntax_tree::TagNode<'db>) {
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
            let node_id =
                syntax_tree::SyntaxNodeId::new(self.db, syntax_tree::SyntaxNode::Tag(tag_node));
            self.add_to_current_context(node_id);
        }
    }

    fn handle_closer(&mut self, closer_tag: syntax_tree::TagNode<'db>) {
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
                        let branch_node_id = syntax_tree::SyntaxNodeId::new(
                            self.db,
                            syntax_tree::SyntaxNode::Tag(branch_tag),
                        );
                        all_children.push(branch_node_id);
                    }
                    // Add branch children
                    all_children.extend(branch.children);
                }

                // Create the block tag with its children
                let block_tag = syntax_tree::TagNode {
                    name: frame.tag_node.name,
                    bits: frame.tag_node.bits,
                    span: frame.tag_node.span,
                    meta: frame.tag_node.meta,
                    children: all_children,
                };

                let block_node_id = syntax_tree::SyntaxNodeId::new(
                    self.db,
                    syntax_tree::SyntaxNode::Tag(block_tag),
                );

                self.add_to_current_context(block_node_id);
                return;
            }
        }

        // No matching opener found - add closer as regular node
        let node_id =
            syntax_tree::SyntaxNodeId::new(self.db, syntax_tree::SyntaxNode::Tag(closer_tag));
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

    fn add_to_current_context(&mut self, node_id: syntax_tree::SyntaxNodeId<'db>) {
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

    fn finish(mut self) -> Vec<syntax_tree::SyntaxNodeId<'db>> {
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
                    let branch_node_id = syntax_tree::SyntaxNodeId::new(
                        self.db,
                        syntax_tree::SyntaxNode::Tag(branch_tag),
                    );
                    all_children.push(branch_node_id);
                }
                all_children.extend(branch.children);
            }

            let block_tag = syntax_tree::TagNode {
                name: frame.tag_node.name,
                bits: frame.tag_node.bits,
                span: frame.tag_node.span,
                meta: frame.tag_node.meta,
                children: all_children,
            };

            let block_node_id =
                syntax_tree::SyntaxNodeId::new(self.db, syntax_tree::SyntaxNode::Tag(block_tag));

            self.root_children.push(block_node_id);
        }

        self.root_children
    }
}

/// Validate the AST using NodeList (LEGACY).
///
/// This is the third phase of template processing. It validates the AST
/// according to Django tag specifications and accumulates any validation errors.
#[salsa::tracked]
fn validate_template_legacy(db: &dyn Db, file: SourceFile) {
    let ast = parse_template(db, file);

    // Skip validation if AST is empty (likely due to parse errors)
    if ast.nodelist(db).is_empty() && lex_template(db, file).stream(db).is_empty() {
        return;
    }

    let validation_errors = TagValidator::new(db, ast).validate();

    for error in validation_errors {
        // Convert validation error to TemplateError for consistency
        let template_error = TemplateError::Validation(error);
        accumulate_error(db, &template_error, ast.line_offsets(db));
    }
}

/// Validate the SyntaxTree.
///
/// This is the third phase of template processing using the new SyntaxTree representation.
/// It validates the tree according to Django tag specifications and accumulates any validation errors.
#[salsa::tracked]
fn validate_template(db: &dyn Db, file: SourceFile) {
    let syntax_tree = parse_syntax_tree(db, file);

    // Skip validation if tree is empty (likely due to parse errors)
    if syntax_tree.children(db).is_empty() && lex_template(db, file).stream(db).is_empty() {
        return;
    }

    // TODO: Implement SyntaxTree-based validation
    // For now, we fall back to the legacy validation by converting back to NodeList
    // This is a temporary approach during the migration phase
    validate_template_legacy(db, file);
}

/// Helper function to convert errors to LSP diagnostics and accumulate
fn accumulate_error(db: &dyn Db, error: &TemplateError, line_offsets: &LineOffsets) {
    let code = error.diagnostic_code();
    let range = error
        .span()
        .map(|(start, length)| {
            let span = crate::ast::Span::new(start, length);
            span.to_lsp_range(line_offsets)
        })
        .unwrap_or_default();

    let diagnostic = tower_lsp_server::lsp_types::Diagnostic {
        range,
        severity: Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR),
        code: Some(tower_lsp_server::lsp_types::NumberOrString::String(
            code.to_string(),
        )),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: match error {
            TemplateError::Lexer(msg) | TemplateError::Parser(msg) => msg.clone(),
            _ => error.to_string(),
        },
        related_information: None,
        tags: None,
        data: None,
    };

    TemplateDiagnostic(diagnostic).accumulate(db);
}

/// LEGACY: Analyze a Django template file returning NodeList (DEPRECATED)
///
/// This function is deprecated in favor of `analyze_template` which returns SyntaxTree.
/// It's kept for backward compatibility during the migration period.
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     analyze_template_legacy::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn analyze_template_legacy(db: &dyn Db, file: SourceFile) -> Option<NodeList<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    validate_template(db, file);
    Some(parse_template(db, file))
}

/// Analyze a Django template file - parse, validate, and accumulate diagnostics.
///
/// This is the PRIMARY function for template processing. It's a Salsa tracked function
/// that orchestrates the three phases of template processing:
/// 1. Lexing (tokenization)
/// 2. Parsing (SyntaxTree construction)
/// 3. Validation (semantic checks)
///
/// Each phase is independently cached by Salsa, allowing for fine-grained
/// incremental computation.
///
/// The function returns the parsed SyntaxTree (or None for non-template files).
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     analyze_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<syntax_tree::SyntaxTree<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    validate_template(db, file);
    Some(parse_syntax_tree(db, file))
}

/// Analyze a Django template file using SyntaxTree - parse, validate, and accumulate diagnostics.
///
/// This is the NEW function for template processing using the hierarchical SyntaxTree
/// representation. It orchestrates the same three phases as analyze_template:
/// 1. Lexing (tokenization)
/// 2. Parsing (SyntaxTree construction)
/// 3. Validation (semantic checks)
///
/// Each phase is independently cached by Salsa, allowing for fine-grained
/// incremental computation.
///
/// The function returns the parsed SyntaxTree (or None for non-template files).
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     analyze_syntax_tree::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn analyze_syntax_tree(db: &dyn Db, file: SourceFile) -> Option<syntax_tree::SyntaxTree<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    validate_template(db, file);
    Some(parse_syntax_tree(db, file))
}
