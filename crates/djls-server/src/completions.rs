//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use djls_project::TemplateTags;
use djls_templates::templatetags::generate_snippet_for_tag;
use djls_templates::templatetags::TagSpecs;
use djls_workspace::FileKind;
use djls_workspace::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::lsp_types::CompletionItem;
use tower_lsp_server::lsp_types::CompletionItemKind;
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::Position;

/// Tracks what closing characters are needed to complete a template tag.
///
/// Used to determine whether the completion system needs to insert
/// closing braces when completing a Django template tag.
#[derive(Debug)]
pub enum ClosingBrace {
    /// No closing brace present - need to add full `%}` or `}}`
    None,
    /// Partial close present (just `}`) - need to add `%` or second `}`
    PartialClose,
    /// Full close present (`%}` or `}}`) - no closing needed
    FullClose,
}

/// Cursor context within a Django template tag for completion support.
///
/// Captures the state around the cursor position to provide intelligent
/// completions and determine what text needs to be inserted.
#[derive(Debug)]
pub struct TemplateTagContext {
    /// The partial tag text before the cursor (e.g., "loa" for "{% loa|")
    pub partial_tag: String,
    /// What closing characters are already present after the cursor
    pub closing_brace: ClosingBrace,
    /// Whether a space is needed before the completion (true if cursor is right after `{%`)
    pub needs_leading_space: bool,
}

/// Information about a line of text and cursor position within it
#[derive(Debug)]
pub struct LineInfo {
    /// The complete line text
    pub text: String,
    /// The cursor offset within the line (in characters)
    pub cursor_offset: usize,
}

/// Main entry point for handling completion requests
pub fn handle_completion(
    document: &TextDocument,
    position: Position,
    encoding: PositionEncoding,
    file_kind: FileKind,
    template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<CompletionItem> {
    // Only handle template files
    if file_kind != FileKind::Template {
        return Vec::new();
    }

    // Get line information from document
    let Some(line_info) = get_line_info(document, position, encoding) else {
        return Vec::new();
    };

    // Analyze template context at cursor position
    let Some(context) = analyze_template_context(&line_info.text, line_info.cursor_offset) else {
        return Vec::new();
    };

    // Generate completions based on available template tags
    generate_template_completions(&context, template_tags, tag_specs, supports_snippets)
}

/// Extract line information from document at given position
fn get_line_info(
    document: &TextDocument,
    position: Position,
    encoding: PositionEncoding,
) -> Option<LineInfo> {
    let content = document.content();
    let lines: Vec<&str> = content.lines().collect();

    let line_index = position.line as usize;
    if line_index >= lines.len() {
        return None;
    }

    let line_text = lines[line_index].to_string();

    // Convert LSP position to character index for Vec<char> operations.
    //
    // LSP default encoding is UTF-16 (emoji = 2 units), but we need
    // character counts (emoji = 1 char) to index into chars[..offset].
    //
    // Example:
    //   "h€llo" cursor after € → UTF-16: 2, chars: 2 ✓, bytes: 4 ✗
    let cursor_offset_in_line = match encoding {
        PositionEncoding::Utf16 => {
            let utf16_pos = position.character as usize;
            let mut char_offset = 0; // Count chars, not bytes
            let mut utf16_offset = 0;

            for ch in line_text.chars() {
                if utf16_offset >= utf16_pos {
                    break;
                }
                utf16_offset += ch.len_utf16();
                char_offset += 1;
            }
            char_offset
        }
        _ => position.character as usize,
    };

    Some(LineInfo {
        text: line_text,
        cursor_offset: cursor_offset_in_line.min(lines[line_index].chars().count()),
    })
}

/// Analyze a line of template text to determine completion context
fn analyze_template_context(line: &str, cursor_offset: usize) -> Option<TemplateTagContext> {
    // Find the last {% before cursor position
    let prefix = &line[..cursor_offset.min(line.len())];
    let tag_start = prefix.rfind("{%")?;

    // Get the content between {% and cursor
    let content_start = tag_start + 2;
    let content = &prefix[content_start..];

    // Check if we need a leading space (no space after {%)
    let needs_leading_space = content.is_empty() || !content.starts_with(' ');

    // Extract the partial tag name
    let partial_tag = content.trim_start().to_string();

    // Check what's after the cursor for closing detection
    let suffix = &line[cursor_offset.min(line.len())..];
    let closing_brace = detect_closing_brace(suffix);

    Some(TemplateTagContext {
        partial_tag,
        closing_brace,
        needs_leading_space,
    })
}

/// Detect what closing brace is present after the cursor
fn detect_closing_brace(suffix: &str) -> ClosingBrace {
    let trimmed = suffix.trim_start();
    if trimmed.starts_with("%}") {
        ClosingBrace::FullClose
    } else if trimmed.starts_with('}') {
        ClosingBrace::PartialClose
    } else {
        ClosingBrace::None
    }
}

/// Generate Django template tag completion items based on context
fn generate_template_completions(
    context: &TemplateTagContext,
    template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    let mut completions = Vec::new();

    for tag in tags.iter() {
        if tag.name().starts_with(&context.partial_tag) {
            // Try to get snippet from TagSpecs if available and client supports snippets
            let (insert_text, insert_format) = if supports_snippets {
                if let Some(specs) = tag_specs {
                    if let Some(spec) = specs.get(tag.name()) {
                        if spec.args.is_empty() {
                            // No args, use plain text
                            build_plain_insert(tag.name(), context)
                        } else {
                            // Generate snippet from tag spec
                            let mut text = String::new();

                            // Add leading space if needed
                            if context.needs_leading_space {
                                text.push(' ');
                            }

                            // Add tag name and snippet arguments
                            text.push_str(&generate_snippet_for_tag(tag.name(), spec));

                            // Add closing based on what's already present
                            match context.closing_brace {
                                ClosingBrace::None => text.push_str(" %}"),
                                ClosingBrace::PartialClose => text.push('%'),
                                ClosingBrace::FullClose => {} // No closing needed
                            }

                            (text, InsertTextFormat::SNIPPET)
                        }
                    } else {
                        // No spec found, use plain text
                        build_plain_insert(tag.name(), context)
                    }
                } else {
                    // No specs available, use plain text
                    build_plain_insert(tag.name(), context)
                }
            } else {
                // Client doesn't support snippets
                build_plain_insert(tag.name(), context)
            };

            // Create completion item
            let completion_item = CompletionItem {
                label: tag.name().clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("from {}", tag.library())),
                documentation: tag.doc().map(|doc| Documentation::String(doc.clone())),
                insert_text: Some(insert_text),
                insert_text_format: Some(insert_format),
                filter_text: Some(tag.name().clone()),
                ..Default::default()
            };

            completions.push(completion_item);
        }
    }

    completions
}

/// Build plain insert text without snippets
fn build_plain_insert(tag_name: &str, context: &TemplateTagContext) -> (String, InsertTextFormat) {
    let mut insert_text = String::new();

    // Add leading space if needed (cursor right after {%)
    if context.needs_leading_space {
        insert_text.push(' ');
    }

    // Add the tag name
    insert_text.push_str(tag_name);

    // Add closing based on what's already present
    match context.closing_brace {
        ClosingBrace::None => insert_text.push_str(" %}"),
        ClosingBrace::PartialClose => insert_text.push('%'),
        ClosingBrace::FullClose => {} // No closing needed
    }

    (insert_text, InsertTextFormat::PLAIN_TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_template_context_basic() {
        let line = "{% loa";
        let cursor_offset = 6; // After "loa"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(context.partial_tag, "loa");
        assert!(!context.needs_leading_space);
        assert!(matches!(context.closing_brace, ClosingBrace::None));
    }

    #[test]
    fn test_analyze_template_context_needs_leading_space() {
        let line = "{%loa";
        let cursor_offset = 5; // After "loa"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(context.partial_tag, "loa");
        assert!(context.needs_leading_space);
        assert!(matches!(context.closing_brace, ClosingBrace::None));
    }

    #[test]
    fn test_analyze_template_context_with_closing() {
        let line = "{% load %}";
        let cursor_offset = 7; // After "load"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(context.partial_tag, "load");
        assert!(!context.needs_leading_space);
        assert!(matches!(context.closing_brace, ClosingBrace::FullClose));
    }

    #[test]
    fn test_analyze_template_context_partial_closing() {
        let line = "{% load }";
        let cursor_offset = 7; // After "load"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(context.partial_tag, "load");
        assert!(!context.needs_leading_space);
        assert!(matches!(context.closing_brace, ClosingBrace::PartialClose));
    }

    #[test]
    fn test_analyze_template_context_no_template() {
        let line = "Just regular HTML";
        let cursor_offset = 5;

        let context = analyze_template_context(line, cursor_offset);

        assert!(context.is_none());
    }

    #[test]
    fn test_generate_template_completions_empty_tags() {
        let context = TemplateTagContext {
            partial_tag: "loa".to_string(),
            needs_leading_space: false,
            closing_brace: ClosingBrace::None,
        };

        let completions = generate_template_completions(&context, None, None, false);

        assert!(completions.is_empty());
    }
}
