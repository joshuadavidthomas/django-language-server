use tower_lsp::lsp_types::*;
use crate::documents::TextDocument;

pub struct Diagnostics;

impl Diagnostics {
    pub fn generate_for_document(document: &TextDocument) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        
        // Simple example: Check for TODO comments
        for (line_num, line) in document.get_text().lines().enumerate() {
            if let Some(col) = line.find("TODO") {
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position { 
                            line: line_num as u32, 
                            character: col as u32 
                        },
                        end: Position { 
                            line: line_num as u32, 
                            character: (col + 4) as u32 
                        },
                    },
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    code: Some(NumberOrString::String("django.todo".to_string())),
                    source: Some("Django LSP".to_string()),
                    message: "Found TODO comment".to_string(),
                    ..Default::default()
                });
            }
        }

        diagnostics
    }
}
