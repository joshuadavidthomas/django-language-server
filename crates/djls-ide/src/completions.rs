//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use djls_project::InspectorInventory;
use djls_semantic::available_filters_at;
use djls_semantic::available_tags_at;
use djls_semantic::LoadedLibraries;
use djls_semantic::TagSpecs;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::ls_types;

// TODO(M9 Phase 4): Restore snippet imports when implementing completions
// use crate::snippets::generate_partial_snippet;
// use crate::snippets::generate_snippet_for_tag_with_end;

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

/// Tracks the closing state for variable/filter context (`{{ ... }}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableClosingBrace {
    /// No closing at all: `{{ var|`
    None,
    /// Single brace: `{{ var|fil}`
    Partial,
    /// Complete: `{{ var|fil }}`
    Full,
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
    /// Completing filters after |
    Filter {
        /// Partial filter name typed so far
        partial: String,
        /// Closing brace state
        closing: VariableClosingBrace,
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
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn handle_completion(
    document: &TextDocument,
    position: ls_types::Position,
    encoding: PositionEncoding,
    file_kind: FileKind,
    inspector_inventory: Option<&InspectorInventory>,
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
    let byte_offset = calculate_byte_offset(document, position, encoding);

    // Generate completions based on available template tags
    generate_template_completions(
        &context,
        inspector_inventory,
        tag_specs,
        loaded_libraries,
        byte_offset,
        supports_snippets,
        position,
        &line_info.text,
        line_info.cursor_offset,
    )
}

/// Calculate byte offset from line/character position.
///
/// This converts LSP (line, character) positions into byte offsets
/// into the document content, respecting the position encoding.
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
                // Convert UTF-16 offset to character count
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

        // Convert character offset to byte offset
        byte_offset += line.chars().take(char_offset).map(char::len_utf8).sum::<usize>();
    }

    u32::try_from(byte_offset).unwrap_or(u32::MAX)
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

/// Detect filter completion context in `{{ var| }}` expressions.
///
/// Returns Some if cursor is after a `|` inside a variable expression.
fn analyze_variable_context(
    prefix: &str,
    full_line: &str,
    cursor_offset: usize,
) -> Option<TemplateCompletionContext> {
    // Find the start of the variable expression (last {{ before cursor)
    let var_start = prefix.rfind("{{")?;
    let var_content = &prefix[var_start + 2..]; // Content after {{

    // Check if we're inside a filter (after |)
    let pipe_pos = var_content.rfind('|')?;

    // Everything after the last | is the partial filter name
    let after_pipe = &var_content[pipe_pos + 1..];
    let partial = after_pipe.trim().to_string();

    // Determine closing state by looking at what's after cursor
    let suffix = &full_line[cursor_offset.min(full_line.len())..];
    let closing = detect_variable_closing_brace(suffix);

    Some(TemplateCompletionContext::Filter { partial, closing })
}

/// Detect what closing brace state exists after the cursor for variables
fn detect_variable_closing_brace(suffix: &str) -> VariableClosingBrace {
    let trimmed = suffix.trim_start();
    if trimmed.starts_with("}}") {
        VariableClosingBrace::Full
    } else if trimmed.starts_with('}') {
        VariableClosingBrace::Partial
    } else {
        VariableClosingBrace::None
    }
}

/// Analyze a line of template text to determine completion context
fn analyze_template_context(line: &str, cursor_offset: usize) -> Option<TemplateCompletionContext> {
    let prefix = &line[..cursor_offset.min(line.len())];

    // Check for variable/filter context first ({{ ... |)
    if let Some(var_ctx) = analyze_variable_context(prefix, line, cursor_offset) {
        return Some(var_ctx);
    }

    // Find the last {% before cursor position
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
    inspector_inventory: Option<&InspectorInventory>,
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
            inspector_inventory,
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
            inspector_inventory,
            tag_specs,
            supports_snippets,
        ),
        TemplateCompletionContext::LibraryName { partial, closing } => {
            generate_library_completions(
                partial,
                closing,
                inspector_inventory,
                loaded_libraries,
                cursor_byte_offset,
            )
        }
        TemplateCompletionContext::Filter { partial, closing } => {
            generate_filter_completions(
                partial,
                closing,
                inspector_inventory,
                loaded_libraries,
                cursor_byte_offset,
            )
        }
        TemplateCompletionContext::Variable { .. } | TemplateCompletionContext::None => {
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
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn generate_tag_name_completions(
    partial: &str,
    needs_space: bool,
    closing: &ClosingBrace,
    inspector_inventory: Option<&InspectorInventory>,
    tag_specs: Option<&TagSpecs>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
    _supports_snippets: bool,
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<ls_types::CompletionItem> {
    let Some(inventory) = inspector_inventory else {
        return Vec::new();
    };

    // Compute available tags at cursor position
    let available = loaded_libraries
        .map(|loaded| available_tags_at(loaded, inventory, cursor_byte_offset));

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

    for tag in inventory.tags() {
        // Filter by partial match
        if !tag.name().starts_with(partial) {
            continue;
        }

        // Filter by availability (if we have load info)
        // When inspector unavailable (available = None), show all tags (fallback)
        if let Some(ref avail) = available {
            if !avail.has_tag(tag.name()) {
                // Tag not available at this position - hide it
                continue;
            }
        }

        // Try to get snippet from TagSpecs if available and client supports snippets
        // TODO(M9 Phase 4): Restore snippet generation using ExtractedArg
        let (insert_text, insert_format) =
            build_plain_insert_for_tag(tag.name(), needs_space, closing);

            // Create completion item
            // Use SNIPPET kind when we're inserting a snippet, KEYWORD otherwise
            let kind = if matches!(insert_format, ls_types::InsertTextFormat::SNIPPET) {
                ls_types::CompletionItemKind::SNIPPET
            } else {
                ls_types::CompletionItemKind::KEYWORD
            };

            let completion_item = ls_types::CompletionItem {
                label: tag.name().to_string(),
                kind: Some(kind),
                detail: Some(if let Some(lib) = tag.library_load_name() {
                    format!("from {} ({{% load {} %}})", tag.defining_module(), lib)
                } else {
                    format!("builtin from {}", tag.defining_module())
                }),
                documentation: tag
                    .doc()
                    .map(|doc| ls_types::Documentation::String(doc.to_string())),
                text_edit: Some(tower_lsp_server::ls_types::CompletionTextEdit::Edit(
                    ls_types::TextEdit::new(replacement_range, insert_text.clone()),
                )),
                insert_text_format: Some(insert_format),
                filter_text: Some(tag.name().to_string()),
                sort_text: Some(format!("1_{}", tag.name())), // Regular tags sort after end tags
                ..Default::default()
            };

            completions.push(completion_item);
    }

    completions
}

/// Generate completions for tag arguments
#[allow(clippy::too_many_arguments)]
fn generate_argument_completions(
    _tag: &str,
    _position: usize,
    _partial: &str,
    _parsed_args: &[String],
    _closing: &ClosingBrace,
    _inspector_inventory: Option<&InspectorInventory>,
    _tag_specs: Option<&TagSpecs>,
    _supports_snippets: bool,
) -> Vec<ls_types::CompletionItem> {
    // TODO(M9 Phase 4): Reimplement using ExtractedArg from extraction
    // This was previously implemented using TagArg which has been removed.
    Vec::new()
}

/// Generate completions for library names (for {% load %} tag)
fn generate_library_completions(
    partial: &str,
    closing: &ClosingBrace,
    inspector_inventory: Option<&InspectorInventory>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
) -> Vec<ls_types::CompletionItem> {
    let Some(inventory) = inspector_inventory else {
        return Vec::new();
    };

    // Collect and sort library names for deterministic ordering
    let mut library_entries: Vec<_> = inventory
        .libraries()
        .iter()
        .filter(|(load_name, _)| load_name.starts_with(partial))
        .collect();
    library_entries.sort_by_key(|(load_name, _)| load_name.as_str());

    // Get already-loaded libraries to deprioritize them
    let already_loaded = loaded_libraries
        .map(|l| l.libraries_before(cursor_byte_offset))
        .unwrap_or_default();

    let mut completions = Vec::new();

    for (load_name, module_path) in library_entries {
        let is_already_loaded = already_loaded.contains(load_name);

        let mut insert_text = load_name.clone();

        // Add closing if needed
        match closing {
            ClosingBrace::None => insert_text.push_str(" %}"),
            ClosingBrace::PartialClose => insert_text.push_str(" %"),
            ClosingBrace::FullClose => {} // No closing needed
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
            // Deprioritize already-loaded libraries
            sort_text: Some(if is_already_loaded {
                format!("1_{load_name}")
            } else {
                format!("0_{load_name}")
            }),
            // Mark deprecated if already loaded (shows strikethrough in some editors)
            deprecated: Some(is_already_loaded),
            ..Default::default()
        });
    }

    completions
}

/// Generate completions for filter names in `{{ var|` context.
fn generate_filter_completions(
    partial: &str,
    closing: &VariableClosingBrace,
    inspector_inventory: Option<&InspectorInventory>,
    loaded_libraries: Option<&LoadedLibraries>,
    cursor_byte_offset: u32,
) -> Vec<ls_types::CompletionItem> {
    let Some(inventory) = inspector_inventory else {
        return Vec::new();
    };

    // Compute available filters at cursor position (reuse M3 infrastructure)
    let available = loaded_libraries
        .map(|loaded| available_filters_at(loaded, inventory, cursor_byte_offset));

    let mut completions = Vec::new();

    for filter in inventory.filters() {
        // Filter by partial match
        if !filter.name().starts_with(partial) {
            continue;
        }

        // Filter by availability (if we have load info)
        if let Some(ref avail) = available {
            if !avail.has_filter(filter.name()) {
                continue;
            }
        }

        let mut insert_text = filter.name().to_string();

        // Add closing if needed
        match closing {
            VariableClosingBrace::None => insert_text.push_str(" }}"),
            VariableClosingBrace::Partial => insert_text.push('}'),
            VariableClosingBrace::Full => {} // No closing needed
        }

        completions.push(ls_types::CompletionItem {
            label: filter.name().to_string(),
            kind: Some(ls_types::CompletionItemKind::FUNCTION),
            detail: Some(if let Some(lib) = filter.library_load_name() {
                format!("Filter from {} ({{% load {} %}})", filter.defining_module(), lib)
            } else {
                format!("Builtin filter from {}", filter.defining_module())
            }),
            documentation: filter.doc().map(|d| {
                ls_types::Documentation::MarkupContent(ls_types::MarkupContent {
                    kind: ls_types::MarkupKind::Markdown,
                    value: d.to_string(),
                })
            }),
            insert_text: Some(insert_text),
            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
            filter_text: Some(filter.name().to_string()),
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
}
