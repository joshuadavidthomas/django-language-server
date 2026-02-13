//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use djls_project::InstalledSymbolCandidate;
use djls_project::InstalledSymbolOrigin;
use djls_project::Knowledge;
use djls_project::TemplateLibraries;
use djls_project::TemplateSymbol;
use djls_project::TemplateSymbolKind;
use djls_semantic::AvailableSymbols;
use djls_semantic::CompletionArgKind;
use djls_semantic::TagSpecs;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::ls_types;

use crate::snippets::generate_partial_snippet;
use crate::snippets::generate_snippet_for_tag_with_end;

fn symbol_is_available(symbol: &TemplateSymbol, available: &AvailableSymbols) -> bool {
    match symbol.kind {
        TemplateSymbolKind::Tag => available.available_tags().contains(symbol.name()),
        TemplateSymbolKind::Filter => available.available_filters().contains(symbol.name()),
    }
}

fn symbol_completion_kind(symbol: &TemplateSymbol) -> ls_types::CompletionItemKind {
    match symbol.kind {
        TemplateSymbolKind::Tag => ls_types::CompletionItemKind::KEYWORD,
        TemplateSymbolKind::Filter => ls_types::CompletionItemKind::FUNCTION,
    }
}

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
    },
}

/// Information about a line of text and cursor position within it
#[derive(Debug)]
pub struct LineInfo {
    /// The complete line text
    pub text: String,
    /// The cursor byte offset within the line (safe for `line[..offset]` slicing)
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
    template_libraries: Option<&TemplateLibraries>,
    tag_specs: Option<&TagSpecs>,
    available_symbols: Option<&AvailableSymbols>,
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

    // Generate completions based on available template tags
    generate_template_completions(
        &context,
        template_libraries,
        tag_specs,
        available_symbols,
        supports_snippets,
        position,
        &line_info.text,
        line_info.cursor_offset,
    )
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

    // Convert LSP position to a byte offset within the line.
    //
    // All downstream consumers do byte-based string slicing (`line[..offset]`),
    // so we must produce a byte offset, not a char count.
    //
    // LSP encodings:
    //   - UTF-16 (default, VS Code): `position.character` counts UTF-16 code units
    //   - UTF-32: `position.character` counts Unicode scalar values (codepoints)
    //   - UTF-8: `position.character` is already a byte offset
    let cursor_byte_offset = match encoding {
        PositionEncoding::Utf16 => {
            let utf16_pos = position.character as usize;
            let mut byte_offset = 0;
            let mut utf16_offset = 0;

            for ch in line_text.chars() {
                if utf16_offset >= utf16_pos {
                    break;
                }
                utf16_offset += ch.len_utf16();
                byte_offset += ch.len_utf8();
            }
            byte_offset
        }
        PositionEncoding::Utf32 => {
            let char_pos = position.character as usize;
            let mut byte_offset = 0;

            for (i, ch) in line_text.chars().enumerate() {
                if i >= char_pos {
                    break;
                }
                byte_offset += ch.len_utf8();
            }
            byte_offset
        }
        PositionEncoding::Utf8 => position.character as usize,
    };

    let clamped_offset = cursor_byte_offset.min(line_text.len());

    Some(LineInfo {
        text: line_text,
        cursor_offset: clamped_offset,
    })
}

/// Analyze a line of template text to determine completion context
fn analyze_template_context(line: &str, cursor_offset: usize) -> Option<TemplateCompletionContext> {
    let prefix = &line[..cursor_offset.min(line.len())];

    // Find the last {{ or {% before cursor position, choosing the nearest one
    let var_start = prefix.rfind("{{");
    let tag_start = prefix.rfind("{%");

    // Check if we're inside a variable expression ({{ ... }})
    // A variable start is only valid if it's the closest template delimiter
    if let Some(vs) = var_start {
        let is_var_closer = tag_start.is_none() || tag_start.unwrap() < vs;
        if is_var_closer {
            return analyze_variable_context(prefix, vs);
        }
    }

    let tag_start = tag_start?;

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

/// Analyze a variable expression context (`{{ ... }}`) to detect filter completion.
///
/// Returns `Some(Filter { partial })` when the cursor is after a pipe character
/// inside a variable expression, indicating the user is typing a filter name.
fn analyze_variable_context(prefix: &str, var_start: usize) -> Option<TemplateCompletionContext> {
    // Get content between {{ and cursor
    let content_start = var_start + 2;
    let content = &prefix[content_start..];

    // Look for the last pipe character (not inside quotes) to detect filter context
    let last_pipe = find_last_unquoted_pipe(content)?;

    // Extract partial filter name after the last pipe
    let after_pipe = &content[last_pipe + 1..];
    let partial = after_pipe.trim_start().to_string();

    Some(TemplateCompletionContext::Filter { partial })
}

/// Find the position of the last pipe character (`|`) that is not inside quotes.
///
/// Handles escaped quotes (e.g., `\"` or `\'`) by counting consecutive
/// preceding backslashes — a quote is only a real delimiter when preceded
/// by an even number of backslashes.
///
/// Returns `None` if no unquoted pipe is found.
fn find_last_unquoted_pipe(s: &str) -> Option<usize> {
    let mut last_pipe = None;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let bytes = s.as_bytes();

    for (i, ch) in s.char_indices() {
        match ch {
            '\'' if !in_double_quote => {
                let num_backslashes = bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count();
                if num_backslashes % 2 == 0 {
                    in_single_quote = !in_single_quote;
                }
            }
            '"' if !in_single_quote => {
                let num_backslashes = bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count();
                if num_backslashes % 2 == 0 {
                    in_double_quote = !in_double_quote;
                }
            }
            '|' if !in_single_quote && !in_double_quote => last_pipe = Some(i),
            _ => {}
        }
    }

    last_pipe
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
    template_libraries: Option<&TemplateLibraries>,
    tag_specs: Option<&TagSpecs>,
    available_symbols: Option<&AvailableSymbols>,
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
            template_libraries,
            tag_specs,
            available_symbols,
            supports_snippets,
            position,
            line_text,
            cursor_offset,
        ),
        TemplateCompletionContext::TagArgument {
            tag,
            position,
            partial,
            closing,
            ..
        } => generate_argument_completions(
            tag,
            *position,
            partial,
            closing,
            tag_specs,
            supports_snippets,
        ),
        TemplateCompletionContext::LibraryName { partial, closing } => {
            generate_library_completions(partial, closing, template_libraries)
        }
        TemplateCompletionContext::Filter { partial } => {
            generate_filter_completions(partial, template_libraries, available_symbols)
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
        if line_text.as_bytes().get(cursor_offset) == Some(&b'}') {
            end_col += 1;
        }
    }
    let end = ls_types::Position::new(position.line, end_col);

    ls_types::Range::new(start, end)
}

#[allow(clippy::too_many_arguments)]
fn generate_discovered_tag_name_completions(
    partial: &str,
    needs_space: bool,
    closing: &ClosingBrace,
    template_libraries: &TemplateLibraries,
    _supports_snippets: bool,
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<ls_types::CompletionItem> {
    let names: Vec<String> = template_libraries
        .discovered_symbol_names(TemplateSymbolKind::Tag)
        .into_iter()
        .map(|name| name.as_str().to_string())
        .collect();

    let replacement_range =
        calculate_replacement_range(position, line_text, cursor_offset, partial.len(), closing);

    names
        .into_iter()
        .filter(|name| name.starts_with(partial))
        .map(|name| {
            let (insert_text, insert_text_format) =
                build_plain_insert_for_tag(&name, needs_space, closing);

            ls_types::CompletionItem {
                label: name.clone(),
                kind: Some(ls_types::CompletionItemKind::KEYWORD),
                detail: Some("scanned tag".to_string()),
                text_edit: Some(tower_lsp_server::ls_types::CompletionTextEdit::Edit(
                    ls_types::TextEdit::new(replacement_range, insert_text),
                )),
                insert_text_format: Some(insert_text_format),
                filter_text: Some(name),
                ..Default::default()
            }
        })
        .collect()
}

/// Generate completions for tag names
///
/// When `available_symbols` is `Some`, only tags that are available at the cursor
/// position (builtins + tags from loaded libraries) are shown. When `None` (inspector
/// unavailable), all tags from `template_tags` are shown as a fallback.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn generate_tag_name_completions(
    partial: &str,
    needs_space: bool,
    closing: &ClosingBrace,
    template_libraries: Option<&TemplateLibraries>,
    tag_specs: Option<&TagSpecs>,
    available_symbols: Option<&AvailableSymbols>,
    supports_snippets: bool,
    position: ls_types::Position,
    line_text: &str,
    cursor_offset: usize,
) -> Vec<ls_types::CompletionItem> {
    let Some(template_libraries) = template_libraries else {
        return Vec::new();
    };

    if template_libraries.inspector_knowledge != Knowledge::Known {
        return generate_discovered_tag_name_completions(
            partial,
            needs_space,
            closing,
            template_libraries,
            supports_snippets,
            position,
            line_text,
            cursor_offset,
        );
    }

    let tags = template_libraries.installed_symbol_candidates(TemplateSymbolKind::Tag);

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

    for tag in tags {
        let symbol = &tag.symbol;

        if let Some(symbols) = available_symbols {
            if !symbol_is_available(symbol, symbols) {
                continue;
            }
        }

        let tag_name = symbol.name();

        if tag_name.starts_with(partial) {
            // Try to get snippet from TagSpecs if available and client supports snippets
            let (insert_text, insert_format) = if supports_snippets {
                if let Some(specs) = tag_specs {
                    if let Some(spec) = specs.get(tag_name) {
                        let has_args = !spec.completion_args().is_empty();
                        if has_args {
                            // Generate snippet from tag spec
                            let mut text = String::new();

                            // Add leading space if needed
                            if needs_space {
                                text.push(' ');
                            }

                            // Generate the snippet
                            let snippet = generate_snippet_for_tag_with_end(tag_name, spec);
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

                            (text, ls_types::InsertTextFormat::SNIPPET)
                        } else {
                            // No args, use plain text
                            build_plain_insert_for_tag(tag_name, needs_space, closing)
                        }
                    } else {
                        // No spec found, use plain text
                        build_plain_insert_for_tag(tag_name, needs_space, closing)
                    }
                } else {
                    // No specs available, use plain text
                    build_plain_insert_for_tag(tag_name, needs_space, closing)
                }
            } else {
                // Client doesn't support snippets
                build_plain_insert_for_tag(tag_name, needs_space, closing)
            };

            let kind = if matches!(insert_format, ls_types::InsertTextFormat::SNIPPET) {
                ls_types::CompletionItemKind::SNIPPET
            } else {
                symbol_completion_kind(symbol)
            };

            let detail = match &tag.origin {
                InstalledSymbolOrigin::Builtin { module } => {
                    format!("builtin from {}", module.as_str())
                }
                InstalledSymbolOrigin::Loadable { load_name } => {
                    format!("{{% load {} %}}", load_name.as_str())
                }
            };

            let completion_item = ls_types::CompletionItem {
                label: tag_name.to_string(),
                kind: Some(kind),
                detail: Some(detail),
                documentation: symbol
                    .doc()
                    .map(|doc| ls_types::Documentation::String(doc.to_string())),
                text_edit: Some(tower_lsp_server::ls_types::CompletionTextEdit::Edit(
                    ls_types::TextEdit::new(replacement_range, insert_text.clone()),
                )),
                insert_text_format: Some(insert_format),
                filter_text: Some(tag_name.to_string()),
                sort_text: Some(format!("1_{tag_name}")), // Regular tags sort after end tags
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
    closing: &ClosingBrace,
    tag_specs: Option<&TagSpecs>,
    supports_snippets: bool,
) -> Vec<ls_types::CompletionItem> {
    let Some(specs) = tag_specs else {
        return Vec::new();
    };

    let Some(spec) = specs.get(tag) else {
        return Vec::new();
    };

    let args = spec.completion_args();

    if position >= args.len() {
        return Vec::new();
    }

    let arg = &args[position];
    let mut completions = Vec::new();

    match &arg.kind {
        CompletionArgKind::Literal(value) => {
            // For literals, complete the exact text
            if value.starts_with(partial) {
                let mut insert_text = value.clone();

                match closing {
                    ClosingBrace::PartialClose | ClosingBrace::None => {
                        insert_text.push_str(" %}");
                    }
                    ClosingBrace::FullClose => {}
                }

                completions.push(ls_types::CompletionItem {
                    label: value.clone(),
                    kind: Some(ls_types::CompletionItemKind::KEYWORD),
                    detail: Some("literal argument".to_string()),
                    insert_text: Some(insert_text),
                    insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        CompletionArgKind::Choice(choices) => {
            for option in choices {
                if option.starts_with(partial) {
                    let mut insert_text = option.clone();

                    match closing {
                        ClosingBrace::PartialClose | ClosingBrace::None => {
                            insert_text.push_str(" %}");
                        }
                        ClosingBrace::FullClose => {}
                    }

                    completions.push(ls_types::CompletionItem {
                        label: option.clone(),
                        kind: Some(ls_types::CompletionItemKind::ENUM_MEMBER),
                        detail: Some(format!("choice for {}", arg.name)),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                        ..Default::default()
                    });
                }
            }
        }
        CompletionArgKind::Variable | CompletionArgKind::Keyword => {
            if partial.is_empty() {
                completions.push(ls_types::CompletionItem {
                    label: format!("<{}>", arg.name),
                    kind: Some(ls_types::CompletionItemKind::VARIABLE),
                    detail: Some("variable argument".to_string()),
                    insert_text: None,
                    insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        CompletionArgKind::VarArgs => {
            // VarArgs not specifically completed
        }
    }

    // If we're at the start of an argument position and client supports snippets,
    // offer a snippet for all remaining arguments
    if partial.is_empty() && supports_snippets && position < args.len() {
        let remaining_snippet = generate_partial_snippet(spec, position);
        if !remaining_snippet.is_empty() {
            let mut insert_text = remaining_snippet;

            match closing {
                ClosingBrace::PartialClose | ClosingBrace::None => {
                    insert_text.push_str(" %}");
                }
                ClosingBrace::FullClose => {}
            }

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
                sort_text: Some("zzz".to_string()),
                ..Default::default()
            });
        }
    }

    completions
}

/// Generate completions for library names (for {% load %} tag).
///
/// When `template_libraries` is `None`, returns an empty list since we have no
/// knowledge of which libraries are available.
fn generate_library_completions(
    partial: &str,
    closing: &ClosingBrace,
    template_libraries: Option<&TemplateLibraries>,
) -> Vec<ls_types::CompletionItem> {
    let Some(template_libraries) = template_libraries else {
        return Vec::new();
    };

    let names = template_libraries.completion_library_names();

    let mut completions = Vec::new();

    for load_name in names {
        if load_name.as_str().starts_with(partial) {
            let load_name_str = load_name.as_str().to_string();
            let mut insert_text = load_name_str.clone();

            // Add closing if needed
            match closing {
                ClosingBrace::PartialClose | ClosingBrace::None => {
                    insert_text.push_str(" %}");
                }
                ClosingBrace::FullClose => {}
            }

            let detail = template_libraries
                .loadable_library_module(&load_name)
                .map_or_else(
                    || "Django template library".to_string(),
                    |module| format!("Django template library ({})", module.as_str()),
                );

            completions.push(ls_types::CompletionItem {
                label: load_name_str.clone(),
                kind: Some(ls_types::CompletionItemKind::MODULE),
                detail: Some(detail),
                insert_text: Some(insert_text),
                insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
                filter_text: Some(load_name_str.clone()),
                ..Default::default()
            });
        }
    }

    completions
}

/// Generate completion items from a collection of template symbols.
///
/// When `available_symbols` is `Some` (inspector healthy), only symbols that are
/// available at the cursor position (builtins + loaded library symbols) are shown.
fn generate_completions(
    symbols: impl IntoIterator<Item = InstalledSymbolCandidate>,
    partial: &str,
    available_symbols: Option<&AvailableSymbols>,
) -> Vec<ls_types::CompletionItem> {
    let mut completions = Vec::new();

    for candidate in symbols {
        let symbol = &candidate.symbol;

        if let Some(avail) = available_symbols {
            if !symbol_is_available(symbol, avail) {
                continue;
            }
        }

        let name = symbol.name();
        if !name.starts_with(partial) {
            continue;
        }

        let detail = match &candidate.origin {
            InstalledSymbolOrigin::Builtin { module } => match symbol.kind {
                TemplateSymbolKind::Tag => format!("builtin from {}", module.as_str()),
                TemplateSymbolKind::Filter => "builtin filter".to_string(),
            },
            InstalledSymbolOrigin::Loadable { load_name } => {
                format!("{{% load {} %}}", load_name.as_str())
            }
        };

        completions.push(ls_types::CompletionItem {
            label: name.to_string(),
            kind: Some(symbol_completion_kind(symbol)),
            detail: Some(detail),
            documentation: symbol
                .doc()
                .map(|doc| ls_types::Documentation::String(doc.to_string())),
            insert_text: Some(name.to_string()),
            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
            filter_text: Some(name.to_string()),
            sort_text: Some(format!("1_{name}")),
            ..Default::default()
        });
    }

    completions.sort_by(|a, b| a.label.cmp(&b.label));
    completions.dedup_by(|a, b| a.label == b.label);

    completions
}

/// Generate completions for filter names in `{{ var|filter }}` context.
///
/// When `available_symbols` is `Some` (inspector healthy), only shows builtin filters
/// and filters from loaded libraries at the cursor position. When `None` (inspector
/// unavailable), shows all known filters as a fallback.
fn generate_filter_completions(
    partial: &str,
    template_libraries: Option<&TemplateLibraries>,
    available_symbols: Option<&AvailableSymbols>,
) -> Vec<ls_types::CompletionItem> {
    let Some(template_libraries) = template_libraries else {
        return Vec::new();
    };

    if template_libraries.inspector_knowledge == Knowledge::Known {
        let filters = template_libraries.installed_symbol_candidates(TemplateSymbolKind::Filter);
        return generate_completions(filters, partial, available_symbols);
    }

    let names: Vec<String> = template_libraries
        .discovered_symbol_names(TemplateSymbolKind::Filter)
        .into_iter()
        .map(|name| name.as_str().to_string())
        .collect();

    names
        .into_iter()
        .filter(|name| name.starts_with(partial))
        .map(|name| ls_types::CompletionItem {
            label: name.clone(),
            kind: Some(ls_types::CompletionItemKind::FUNCTION),
            detail: Some("scanned filter".to_string()),
            insert_text: Some(name),
            insert_text_format: Some(ls_types::InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        })
        .collect()
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
    use std::collections::BTreeMap;
    use std::collections::HashMap;

    use djls_semantic::CompletionArg;

    use super::*;

    fn symbol(
        kind: TemplateSymbolKind,
        name: &str,
        load_name: Option<&str>,
        library_module: &str,
        module: &str,
        doc: Option<&str>,
    ) -> djls_project::InspectorLibrarySymbol {
        djls_project::InspectorLibrarySymbol {
            kind: Some(kind),
            name: name.to_string(),
            load_name: load_name.map(str::to_string),
            library_module: library_module.to_string(),
            module: module.to_string(),
            doc: doc.map(str::to_string),
        }
    }

    fn response_from_symbols(
        symbols: Vec<djls_project::InspectorLibrarySymbol>,
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> djls_project::TemplateLibrariesResponse {
        djls_project::TemplateLibrariesResponse {
            symbols,
            libraries: libraries
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            builtins: builtins.to_vec(),
        }
    }

    fn build_template_libraries(
        libraries: &HashMap<String, String>,
        builtins: &[String],
    ) -> TemplateLibraries {
        let mut symbols = Vec::new();

        for module in builtins {
            symbols.push(symbol(
                TemplateSymbolKind::Tag,
                &format!(
                    "builtin_from_{}",
                    module.split('.').next_back().unwrap_or("unknown")
                ),
                None,
                module,
                module,
                None,
            ));
        }

        for (load_name, module) in libraries {
            symbols.push(symbol(
                TemplateSymbolKind::Tag,
                &format!("{load_name}_tag"),
                Some(load_name),
                module,
                module,
                None,
            ));
        }

        let response = response_from_symbols(symbols, libraries, builtins);

        TemplateLibraries::default().apply_inspector(Some(response))
    }

    #[test]
    fn test_library_completions_show_load_names_not_module_paths() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("", &ClosingBrace::None, Some(&libs));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"static"));
        assert!(labels.contains(&"i18n"));
        // Should NOT contain module paths
        assert!(!labels.contains(&"django.templatetags.static"));
        assert!(!labels.contains(&"django.templatetags.i18n"));
    }

    #[test]
    fn test_library_completions_deterministic_alphabetical_order() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "admin_list".to_string(),
            "django.contrib.admin.templatetags.admin_list".to_string(),
        );
        libraries.insert("tz".to_string(), "django.templatetags.tz".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("", &ClosingBrace::None, Some(&libs));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["admin_list", "i18n", "static", "tz"]);
    }

    #[test]
    fn test_library_completions_builtins_excluded() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        let libs = build_template_libraries(&libraries, &builtins);

        let completions = generate_library_completions("", &ClosingBrace::None, Some(&libs));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // Only the library, not builtins
        assert_eq!(labels, vec!["static"]);
    }

    #[test]
    fn test_library_completions_partial_prefix_filtering() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "staticfiles".to_string(),
            "django.contrib.staticfiles.templatetags.staticfiles".to_string(),
        );

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("stat", &ClosingBrace::None, Some(&libs));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["static", "staticfiles"]);
        // i18n should be filtered out
        assert!(!labels.contains(&"i18n"));
    }

    #[test]
    fn test_library_completions_detail_shows_module_path() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("", &ClosingBrace::None, Some(&libs));

        assert_eq!(completions.len(), 1);
        assert_eq!(
            completions[0].detail.as_deref(),
            Some("Django template library (django.templatetags.static)")
        );
    }

    #[test]
    fn test_library_completions_inspector_unavailable_returns_empty() {
        let completions = generate_library_completions("", &ClosingBrace::None, None);
        assert!(
            completions.is_empty(),
            "Library completions should be empty when inspector is unavailable"
        );
    }

    #[test]
    fn test_library_completions_inspector_unavailable_with_partial_returns_empty() {
        let completions = generate_library_completions("stat", &ClosingBrace::None, None);
        assert!(
            completions.is_empty(),
            "Library completions should be empty even with partial input when inspector is unavailable"
        );
    }

    #[test]
    fn test_library_completions_healthy_inspector_returns_names() {
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert("tz".to_string(), "django.templatetags.tz".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("", &ClosingBrace::None, Some(&libs));

        assert_eq!(completions.len(), 3);
        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["i18n", "static", "tz"]);

        // Each has MODULE kind
        for c in &completions {
            assert_eq!(c.kind, Some(ls_types::CompletionItemKind::MODULE));
        }
    }

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

    // Helper to build AvailableSymbols for testing load-scoped completions
    fn build_available_symbols<'a>(
        template_libraries: &'a TemplateLibraries,
        loaded_libs: &'a djls_semantic::LoadedLibraries,
        position: u32,
    ) -> AvailableSymbols<'a> {
        AvailableSymbols::at_position(loaded_libs, template_libraries, position)
    }

    fn make_load_statement(
        span: (u32, u32),
        kind: djls_semantic::LoadKind,
    ) -> djls_semantic::LoadStatement {
        djls_semantic::LoadStatement::new(djls_source::Span::new(span.0, span.1), kind)
    }

    fn build_test_libraries() -> TemplateLibraries {
        use std::collections::BTreeMap;

        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        let symbols = vec![
            symbol(
                TemplateSymbolKind::Tag,
                "if",
                None,
                "django.template.defaulttags",
                "django.template.defaulttags",
                None,
            ),
            symbol(
                TemplateSymbolKind::Tag,
                "for",
                None,
                "django.template.defaulttags",
                "django.template.defaulttags",
                None,
            ),
            symbol(
                TemplateSymbolKind::Tag,
                "block",
                None,
                "django.template.defaulttags",
                "django.template.defaulttags",
                None,
            ),
            symbol(
                TemplateSymbolKind::Tag,
                "trans",
                Some("i18n"),
                "django.templatetags.i18n",
                "django.templatetags.i18n",
                None,
            ),
            symbol(
                TemplateSymbolKind::Tag,
                "blocktrans",
                Some("i18n"),
                "django.templatetags.i18n",
                "django.templatetags.i18n",
                None,
            ),
            symbol(
                TemplateSymbolKind::Tag,
                "get_static_prefix",
                Some("static"),
                "django.templatetags.static",
                "django.templatetags.static",
                None,
            ),
        ];

        let response = djls_project::TemplateLibrariesResponse {
            symbols,
            libraries: libraries.into_iter().collect::<BTreeMap<String, String>>(),
            builtins,
        };

        TemplateLibraries::default().apply_inspector(Some(response))
    }

    #[test]
    fn test_tag_completions_before_any_load_only_builtins() {
        let template_libraries = build_test_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (100, 20),
            djls_semantic::LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // Position 10 = before any load
        let symbols = build_available_symbols(&template_libraries, &loaded, 10);

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&template_libraries),
            None,
            Some(&symbols),
            false,
            ls_types::Position::new(0, 0),
            "{% ",
            3,
        );

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // Builtins should be present
        assert!(labels.contains(&"if"));
        assert!(labels.contains(&"for"));
        assert!(labels.contains(&"block"));
        // Library tags should NOT be present (not loaded yet)
        assert!(!labels.contains(&"trans"));
        assert!(!labels.contains(&"blocktrans"));
        assert!(!labels.contains(&"get_static_prefix"));
    }

    #[test]
    fn test_tag_completions_after_load_shows_library_tags() {
        let template_libraries = build_test_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (10, 20),
            djls_semantic::LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // Position 100 = after load
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&template_libraries),
            None,
            Some(&symbols),
            false,
            ls_types::Position::new(0, 0),
            "{% ",
            3,
        );

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // Builtins present
        assert!(labels.contains(&"if"));
        assert!(labels.contains(&"for"));
        // i18n tags present (loaded)
        assert!(labels.contains(&"trans"));
        assert!(labels.contains(&"blocktrans"));
        // static tags NOT present (not loaded)
        assert!(!labels.contains(&"get_static_prefix"));
    }

    #[test]
    fn test_tag_completions_selective_import_only_imported_symbols() {
        let template_libraries = build_test_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (10, 30),
            djls_semantic::LoadKind::SelectiveImport {
                symbols: vec!["trans".into()],
                library: "i18n".into(),
            },
        )]);

        // Position 100 = after selective load
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&template_libraries),
            None,
            Some(&symbols),
            false,
            ls_types::Position::new(0, 0),
            "{% ",
            3,
        );

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // trans selectively imported → present
        assert!(labels.contains(&"trans"));
        // blocktrans NOT imported → absent
        assert!(!labels.contains(&"blocktrans"));
        // builtins always present
        assert!(labels.contains(&"if"));
    }

    #[test]
    fn test_tag_completions_inspector_unavailable_shows_all_tags() {
        let template_libraries = build_test_libraries();

        // No available_symbols = inspector unavailable → show all tags
        let completions = generate_tag_name_completions(
            "",
            false,
            &ClosingBrace::None,
            Some(&template_libraries),
            None,
            None, // no available symbols
            false,
            ls_types::Position::new(0, 0),
            "{% ",
            3,
        );

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // ALL tags shown (fallback behavior)
        assert!(labels.contains(&"if"));
        assert!(labels.contains(&"for"));
        assert!(labels.contains(&"block"));
        assert!(labels.contains(&"trans"));
        assert!(labels.contains(&"blocktrans"));
        assert!(labels.contains(&"get_static_prefix"));
    }

    #[test]
    fn test_tag_completions_partial_filtering_with_scoping() {
        let template_libraries = build_test_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (10, 20),
            djls_semantic::LoadKind::FullLoad {
                libraries: vec!["i18n".into()],
            },
        )]);

        // Position 100 = after load, partial = "bl"
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions = generate_tag_name_completions(
            "bl",
            false,
            &ClosingBrace::None,
            Some(&template_libraries),
            None,
            Some(&symbols),
            false,
            ls_types::Position::new(0, 0),
            "{% bl",
            5,
        );

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // "block" (builtin, starts with "bl") → present
        assert!(labels.contains(&"block"));
        // "blocktrans" (i18n loaded, starts with "bl") → present
        assert!(labels.contains(&"blocktrans"));
        // "if", "for", "trans" don't start with "bl" → absent
        assert!(!labels.contains(&"if"));
        assert!(!labels.contains(&"trans"));
    }

    // --- Filter completion tests ---

    fn build_test_filter_libraries() -> TemplateLibraries {
        let tags = vec![symbol(
            TemplateSymbolKind::Tag,
            "if",
            None,
            "django.template.defaulttags",
            "django.template.defaulttags",
            None,
        )];

        let filters = vec![
            symbol(
                TemplateSymbolKind::Filter,
                "lower",
                None,
                "django.template.defaultfilters",
                "django.template.defaultfilters",
                Some("Convert a string to lowercase."),
            ),
            symbol(
                TemplateSymbolKind::Filter,
                "title",
                None,
                "django.template.defaultfilters",
                "django.template.defaultfilters",
                None,
            ),
            symbol(
                TemplateSymbolKind::Filter,
                "default",
                None,
                "django.template.defaultfilters",
                "django.template.defaultfilters",
                None,
            ),
            symbol(
                TemplateSymbolKind::Filter,
                "intcomma",
                Some("humanize"),
                "django.contrib.humanize.templatetags.humanize",
                "django.contrib.humanize.templatetags.humanize",
                Some("Converts an integer to a string containing commas."),
            ),
            symbol(
                TemplateSymbolKind::Filter,
                "naturaltime",
                Some("humanize"),
                "django.contrib.humanize.templatetags.humanize",
                "django.contrib.humanize.templatetags.humanize",
                None,
            ),
            symbol(
                TemplateSymbolKind::Filter,
                "localize",
                Some("l10n"),
                "django.templatetags.l10n",
                "django.templatetags.l10n",
                None,
            ),
        ];

        let mut libraries = HashMap::new();
        libraries.insert(
            "humanize".to_string(),
            "django.contrib.humanize.templatetags.humanize".to_string(),
        );
        libraries.insert("l10n".to_string(), "django.templatetags.l10n".to_string());

        let builtins = vec![
            "django.template.defaulttags".to_string(),
            "django.template.defaultfilters".to_string(),
        ];

        let mut symbols: Vec<djls_project::InspectorLibrarySymbol> = tags;
        symbols.extend(filters);

        let response = djls_project::TemplateLibrariesResponse {
            symbols,
            libraries: libraries.into_iter().collect::<BTreeMap<String, String>>(),
            builtins,
        };

        TemplateLibraries::default().apply_inspector(Some(response))
    }

    #[test]
    fn test_analyze_variable_context_pipe_detected() {
        let line = "{{ value|";
        let cursor_offset = 9;

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::Filter {
                partial: String::new(),
            }
        );
    }

    #[test]
    fn test_analyze_variable_context_partial_filter() {
        let line = "{{ value|def";
        let cursor_offset = 12;

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::Filter {
                partial: "def".to_string(),
            }
        );
    }

    #[test]
    fn test_analyze_variable_context_chained_filter() {
        let line = "{{ value|lower|tit";
        let cursor_offset = 18;

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::Filter {
                partial: "tit".to_string(),
            }
        );
    }

    #[test]
    fn test_analyze_variable_context_pipe_after_arg() {
        // After a filter with argument and a new pipe, cursor is after the pipe
        let line = "{{ value|default:'nothing'|";
        let cursor_offset = 27;

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::Filter {
                partial: String::new(),
            }
        );
    }

    #[test]
    fn test_analyze_variable_context_no_pipe_returns_none() {
        // No pipe = not in filter context; variable completions not yet implemented
        let line = "{{ value";
        let cursor_offset = 8;

        let context = analyze_template_context(line, cursor_offset);

        // Variable context without pipe doesn't match any completion context
        assert!(context.is_none());
    }

    #[test]
    fn test_analyze_variable_context_pipe_inside_quotes_ignored() {
        // Pipe inside quotes should not be treated as filter separator
        let line = "{{ value|default:\"a|b\"";
        let cursor_offset = 21;

        let context = analyze_template_context(line, cursor_offset);

        // The last unquoted pipe is at position 8, so partial is `default:"a|b"`
        // This is a valid filter context — cursor is after the pipe
        assert!(matches!(
            context,
            Some(TemplateCompletionContext::Filter { .. })
        ));
    }

    #[test]
    fn test_filter_completions_all_builtins_with_empty_partial() {
        let template_libraries = build_test_filter_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![]);
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions =
            generate_filter_completions("", Some(&template_libraries), Some(&symbols));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // Builtin filters always present
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"title"));
        assert!(labels.contains(&"default"));
        // Library filters NOT present (not loaded)
        assert!(!labels.contains(&"intcomma"));
        assert!(!labels.contains(&"naturaltime"));
        assert!(!labels.contains(&"localize"));
    }

    #[test]
    fn test_filter_completions_partial_prefix_filtering() {
        let template_libraries = build_test_filter_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![]);
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions =
            generate_filter_completions("def", Some(&template_libraries), Some(&symbols));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["default"]);
    }

    #[test]
    fn test_filter_completions_library_filters_after_load() {
        let template_libraries = build_test_filter_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (10, 20),
            djls_semantic::LoadKind::FullLoad {
                libraries: vec!["humanize".into()],
            },
        )]);
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions =
            generate_filter_completions("", Some(&template_libraries), Some(&symbols));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // Builtins present
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"title"));
        assert!(labels.contains(&"default"));
        // Humanize filters present (loaded)
        assert!(labels.contains(&"intcomma"));
        assert!(labels.contains(&"naturaltime"));
        // l10n filter NOT present (not loaded)
        assert!(!labels.contains(&"localize"));
    }

    #[test]
    fn test_filter_completions_library_filters_excluded_when_not_loaded() {
        let template_libraries = build_test_filter_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![]);
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions =
            generate_filter_completions("int", Some(&template_libraries), Some(&symbols));

        // intcomma not loaded → should not appear
        assert!(completions.is_empty());
    }

    #[test]
    fn test_filter_completions_inspector_unavailable_shows_all() {
        let template_libraries = build_test_filter_libraries();

        // No available_symbols → show all installed filters
        let completions = generate_filter_completions("", Some(&template_libraries), None);

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"title"));
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"intcomma"));
        assert!(labels.contains(&"naturaltime"));
        assert!(labels.contains(&"localize"));
    }

    #[test]
    fn test_filter_completions_selective_import_only_imported_symbols() {
        let template_libraries = build_test_filter_libraries();
        let loaded = djls_semantic::LoadedLibraries::new(vec![make_load_statement(
            (10, 40),
            djls_semantic::LoadKind::SelectiveImport {
                symbols: vec!["intcomma".into()],
                library: "humanize".into(),
            },
        )]);
        let symbols = build_available_symbols(&template_libraries, &loaded, 100);

        let completions =
            generate_filter_completions("", Some(&template_libraries), Some(&symbols));

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        // intcomma selectively imported → present
        assert!(labels.contains(&"intcomma"));
        // naturaltime NOT imported → absent
        assert!(!labels.contains(&"naturaltime"));
        // builtins always present
        assert!(labels.contains(&"lower"));
    }

    #[test]
    fn test_filter_completions_alphabetical_order() {
        let template_libraries = build_test_filter_libraries();

        let completions = generate_filter_completions("", Some(&template_libraries), None);

        let labels: Vec<&str> = completions.iter().map(|c| c.label.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(
            labels, sorted,
            "Filter completions should be alphabetically sorted"
        );
    }

    #[test]
    fn test_filter_completions_detail_text() {
        let template_libraries = build_test_filter_libraries();

        let completions = generate_filter_completions("", Some(&template_libraries), None);

        let lower_completion = completions.iter().find(|c| c.label == "lower").unwrap();
        assert_eq!(lower_completion.detail.as_deref(), Some("builtin filter"));

        let intcomma_completion = completions.iter().find(|c| c.label == "intcomma").unwrap();
        assert_eq!(
            intcomma_completion.detail.as_deref(),
            Some("{% load humanize %}")
        );
    }

    #[test]
    fn test_filter_completions_documentation() {
        let template_libraries = build_test_filter_libraries();

        let completions = generate_filter_completions("", Some(&template_libraries), None);

        let lower_completion = completions.iter().find(|c| c.label == "lower").unwrap();
        assert_eq!(
            lower_completion.documentation,
            Some(ls_types::Documentation::String(
                "Convert a string to lowercase.".to_string()
            ))
        );

        let title_completion = completions.iter().find(|c| c.label == "title").unwrap();
        assert!(title_completion.documentation.is_none());
    }

    #[test]
    fn test_filter_completions_no_template_tags_returns_empty() {
        let completions = generate_filter_completions("", None, None);
        assert!(completions.is_empty());
    }

    #[test]
    fn test_filter_completions_kind_is_function() {
        let template_libraries = build_test_filter_libraries();

        let completions = generate_filter_completions("lower", Some(&template_libraries), None);

        assert_eq!(completions.len(), 1);
        assert_eq!(
            completions[0].kind,
            Some(ls_types::CompletionItemKind::FUNCTION)
        );
    }

    #[test]
    fn test_tag_context_preferred_over_variable_when_both_present() {
        // When both {% and {{ are present, the closer one wins
        let line = "{{ var }} {% if";
        let cursor_offset = 14; // After "if"

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        // {% is closer to cursor than {{ so it's a tag context
        assert!(matches!(context, TemplateCompletionContext::TagName { .. }));
    }

    #[test]
    fn test_variable_context_preferred_when_closer() {
        let line = "{% if True %} {{ value|";
        let cursor_offset = 23;

        let context = analyze_template_context(line, cursor_offset).expect("Should get context");

        assert_eq!(
            context,
            TemplateCompletionContext::Filter {
                partial: String::new(),
            }
        );
    }

    fn build_test_tag_specs_with_args() -> TagSpecs {
        use std::borrow::Cow;

        let mut specs = TagSpecs::default();

        specs.insert(
            "autoescape".to_string(),
            djls_semantic::TagSpec {
                module: Cow::Borrowed("django.template.defaulttags"),
                end_tag: Some(djls_semantic::EndTag {
                    name: Cow::Borrowed("endautoescape"),
                    required: true,
                }),
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: None,
            }
            .with_completion_args(vec![CompletionArg {
                name: "setting".to_string(),
                required: true,
                kind: CompletionArgKind::Choice(vec!["on".to_string(), "off".to_string()]),
                position: 0,
            }]),
        );

        specs.insert(
            "cycle".to_string(),
            djls_semantic::TagSpec {
                module: Cow::Borrowed("django.template.defaulttags"),
                end_tag: None,
                intermediate_tags: Cow::Borrowed(&[]),
                opaque: false,
                extracted_rules: None,
            }
            .with_completion_args(vec![
                CompletionArg {
                    name: "value1".to_string(),
                    required: true,
                    kind: CompletionArgKind::Variable,
                    position: 0,
                },
                CompletionArg {
                    name: "as".to_string(),
                    required: false,
                    kind: CompletionArgKind::Literal("as".to_string()),
                    position: 1,
                },
            ]),
        );

        specs
    }

    #[test]
    fn test_library_completions_partial_close_includes_closing_brace() {
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions =
            generate_library_completions("i18n", &ClosingBrace::PartialClose, Some(&libs));

        assert_eq!(completions.len(), 1);
        let insert = completions[0].insert_text.as_deref().unwrap();
        assert!(
            insert.ends_with(" %}"),
            "PartialClose should append ' %}}' but got: {insert:?}"
        );
    }

    #[test]
    fn test_library_completions_none_close_includes_closing_brace() {
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions = generate_library_completions("i18n", &ClosingBrace::None, Some(&libs));

        assert_eq!(completions.len(), 1);
        let insert = completions[0].insert_text.as_deref().unwrap();
        assert!(
            insert.ends_with(" %}"),
            "None should append ' %}}' but got: {insert:?}"
        );
    }

    #[test]
    fn test_library_completions_full_close_no_closing_appended() {
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let libs = build_template_libraries(&libraries, &[]);

        let completions =
            generate_library_completions("i18n", &ClosingBrace::FullClose, Some(&libs));

        assert_eq!(completions.len(), 1);
        let insert = completions[0].insert_text.as_deref().unwrap();
        assert_eq!(insert, "i18n", "FullClose should not append closing");
    }

    #[test]
    fn test_argument_completions_choice_partial_close_includes_closing_brace() {
        let specs = build_test_tag_specs_with_args();

        let completions = generate_argument_completions(
            "autoescape",
            0,
            "o",
            &ClosingBrace::PartialClose,
            Some(&specs),
            false,
        );

        assert!(
            !completions.is_empty(),
            "Should have completions for 'o' prefix"
        );
        for c in &completions {
            let insert = c.insert_text.as_deref().unwrap();
            assert!(
                insert.ends_with(" %}"),
                "Choice completion with PartialClose should end with ' %}}' but got: {insert:?}"
            );
        }
    }

    #[test]
    fn test_argument_completions_literal_partial_close_includes_closing_brace() {
        let specs = build_test_tag_specs_with_args();

        let completions = generate_argument_completions(
            "cycle",
            1,
            "as",
            &ClosingBrace::PartialClose,
            Some(&specs),
            false,
        );

        assert!(
            !completions.is_empty(),
            "Should have completions for 'as' literal"
        );
        for c in &completions {
            let insert = c.insert_text.as_deref().unwrap();
            assert!(
                insert.ends_with(" %}"),
                "Literal completion with PartialClose should end with ' %}}' but got: {insert:?}"
            );
        }
    }

    #[test]
    fn test_argument_completions_snippet_partial_close_includes_closing_brace() {
        let specs = build_test_tag_specs_with_args();

        let completions = generate_argument_completions(
            "autoescape",
            0,
            "",
            &ClosingBrace::PartialClose,
            Some(&specs),
            true,
        );

        let snippet = completions
            .iter()
            .find(|c| c.kind == Some(ls_types::CompletionItemKind::SNIPPET));
        assert!(snippet.is_some(), "Should have a snippet completion");
        let insert = snippet.unwrap().insert_text.as_deref().unwrap();
        assert!(
            insert.ends_with(" %}"),
            "Snippet completion with PartialClose should end with ' %}}' but got: {insert:?}"
        );
    }
}
