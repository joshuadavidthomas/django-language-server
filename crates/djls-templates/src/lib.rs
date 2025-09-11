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
mod lexer;
mod parser;
mod syntax;
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
use syntax::SyntaxNode;
use syntax::SyntaxNodeId;
use syntax::SyntaxTree;
use syntax::TreeBuilder;
use tokens::TokenStream;


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

    let (tokens, errors) = Lexer::new(text).tokenize();
    
    // Accumulate any lexer errors
    if !errors.is_empty() {
        let empty_offsets = LineOffsets::default();
        for err in errors {
            let error = TemplateError::from(err);
            accumulate_error(db, &error, &empty_offsets);
        }
    }
    
    TokenStream::new(db, tokens)
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
                let template_error = TemplateError::from(error);
                accumulate_error(db, &template_error, ast.line_offsets(db));
            }
            ast
        }
        Err(err) => {
            // Critical parser error
            let template_error = TemplateError::from(err);
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
fn parse_syntax_tree(db: &dyn Db, file: SourceFile) -> SyntaxTree<'_> {
    let nodelist = parse_template(db, file);

    // Check if AST is empty (likely due to parse errors)
    if nodelist.nodelist(db).is_empty() && lex_template(db, file).stream(db).is_empty() {
        return SyntaxTree::empty(db);
    }

    // Build syntax tree inline to avoid lifetime issues
    build_syntax_tree_inline(db, nodelist)
}

/// Build `SyntaxTree` inline from `NodeList`
fn build_syntax_tree_inline<'db>(db: &'db dyn Db, nodelist: NodeList<'db>) -> SyntaxTree<'db> {
    let mut builder = TreeBuilder::new(db);

    for node in nodelist.nodelist(db) {
        builder.add_node(node.clone());
    }

    let root_children = builder.finish();
    let root = SyntaxNode::Root {
        children: root_children,
    };
    let root_id = SyntaxNodeId::new(db, root);

    SyntaxTree::new(db, root_id, nodelist.line_offsets(db).clone())
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

    let validation_errors = validation::SyntaxTreeValidator::new(db, syntax_tree).validate();

    for error in validation_errors {
        // Convert validation error to TemplateError for consistency
        let template_error = TemplateError::from(error);
        accumulate_error(db, &template_error, syntax_tree.line_offsets(db));
    }
}

/// Helper function to convert errors to LSP diagnostics and accumulate
fn accumulate_error(db: &dyn Db, error: &TemplateError, line_offsets: &LineOffsets) {
    let code = error.code();
    let range = error
        .span()
        .map(|span| span.to_lsp_range(line_offsets))
        .unwrap_or_default();

    let diagnostic = tower_lsp_server::lsp_types::Diagnostic {
        range,
        severity: Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR),
        code: Some(tower_lsp_server::lsp_types::NumberOrString::String(
            code.to_string(),
        )),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: error.to_string(),
        related_information: None,
        tags: None,
        data: None,
    };

    TemplateDiagnostic(diagnostic).accumulate(db);
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
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<SyntaxTree<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    validate_template(db, file);
    Some(parse_syntax_tree(db, file))
}
