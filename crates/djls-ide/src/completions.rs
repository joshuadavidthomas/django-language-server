//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use djls_project::TemplateTags;
use djls_semantic::TagArg;
use djls_semantic::TagSpecs;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::lsp_types;

use crate::snippets::generate_partial_snippet;
use crate::snippets::generate_snippet_for_tag_with_end;

/// Tracks what closing characters are needed to complete a template tag.
///
/// Used to determine whether the completion system needs to insert
/// closing braces when completing a Django template tag.
#[derive(Debug, Clone, PartialEq)]
pub enum ClosingBrace {
    /// No closing brace present - need to add full `%}` or `}}`
    None,
    /// Partial close present (just `}`) - need to add `%` or second `}`
    PartialClose,
    /// Full close present (`%}` or `}}`) - no closing needed
    FullClose,
}

/// Rich context-aware completion information for Django templates.
///
/// Distinguishes between different completion contexts to provide
/// appropriate suggestions based on cursor position.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum TemplateCompletionContext {
    /// Completing a tag name after {%
    TagName {
        /// Partial tag name typed so far
        partial: String,
        /// Whether a space is needed before the tag name
        needs_space: bool,
        /// What closing characters are present
        closing: ClosingBrace,
    },
    /// Completing arguments within a tag
    TagArgument {
        /// The tag name
        tag: String,
        /// Position in the argument list (0-based)
        position: usize,
        /// Partial text for current argument
        partial: String,
        /// Arguments already parsed before cursor
        parsed_args: Vec<String>,
        /// What closing characters are present
        closing: ClosingBrace,
    },
    /// Completing a library name after {% load
    LibraryName {
        /// Partial library name typed so far
        partial: String,
        /// What closing characters are present
        closing: ClosingBrace,
    },
    /// TODO: Future - completing filters after |
    Filter {
        /// Partial filter name typed so far
        partial: String,
    },
    /// TODO: Future - completing variables after {{
    Variable {
        /// Partial variable name typed so far
        partial: String,
        /// What closing characters are present
        closing: ClosingBrace,
    },
    /// No template context found
    None,
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
#[must_use]
pub fn handle_completion(
    document: &TextDocument,
    position: lsp_types::Position,
    encoding: PositionEncoding,
    file_kind: FileKind,
    template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<lsp_types::CompletionItem> {
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
    generate_template_completions(
        &context,
        template_tags,
        tag_specs,
        supports_snippets,
        position,
        &line_info.text,
        line_info.cursor_offset,
    )
}

/// Extract line information from document at given position
fn get_line_info(
    document: &TextDocument,
    position: lsp_types::Position,
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
fn analyze_template_context(line: &str, cursor_offset: usize) -> Option<TemplateCompletionContext> {
    // Find the last {% before cursor position
    let prefix = &line[..cursor_offset.min(line.len())];
    let tag_start = prefix.rfind("{%")?;

    // Get the content between {% and cursor
    let content_start = tag_start + 2;
    let content = &prefix[content_start..];

    // Check what's after the cursor for closing detection
    let suffix = &line[cursor_offset.min(line.len())..];
    let closing = detect_closing_brace(suffix);

    // Check if we need a leading space (no space after {%)
    let needs_space = content.is_empty() || !content.starts_with(' ');

    // Parse the content to determine context
    let trimmed = content.trim_start();

    // Split into tokens by whitespace
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();

    if tokens.is_empty() {
        // Just opened tag, completing tag name
        return Some(TemplateCompletionContext::TagName {
            partial: String::new(),
            needs_space,
            closing,
        });
    }

    // Check if we're in the middle of typing the first token (tag name)
    if tokens.len() == 1 && !trimmed.ends_with(char::is_whitespace) {
        // Still typing the tag name
        return Some(TemplateCompletionContext::TagName {
            partial: tokens[0].to_string(),
            needs_space,
            closing,
        });
    }

    // We have a complete tag name and are working on arguments
    let tag_name = tokens[0];

    // Special case for {% load %} - completing library names
    if tag_name == "load" {
        // Get the partial library name being typed
        let partial = if trimmed.ends_with(char::is_whitespace) {
            String::new()
        } else if tokens.len() > 1 {
            (*tokens.last().unwrap()).to_string()
        } else {
            String::new()
        };

        return Some(TemplateCompletionContext::LibraryName { partial, closing });
    }

    // For other tags, we're completing arguments
    // Calculate argument position and partial text
    let parsed_args: Vec<String> = if tokens.len() > 1 {
        tokens[1..].iter().map(|&s| s.to_string()).collect()
    } else {
        Vec::new()
    };

    // Determine position and partial
    let (position, partial) = if trimmed.ends_with(char::is_whitespace) {
        // After a space, starting a new argument
        (parsed_args.len(), String::new())
    } else if !parsed_args.is_empty() {
        // In the middle of typing an argument
        (parsed_args.len() - 1, parsed_args.last().unwrap().clone())
    } else {
        // Just after tag name with space
        (0, String::new())
    };

    Some(TemplateCompletionContext::TagArgument {
        tag: tag_name.to_string(),
        position,
        partial: partial.clone(),
        parsed_args: if partial.is_empty() {
            parsed_args
        } else {
            // Don't include the partial argument in parsed_args
            parsed_args[..parsed_args.len() - 1].to_vec()
        },
        closing,
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
    context: &TemplateCompletionContext,
    template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
    position: lsp_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<lsp_types::CompletionItem> {
    match context {
        TemplateCompletionContext::TagName {
            partial,
            needs_space,
            closing,
        } => generate_tag_name_completions(
            partial,
            *needs_space,
            closing,
            template_tags,
            tag_specs,
            supports_snippets,
            position,
            line_text,
            cursor_offset,
        ),
        TemplateCompletionContext::TagArgument {
            tag,
            position,
            partial,
            parsed_args,
            closing,
        } => generate_argument_completions(
            tag,
            *position,
            partial,
            parsed_args,
            closing,
            template_tags,
            tag_specs,
            supports_snippets,
        ),
        TemplateCompletionContext::LibraryName { partial, closing } => {
            generate_library_completions(partial, closing, template_tags)
        }
        TemplateCompletionContext::Filter { .. }
        | TemplateCompletionContext::Variable { .. }
        | TemplateCompletionContext::None => {
            // Not implemented yet
            Vec::new()
        }
    }
}

/// Calculate the range to replace for a completion
fn calculate_replacement_range(
    position: lsp_types::Position,
    line_text: &str,
    cursor_offset: usize,
    partial_len: usize,
    closing: &ClosingBrace,
) -> lsp_types::Range {
    // Start position: move back by the length of the partial text
    let start_col = position
        .character
        .saturating_sub(u32::try_from(partial_len).unwrap_or(0));
    let start = lsp_types::Position::new(position.line, start_col);

    // End position: include auto-paired } if present
    let mut end_col = position.character;
    if matches!(closing, ClosingBrace::PartialClose) {
        // Include the auto-paired } in the replacement range
        // Check if there's a } immediately after cursor
        if line_text.len() > cursor_offset && &line_text[cursor_offset..=cursor_offset] == "}" {
            end_col += 1;
        }
    }
    let end = lsp_types::Position::new(position.line, end_col);

    lsp_types::Range::new(start, end)
}

/// Generate completions for tag names
#[allow(clippy::too_many_arguments)]
fn generate_tag_name_completions(
    partial: &str,
    needs_space: bool,
    closing: &ClosingBrace,
    template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
    position: lsp_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<lsp_types::CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    let mut completions = Vec::new();

    // Calculate the replacement range for all completions
    let replacement_range =
        calculate_replacement_range(position, line_text, cursor_offset, partial.len(), closing);

    // First, check if we should suggest end tags
    // If partial starts with "end", prioritize end tags
    if partial.starts_with("end") {
        if let Some(specs) = tag_specs {
            // Add all end tags that match the partial
            for (opener_name, spec) in specs {
                if let Some(end_tag) = &spec.end_tag {
                    if end_tag.name.starts_with(partial) {
                        // Create a completion for the end tag
                        let mut insert_text = String::new();
                        if needs_space {
                            insert_text.push(' ');
                        }
                        insert_text.push_str(&end_tag.name);

                        // Add closing based on what's already present
                        match closing {
                            ClosingBrace::PartialClose | ClosingBrace::None => {
                                insert_text.push_str(" %}");
                            }
                            ClosingBrace::FullClose => {} // No closing needed
                        }

                        completions.push(lsp_types::CompletionItem {
                            label: end_tag.name.to_string(),
                            kind: Some(lsp_types::CompletionItemKind::KEYWORD),
                            detail: Some(format!("End tag for {opener_name}")),
                            text_edit: Some(tower_lsp_server::lsp_types::CompletionTextEdit::Edit(
                                lsp_types::TextEdit::new(replacement_range, insert_text.clone()),
                            )),
                            insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                            filter_text: Some(end_tag.name.to_string()),
                            sort_text: Some(format!("0_{}", end_tag.name.as_ref())), // Priority sort
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    for tag in tags.iter() {
        if tag.name().starts_with(partial) {
            // Try to get snippet from TagSpecs if available and client supports snippets
            let (insert_text, insert_format) = if supports_snippets {
                if let Some(specs) = tag_specs {
                    if let Some(spec) = specs.get(tag.name()) {
                        if spec.args.is_empty() {
                            // No args, use plain text
                            build_plain_insert_for_tag(tag.name(), needs_space, closing)
                        } else {
                            // Generate snippet from tag spec
                            let mut text = String::new();

                            // Add leading space if needed
                            if needs_space {
                                text.push(' ');
                            }

                            // Generate the snippet
                            let snippet = generate_snippet_for_tag_with_end(tag.name(), spec);
                            text.push_str(&snippet);

                            // Only add closing if the snippet doesn't already include it
                            // (snippets for tags with end tags include their own %} closing)
                            if !snippet.contains("%}") {
                                // Add closing based on what's already present
                                match closing {
                                    ClosingBrace::PartialClose | ClosingBrace::None => {
                                        text.push_str(" %}");
                                    }
                                    ClosingBrace::FullClose => {} // No closing needed
                                }
                            }

                            (text, lsp_types::InsertTextFormat::SNIPPET)
                        }
                    } else {
                        // No spec found, use plain text
                        build_plain_insert_for_tag(tag.name(), needs_space, closing)
                    }
                } else {
                    // No specs available, use plain text
                    build_plain_insert_for_tag(tag.name(), needs_space, closing)
                }
            } else {
                // Client doesn't support snippets
                build_plain_insert_for_tag(tag.name(), needs_space, closing)
            };

            // Create completion item
            // Use SNIPPET kind when we're inserting a snippet, KEYWORD otherwise
            let kind = if matches!(insert_format, lsp_types::InsertTextFormat::SNIPPET) {
                lsp_types::CompletionItemKind::SNIPPET
            } else {
                lsp_types::CompletionItemKind::KEYWORD
            };

            let completion_item = lsp_types::CompletionItem {
                label: tag.name().clone(),
                kind: Some(kind),
                detail: Some(format!("from {}", tag.module())),
                documentation: tag
                    .doc()
                    .map(|doc| lsp_types::Documentation::String(doc.clone())),
                text_edit: Some(tower_lsp_server::lsp_types::CompletionTextEdit::Edit(
                    lsp_types::TextEdit::new(replacement_range, insert_text.clone()),
                )),
                insert_text_format: Some(insert_format),
                filter_text: Some(tag.name().clone()),
                sort_text: Some(format!("1_{}", tag.name())), // Regular tags sort after end tags
                ..Default::default()
            };

            completions.push(completion_item);
        }
    }

    completions
}

/// Generate completions for tag arguments
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn generate_argument_completions(
    tag: &str,
    position: usize,
    partial: &str,
    _parsed_args: &[String],
    closing: &ClosingBrace,
    _template_tags: Option<&TemplateTags>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<lsp_types::CompletionItem> {
    let Some(specs) = tag_specs else {
        return Vec::new();
    };

    let Some(spec) = specs.get(tag) else {
        return Vec::new();
    };

    // Get the argument at this position
    if position >= spec.args.len() {
        return Vec::new(); // Beyond expected args
    }

    let arg = &spec.args[position];
    let mut completions = Vec::new();

    match arg {
        TagArg::Literal { lit, .. } => {
            // For literals, complete the exact text
            if lit.starts_with(partial) {
                let mut insert_text = lit.to_string();

                // Add closing if needed
                match closing {
                    ClosingBrace::PartialClose | ClosingBrace::None => insert_text.push_str(" %}"), // Include full closing since we're replacing the auto-paired }
                    ClosingBrace::FullClose => {} // No closing needed
                }

                completions.push(lsp_types::CompletionItem {
                    label: lit.to_string(),
                    kind: Some(lsp_types::CompletionItemKind::KEYWORD),
                    detail: Some("literal argument".to_string()),
                    insert_text: Some(insert_text),
                    insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        TagArg::Choice { name, choices, .. } => {
            // For choices, offer each option
            for option in choices.iter() {
                if option.starts_with(partial) {
                    let mut insert_text = option.to_string();

                    // Add closing if needed
                    match closing {
                        ClosingBrace::None => insert_text.push_str(" %}"),
                        ClosingBrace::PartialClose => insert_text.push_str(" %"),
                        ClosingBrace::FullClose => {} // No closing needed
                    }

                    completions.push(lsp_types::CompletionItem {
                        label: option.to_string(),
                        kind: Some(lsp_types::CompletionItemKind::ENUM_MEMBER),
                        detail: Some(format!("choice for {}", name.as_ref())),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                        ..Default::default()
                    });
                }
            }
        }
        TagArg::Var { name, .. } => {
            // For variables, we could offer variable completions from context
            // For now, just provide a hint
            if partial.is_empty() {
                completions.push(lsp_types::CompletionItem {
                    label: format!("<{}>", name.as_ref()),
                    kind: Some(lsp_types::CompletionItemKind::VARIABLE),
                    detail: Some("variable argument".to_string()),
                    insert_text: None, // Don't insert placeholder
                    insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        TagArg::String { name, .. } => {
            // For strings, could offer template name completions
            // For now, just provide a hint
            if partial.is_empty() {
                completions.push(lsp_types::CompletionItem {
                    label: format!("\"{}\"", name.as_ref()),
                    kind: Some(lsp_types::CompletionItemKind::TEXT),
                    detail: Some("string argument".to_string()),
                    insert_text: None, // Don't insert placeholder
                    insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        _ => {
            // Other argument types (Expr, Assignment, VarArgs) not handled yet
        }
    }

    // If we're at the start of an argument position and client supports snippets,
    // offer a snippet for all remaining arguments
    if partial.is_empty() && supports_snippets && position < spec.args.len() {
        let remaining_snippet = generate_partial_snippet(spec, position);
        if !remaining_snippet.is_empty() {
            let mut insert_text = remaining_snippet;

            // Add closing if needed
            match closing {
                ClosingBrace::None => insert_text.push_str(" %}"),
                ClosingBrace::PartialClose => insert_text.push_str(" %"),
                ClosingBrace::FullClose => {} // No closing needed
            }

            // Create a completion item for the full remaining arguments
            let label = if position == 0 {
                format!("{tag} arguments")
            } else {
                "remaining arguments".to_string()
            };

            completions.push(lsp_types::CompletionItem {
                label,
                kind: Some(lsp_types::CompletionItemKind::SNIPPET),
                detail: Some("Complete remaining arguments".to_string()),
                insert_text: Some(insert_text),
                insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
                sort_text: Some("zzz".to_string()), // Sort at the end
                ..Default::default()
            });
        }
    }

    completions
}

/// Generate completions for library names (for {% load %} tag)
fn generate_library_completions(
    partial: &str,
    closing: &ClosingBrace,
    template_tags: Option<&TemplateTags>,
) -> Vec<lsp_types::CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    // Get unique library names
    let mut libraries = std::collections::HashSet::new();
    for tag in tags.iter() {
        libraries.insert(tag.module());
    }

    let mut completions = Vec::new();

    for library in libraries {
        if library.starts_with(partial) {
            let mut insert_text = library.clone();

            // Add closing if needed
            match closing {
                ClosingBrace::None => insert_text.push_str(" %}"),
                ClosingBrace::PartialClose => insert_text.push_str(" %"),
                ClosingBrace::FullClose => {} // No closing needed
            }

            completions.push(lsp_types::CompletionItem {
                label: library.clone(),
                kind: Some(lsp_types::CompletionItemKind::MODULE),
                detail: Some("Django template library".to_string()),
                insert_text: Some(insert_text),
                insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                filter_text: Some(library.clone()),
                ..Default::default()
            });
        }
    }

    completions
}

/// Build plain insert text without snippets for tag names
fn build_plain_insert_for_tag(
    tag_name: &str,
    needs_space: bool,
    closing: &ClosingBrace,
) -> (String, lsp_types::InsertTextFormat) {
    let mut insert_text = String::new();

    // Add leading space if needed (cursor right after {%)
    if needs_space {
        insert_text.push(' ');
    }

    // Add the tag name
    insert_text.push_str(tag_name);

    // Add closing based on what's already present
    match closing {
        ClosingBrace::PartialClose | ClosingBrace::None => insert_text.push_str(" %}"), // Include full closing since we're replacing the auto-paired }
        ClosingBrace::FullClose => {} // No closing needed
    }

    (insert_text, lsp_types::InsertTextFormat::PLAIN_TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_template_context_tag_name() {
        let line = "{% loa";
        let cursor_offset = 6; // After "loa"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: "loa".to_string(),
                needs_space: false,
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_needs_space() {
        let line = "{%loa";
        let cursor_offset = 5; // After "loa"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: "loa".to_string(),
                needs_space: true,
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_with_closing() {
        let line = "{% load %}";
        let cursor_offset = 7; // After "load"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: "load".to_string(),
                needs_space: false,
                closing: ClosingBrace::FullClose,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_library_name() {
        let line = "{% load stat";
        let cursor_offset = 12; // After "stat"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::LibraryName {
                partial: "stat".to_string(),
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_tag_argument() {
        let line = "{% for item i";
        let cursor_offset = 13; // After "i"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagArgument {
                tag: "for".to_string(),
                position: 1,
                partial: "i".to_string(),
                parsed_args: vec!["item".to_string()],
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_tag_argument_with_space() {
        let line = "{% for item ";
        let cursor_offset = 12; // After space

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagArgument {
                tag: "for".to_string(),
                position: 1,
                partial: String::new(),
                parsed_args: vec!["item".to_string()],
                closing: ClosingBrace::None,
            }
        );
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
        let context = TemplateCompletionContext::TagName {
            partial: "loa".to_string(),
            needs_space: false,
            closing: ClosingBrace::None,
        };

        let completions = generate_template_completions(
            &context,
            None,
            None,
            false,
            lsp_types::Position::new(0, 0),
            "",
            0,
        );

        assert!(completions.is_empty());
    }

    #[test]
    fn test_analyze_context_for_tag_empty() {
        let line = "{% ";
        let cursor_offset = 3; // After space

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: String::new(),
                needs_space: false,
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_context_for_second_argument() {
        let line = "{% for item in ";
        let cursor_offset = 15; // After "in "

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagArgument {
                tag: "for".to_string(),
                position: 2,
                partial: String::new(),
                parsed_args: vec!["item".to_string(), "in".to_string()],
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_context_autoescape_argument() {
        let line = "{% autoescape o";
        let cursor_offset = 15; // After "o"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagArgument {
                tag: "autoescape".to_string(),
                position: 0,
                partial: "o".to_string(),
                parsed_args: vec![],
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_library_context_multiple_libs() {
        let line = "{% load staticfiles i18n ";
        let cursor_offset = 25; // After "i18n "

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::LibraryName {
                partial: String::new(),
                closing: ClosingBrace::None,
            }
        );
    }

    #[test]
    fn test_analyze_template_context_with_auto_paired_brace() {
        // Simulates when editor auto-pairs { with } and user types {% if
        let line = "{% if}";
        let cursor_offset = 5; // After "if", before the auto-paired }

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: "if".to_string(),
                needs_space: false,
                closing: ClosingBrace::PartialClose, // Auto-paired } is detected as PartialClose
            }
        );
    }

    #[test]
    fn test_analyze_template_context_with_proper_closing() {
        // Proper closing should still be detected
        let line = "{% if %}";
        let cursor_offset = 5; // After "if"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::TagName {
                partial: "if".to_string(),
                needs_space: false,
                closing: ClosingBrace::FullClose,
            }
        );
    }
}
