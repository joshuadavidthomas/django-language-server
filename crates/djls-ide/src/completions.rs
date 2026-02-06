//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use djls_project::InspectorInventory;
use djls_semantic::LoadedLibraries;
use djls_semantic::TagArg;
use djls_semantic::TagSpecs;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::ls_types;

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
#[allow(clippy::too_many_arguments)]
pub fn handle_completion(
    document: &TextDocument,
    position: ls_types::Position,
    encoding: PositionEncoding,
    file_kind: FileKind,
    inventory: Option<&InspectorInventory>,
    tag_specs: Option<&TagSpecs>,
    loaded_libraries: Option<&LoadedLibraries>,
    supports_snippets: bool,
) -> Vec<ls_types::CompletionItem> {
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

    // Calculate byte offset for load scoping
    let cursor_byte_offset = calculate_byte_offset(document, position, encoding);

    // Generate completions based on available template tags
    generate_template_completions(
        &context,
        inventory,
        tag_specs,
        loaded_libraries,
        cursor_byte_offset,
        supports_snippets,
        position,
        &line_info.text,
        line_info.cursor_offset,
    )
}

/// Calculate byte offset from line/character position
fn calculate_byte_offset(
    document: &TextDocument,
    position: ls_types::Position,
    encoding: PositionEncoding,
) -> u32 {
    let content = document.content();
    let lines: Vec<&str> = content.lines().collect();

    let mut byte_offset: usize = 0;

    // Add bytes for all complete lines before cursor
    for line in lines.iter().take(position.line as usize) {
        byte_offset += line.len() + 1; // +1 for newline
    }

    // Add bytes for characters in the current line up to cursor
    if let Some(line) = lines.get(position.line as usize) {
        let char_offset = match encoding {
            PositionEncoding::Utf16 => {
                let mut char_count = 0;
                let mut utf16_count = 0;
                for ch in line.chars() {
                    if utf16_count >= position.character as usize {
                        break;
                    }
                    utf16_count += ch.len_utf16();
                    char_count += 1;
                }
                char_count
            }
            _ => position.character as usize,
        };

        byte_offset += line.chars().take(char_offset).map(char::len_utf8).sum::<usize>();
    }

    #[allow(clippy::cast_possible_truncation)]
    {
        byte_offset as u32
    }
}

/// Extract line information from document at given position
fn get_line_info(
    document: &TextDocument,
    position: ls_types::Position,
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
#[allow(clippy::too_many_arguments)]
fn generate_template_completions(
    context: &TemplateCompletionContext,
    inventory: Option<&InspectorInventory>,
    tag_specs: Option<&TagSpecs>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
    supports_snippets: bool,
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<ls_types::CompletionItem> {
    match context {
        TemplateCompletionContext::TagName {
            partial,
            needs_space,
            closing,
        } => generate_tag_name_completions(
            partial,
            *needs_space,
            closing,
            inventory,
            tag_specs,
            loaded_libraries,
            cursor_byte_offset,
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
            inventory,
            tag_specs,
            supports_snippets,
        ),
        TemplateCompletionContext::LibraryName { partial, closing } => {
            generate_library_completions(
                partial,
                closing,
                inventory,
                loaded_libraries,
                cursor_byte_offset,
            )
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
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
    partial_len: usize,
    closing: &ClosingBrace,
) -> ls_types::Range {
    // Start position: move back by the length of the partial text
    let start_col = position
        .character
        .saturating_sub(u32::try_from(partial_len).unwrap_or(0));
    let start = ls_types::Position::new(position.line, start_col);

    // End position: include auto-paired } if present
    let mut end_col = position.character;
    if matches!(closing, ClosingBrace::PartialClose) {
        // Include the auto-paired } in the replacement range
        // Check if there's a } immediately after cursor
        if line_text.len() > cursor_offset && &line_text[cursor_offset..=cursor_offset] == "}" {
            end_col += 1;
        }
    }
    let end = ls_types::Position::new(position.line, end_col);

    ls_types::Range::new(start, end)
}

/// Generate completions for tag names
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn generate_tag_name_completions(
    partial: &str,
    needs_space: bool,
    closing: &ClosingBrace,
    inventory: Option<&InspectorInventory>,
    tag_specs: Option<&TagSpecs>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
    supports_snippets: bool,
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<ls_types::CompletionItem> {
    let Some(inv) = inventory else {
        return Vec::new();
    };

    // Compute available tags at cursor position when load info is present.
    // When load info is unavailable (None), show all tags as fallback.
    let available = loaded_libraries
        .map(|loaded| djls_semantic::available_tags_at(loaded, inv, cursor_byte_offset));

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

                        completions.push(ls_types::CompletionItem {
                            label: end_tag.name.to_string(),
                            kind: Some(ls_types::CompletionItemKind::KEYWORD),
                            detail: Some(format!("End tag for {opener_name}")),
                            text_edit: Some(tower_lsp_server::ls_types::CompletionTextEdit::Edit(
                                ls_types::TextEdit::new(replacement_range, insert_text.clone()),
                            )),
                            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                            filter_text: Some(end_tag.name.to_string()),
                            sort_text: Some(format!("0_{}", end_tag.name.as_ref())), // Priority sort
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    for tag in inv.iter_tags() {
        if !tag.name().starts_with(partial) {
            continue;
        }

        // Filter by availability when load info is present.
        // When available is None (no load info / inspector unavailable),
        // show all tags to avoid false negatives.
        if let Some(ref avail) = available {
            if !avail.has_tag(tag.name()) {
                continue;
            }
        }

        // Try to get snippet from TagSpecs if available and client supports snippets
        let (insert_text, insert_format) = if supports_snippets {
            if let Some(specs) = tag_specs {
                if let Some(spec) = specs.get(tag.name()) {
                    if spec.args.is_empty() {
                        build_plain_insert_for_tag(tag.name(), needs_space, closing)
                    } else {
                        let mut text = String::new();

                        if needs_space {
                            text.push(' ');
                        }

                        let snippet = generate_snippet_for_tag_with_end(tag.name(), spec);
                        text.push_str(&snippet);

                        if !snippet.contains("%}") {
                            match closing {
                                ClosingBrace::PartialClose | ClosingBrace::None => {
                                    text.push_str(" %}");
                                }
                                ClosingBrace::FullClose => {}
                            }
                        }

                        (text, ls_types::InsertTextFormat::SNIPPET)
                    }
                } else {
                    build_plain_insert_for_tag(tag.name(), needs_space, closing)
                }
            } else {
                build_plain_insert_for_tag(tag.name(), needs_space, closing)
            }
        } else {
            build_plain_insert_for_tag(tag.name(), needs_space, closing)
        };

        let kind = if matches!(insert_format, ls_types::InsertTextFormat::SNIPPET) {
            ls_types::CompletionItemKind::SNIPPET
        } else {
            ls_types::CompletionItemKind::KEYWORD
        };

        let completion_item = ls_types::CompletionItem {
            label: tag.name().clone(),
            kind: Some(kind),
            detail: Some(if let Some(lib) = tag.library_load_name() {
                format!("from {} ({{% load {} %}})", tag.defining_module(), lib)
            } else {
                format!("builtin from {}", tag.defining_module())
            }),
            documentation: tag
                .doc()
                .map(|doc| ls_types::Documentation::String(doc.clone())),
            text_edit: Some(tower_lsp_server::ls_types::CompletionTextEdit::Edit(
                ls_types::TextEdit::new(replacement_range, insert_text.clone()),
            )),
            insert_text_format: Some(insert_format),
            filter_text: Some(tag.name().clone()),
            sort_text: Some(format!("1_{}", tag.name())),
            ..Default::default()
        };

        completions.push(completion_item);
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
    _inventory: Option<&InspectorInventory>,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<ls_types::CompletionItem> {
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

                completions.push(ls_types::CompletionItem {
                    label: lit.to_string(),
                    kind: Some(ls_types::CompletionItemKind::KEYWORD),
                    detail: Some("literal argument".to_string()),
                    insert_text: Some(insert_text),
                    insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
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

                    completions.push(ls_types::CompletionItem {
                        label: option.to_string(),
                        kind: Some(ls_types::CompletionItemKind::ENUM_MEMBER),
                        detail: Some(format!("choice for {}", name.as_ref())),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                        ..Default::default()
                    });
                }
            }
        }
        TagArg::Variable { name, .. } => {
            // For variables, we could offer variable completions from context
            // For now, just provide a hint
            if partial.is_empty() {
                completions.push(ls_types::CompletionItem {
                    label: format!("<{}>", name.as_ref()),
                    kind: Some(ls_types::CompletionItemKind::VARIABLE),
                    detail: Some("variable argument".to_string()),
                    insert_text: None, // Don't insert placeholder
                    insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        TagArg::String { name, .. } => {
            // For strings, could offer template name completions
            // For now, just provide a hint
            if partial.is_empty() {
                completions.push(ls_types::CompletionItem {
                    label: format!("\"{}\"", name.as_ref()),
                    kind: Some(ls_types::CompletionItemKind::TEXT),
                    detail: Some("string argument".to_string()),
                    insert_text: None, // Don't insert placeholder
                    insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        _ => {
            // Other argument types (Any, Assignment, VarArgs) not handled yet
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

            completions.push(ls_types::CompletionItem {
                label,
                kind: Some(ls_types::CompletionItemKind::SNIPPET),
                detail: Some("Complete remaining arguments".to_string()),
                insert_text: Some(insert_text),
                insert_text_format: Some(ls_types::InsertTextFormat::SNIPPET),
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
    inventory: Option<&InspectorInventory>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
) -> Vec<ls_types::CompletionItem> {
    let Some(inv) = inventory else {
        return Vec::new();
    };

    let mut library_entries: Vec<_> = inv
        .libraries()
        .iter()
        .filter(|(load_name, _)| load_name.starts_with(partial))
        .collect();
    library_entries.sort_by_key(|(load_name, _)| load_name.as_str());

    let already_loaded = loaded_libraries
        .map(|l| l.libraries_before(cursor_byte_offset))
        .unwrap_or_default();

    let mut completions = Vec::new();

    for (load_name, module_path) in library_entries {
        let is_already_loaded = already_loaded.contains(load_name.as_str());

        let mut insert_text = load_name.clone();

        match closing {
            ClosingBrace::None => insert_text.push_str(" %}"),
            ClosingBrace::PartialClose => insert_text.push_str(" %"),
            ClosingBrace::FullClose => {}
        }

        completions.push(ls_types::CompletionItem {
            label: load_name.clone(),
            kind: Some(ls_types::CompletionItemKind::MODULE),
            detail: Some(if is_already_loaded {
                format!("Already loaded ({module_path})")
            } else {
                format!("Django template library ({module_path})")
            }),
            insert_text: Some(insert_text),
            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
            filter_text: Some(load_name.clone()),
            sort_text: Some(if is_already_loaded {
                format!("1_{load_name}")
            } else {
                format!("0_{load_name}")
            }),
            deprecated: Some(is_already_loaded),
            ..Default::default()
        });
    }

    completions
}

/// Build plain insert text without snippets for tag names
fn build_plain_insert_for_tag(
    tag_name: &str,
    needs_space: bool,
    closing: &ClosingBrace,
) -> (String, ls_types::InsertTextFormat) {
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

    (insert_text, ls_types::InsertTextFormat::PLAIN_TEXT)
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
            None,
            0,
            false,
            ls_types::Position::new(0, 0),
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

    #[test]
    fn test_generate_library_completions_uses_load_names() {
        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );
        libraries.insert(
            "cache".to_string(),
            "django.templatetags.cache".to_string(),
        );

        let tags = InspectorInventory::new(
            libraries,
            vec!["django.template.defaulttags".to_string()],
            vec![
                djls_project::TemplateTag::new_builtin(
                    "if",
                    "django.template.defaulttags",
                    None,
                ),
                djls_project::TemplateTag::new_library(
                    "static",
                    "static",
                    "django.templatetags.static",
                    None,
                ),
            ],
            vec![],
        );

        let completions =
            generate_library_completions("", &ClosingBrace::None, Some(&tags), None, 0);

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"static"));
        assert!(labels.contains(&"i18n"));
        assert!(labels.contains(&"cache"));
        assert!(!labels.iter().any(|l| l.contains("django.")));
        assert_eq!(labels, vec!["cache", "i18n", "static"]);
    }

    #[test]
    fn test_generate_library_completions_partial_filter() {
        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );

        let tags = InspectorInventory::new(libraries, vec![], vec![], vec![]);

        let completions =
            generate_library_completions("st", &ClosingBrace::None, Some(&tags), None, 0);

        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].label, "static");
    }

    #[test]
    fn test_generate_library_completions_closing_brace() {
        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let tags = InspectorInventory::new(libraries, vec![], vec![], vec![]);

        let no_close =
            generate_library_completions("", &ClosingBrace::None, Some(&tags), None, 0);
        assert_eq!(
            no_close[0].insert_text.as_deref(),
            Some("static %}")
        );

        let partial =
            generate_library_completions("", &ClosingBrace::PartialClose, Some(&tags), None, 0);
        assert_eq!(
            partial[0].insert_text.as_deref(),
            Some("static %")
        );

        let full =
            generate_library_completions("", &ClosingBrace::FullClose, Some(&tags), None, 0);
        assert_eq!(
            full[0].insert_text.as_deref(),
            Some("static")
        );
    }

    fn make_test_tags() -> InspectorInventory {
        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        InspectorInventory::new(
            libraries,
            vec!["django.template.defaulttags".to_string()],
            vec![
                djls_project::TemplateTag::new_builtin(
                    "if",
                    "django.template.defaulttags",
                    None,
                ),
                djls_project::TemplateTag::new_builtin(
                    "for",
                    "django.template.defaulttags",
                    None,
                ),
                djls_project::TemplateTag::new_library(
                    "trans",
                    "i18n",
                    "django.templatetags.i18n",
                    None,
                ),
                djls_project::TemplateTag::new_library(
                    "blocktrans",
                    "i18n",
                    "django.templatetags.i18n",
                    None,
                ),
                djls_project::TemplateTag::new_library(
                    "static",
                    "static",
                    "django.templatetags.static",
                    None,
                ),
            ],
            vec![],
        )
    }

    #[test]
    fn test_tag_completions_no_load_info_shows_all() {
        let tags = make_test_tags();

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&tags),
            None,
            None, // no loaded_libraries → fallback shows all
            0,
            false,
            ls_types::Position::new(0, 3),
            "{% ",
            3,
        );

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"if"));
        assert!(labels.contains(&"for"));
        assert!(labels.contains(&"trans"));
        assert!(labels.contains(&"blocktrans"));
        assert!(labels.contains(&"static"));
    }

    #[test]
    fn test_tag_completions_no_loads_only_builtins() {
        let tags = make_test_tags();
        let loaded = djls_semantic::LoadedLibraries::new();

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&tags),
            None,
            Some(&loaded),
            100, // cursor at byte 100, no loads present
            false,
            ls_types::Position::new(0, 3),
            "{% ",
            3,
        );

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"if"), "builtins should appear");
        assert!(labels.contains(&"for"), "builtins should appear");
        assert!(!labels.contains(&"trans"), "library tags should not appear without load");
        assert!(!labels.contains(&"static"), "library tags should not appear without load");
    }

    #[test]
    fn test_tag_completions_after_load_shows_loaded_tags() {
        use djls_semantic::LoadKind;
        use djls_semantic::LoadStatement;
        use djls_source::Span;

        let tags = make_test_tags();
        let mut loaded = djls_semantic::LoadedLibraries::new();
        // {% load i18n %} at span 0..15
        loaded.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&tags),
            None,
            Some(&loaded),
            50, // cursor after the load
            false,
            ls_types::Position::new(1, 3),
            "{% ",
            3,
        );

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"if"), "builtins should appear");
        assert!(labels.contains(&"trans"), "i18n tags should appear after load");
        assert!(labels.contains(&"blocktrans"), "i18n tags should appear after load");
        assert!(!labels.contains(&"static"), "static tags should not appear (not loaded)");
    }

    #[test]
    fn test_tag_completions_before_load_hides_library_tags() {
        use djls_semantic::LoadKind;
        use djls_semantic::LoadStatement;
        use djls_source::Span;

        let tags = make_test_tags();
        let mut loaded = djls_semantic::LoadedLibraries::new();
        // {% load i18n %} at span 50..65
        loaded.push(LoadStatement {
            span: Span::new(50, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&tags),
            None,
            Some(&loaded),
            10, // cursor BEFORE the load
            false,
            ls_types::Position::new(0, 3),
            "{% ",
            3,
        );

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"if"), "builtins should appear");
        assert!(!labels.contains(&"trans"), "i18n tags should NOT appear before load");
    }

    #[test]
    fn test_tag_completions_selective_import() {
        use djls_semantic::LoadKind;
        use djls_semantic::LoadStatement;
        use djls_source::Span;

        let tags = make_test_tags();
        let mut loaded = djls_semantic::LoadedLibraries::new();
        // {% load trans from i18n %} at span 0..25
        loaded.push(LoadStatement {
            span: Span::new(0, 25),
            kind: LoadKind::Selective {
                symbols: vec!["trans".to_string()],
                library: "i18n".to_string(),
            },
        });

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&tags),
            None,
            Some(&loaded),
            50, // cursor after the load
            false,
            ls_types::Position::new(1, 3),
            "{% ",
            3,
        );

        let labels: Vec<_> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"if"), "builtins should appear");
        assert!(labels.contains(&"trans"), "selectively imported trans should appear");
        assert!(!labels.contains(&"blocktrans"), "non-imported blocktrans should NOT appear");
    }

    #[test]
    fn test_library_completions_deprioritize_already_loaded() {
        use djls_semantic::LoadKind;
        use djls_semantic::LoadStatement;
        use djls_source::Span;

        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );
        libraries.insert(
            "cache".to_string(),
            "django.templatetags.cache".to_string(),
        );

        let tags = InspectorInventory::new(libraries, vec![], vec![], vec![]);

        let mut loaded = djls_semantic::LoadedLibraries::new();
        // {% load i18n %} at span 0..15
        loaded.push(LoadStatement {
            span: Span::new(0, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        let completions = generate_library_completions(
            "",
            &ClosingBrace::None,
            Some(&tags),
            Some(&loaded),
            50, // cursor after the load
        );

        // i18n should be marked as already loaded
        let i18n = completions.iter().find(|c| c.label == "i18n").unwrap();
        assert_eq!(i18n.deprecated, Some(true));
        assert!(i18n.detail.as_ref().unwrap().starts_with("Already loaded"));
        assert!(i18n.sort_text.as_ref().unwrap().starts_with("1_"));

        // static and cache should not be deprioritized
        let static_item = completions.iter().find(|c| c.label == "static").unwrap();
        assert_eq!(static_item.deprecated, Some(false));
        assert!(static_item
            .detail
            .as_ref()
            .unwrap()
            .starts_with("Django template library"));
        assert!(static_item.sort_text.as_ref().unwrap().starts_with("0_"));

        let cache_item = completions.iter().find(|c| c.label == "cache").unwrap();
        assert_eq!(cache_item.deprecated, Some(false));
        assert!(cache_item.sort_text.as_ref().unwrap().starts_with("0_"));
    }

    #[test]
    fn test_library_completions_no_load_info_no_deprioritization() {
        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );

        let tags = InspectorInventory::new(libraries, vec![], vec![], vec![]);

        let completions = generate_library_completions(
            "",
            &ClosingBrace::None,
            Some(&tags),
            None, // no load info
            0,
        );

        // All should be not deprecated and have 0_ sort prefix
        for c in &completions {
            assert_eq!(c.deprecated, Some(false));
            assert!(c.sort_text.as_ref().unwrap().starts_with("0_"));
        }
    }

    #[test]
    fn test_library_completions_before_load_not_deprioritized() {
        use djls_semantic::LoadKind;
        use djls_semantic::LoadStatement;
        use djls_source::Span;

        let mut libraries = std::collections::HashMap::new();
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );

        let tags = InspectorInventory::new(libraries, vec![], vec![], vec![]);

        let mut loaded = djls_semantic::LoadedLibraries::new();
        // {% load i18n %} at span 50..65
        loaded.push(LoadStatement {
            span: Span::new(50, 15),
            kind: LoadKind::Libraries(vec!["i18n".to_string()]),
        });

        let completions = generate_library_completions(
            "",
            &ClosingBrace::None,
            Some(&tags),
            Some(&loaded),
            10, // cursor BEFORE the load
        );

        // i18n should NOT be deprioritized since cursor is before the load
        let i18n = completions.iter().find(|c| c.label == "i18n").unwrap();
        assert_eq!(i18n.deprecated, Some(false));
        assert!(i18n.sort_text.as_ref().unwrap().starts_with("0_"));
    }
}
