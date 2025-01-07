use tower_lsp::lsp_types::*;
use crate::documents::TextDocument;
use djls_template_ast::{Lexer, Parser};
use djls_template_ast::ast::{Ast, Node, LineOffsets, Span};
use djls_template_ast::tokens::TokenStream;

pub struct Diagnostics;

impl Diagnostics {
    pub fn generate_for_document(document: &TextDocument) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        
        let text = document.get_text();
        let lexer = Lexer::new(text);
        let token_stream = match lexer.tokenize() {
            Ok(tokens) => tokens,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("lexing_error".to_string())),
                    source: Some("Django LSP".to_string()),
                    message: format!("Lexing error: {}", e),
                    ..Default::default()
                });
                return diagnostics;
            }
        };

        let mut parser = Parser::new(token_stream);
        let (ast, parse_errors) = match parser.parse() {
            Ok(result) => result,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("parsing_error".to_string())),
                    source: Some("Django LSP".to_string()),
                    message: format!("Parsing error: {}", e),
                    ..Default::default()
                });
                return diagnostics;
            }
        };

        for error in parse_errors {
            diagnostics.push(Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)), // Adjust range based on error
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("parse_error".to_string())),
                source: Some("Django LSP".to_string()),
                message: format!("Parse error: {}", error),
                ..Default::default()
            });
        }

        for node in ast.nodes() {
            match node {
                Node::Block(block) => {
                    if let Some(closing) = block.closing() {
                    } else {
                        let span = block.tag().span;
                        let range = get_range_from_span(&ast.line_offsets(), &span);
                        diagnostics.push(Diagnostic {
                            range,
                            severity: Some(DiagnosticSeverity::ERROR),
                            code: Some(NumberOrString::String("unclosed_block".to_string())),
                            source: Some("Django LSP".to_string()),
                            message: format!("Unclosed block tag '{}'", block.tag().name),
                            ..Default::default()
                        });
                    }
                },
                Node::Variable { .. } => {},
                _ => {},
            }
        }

        diagnostics
    }
}

fn get_range_from_span(line_offsets: &LineOffsets, span: &Span) -> Range {
    let (start_line, start_col) = line_offsets.position_to_line_col(span.start as usize);
    let (end_line, end_col) = line_offsets.position_to_line_col((span.start + span.length) as usize);

    Range {
        start: Position::new(start_line as u32 - 1, start_col as u32),
        end: Position::new(end_line as u32 - 1, end_col as u32),
    }
}
