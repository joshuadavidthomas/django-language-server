//! Completion logic for Django Language Server
//!
//! This module handles all LSP completion requests, analyzing cursor context
//! and generating appropriate completion items for Django templates.

use std::cmp::Ordering;

use djls_project::TemplateTags;
use djls_workspace::FileKind;
use djls_workspace::PositionEncoding;
use djls_workspace::TextDocument;
use tower_lsp_server::lsp_types::CompletionItem;
use tower_lsp_server::lsp_types::CompletionItemKind;
use tower_lsp_server::lsp_types::Documentation;
use tower_lsp_server::lsp_types::InsertTextFormat;
use tower_lsp_server::lsp_types::Position;

/// The kind of template completion context identified at the cursor
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateCompletionContext {
    /// Completing a tag name after {%
    /// e.g., {% loa| %} where | is cursor
    TagName {
        /// The partial tag name typed so far
        partial: String,
        /// Whether we need to add a leading space
        needs_space: bool,
        /// What closing is already present
        closing: ClosingBrace,
    },
    /// Completing arguments within a tag
    /// e.g., {% load sta| %} where | is cursor
    TagArgument {
        /// The tag being completed (e.g., "load")
        tag: String,
        /// Position in the argument list (0-based)
        position: usize,
        /// The partial argument typed so far
        partial: String,
        /// What closing is already present
        closing: ClosingBrace,
    },
    /// Completing a library name after {% load
    LibraryName {
        /// The partial library name typed so far
        partial: String,
        /// What closing is already present
        closing: ClosingBrace,
    },
    /// No template context found
    None,
}

/// Tracks what closing characters are needed to complete a template tag.
///
/// Used to determine whether the completion system needs to insert
/// closing braces when completing a Django template tag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClosingBrace {
    /// No closing brace present - need to add full `%}` or `}}`
    None,
    /// Partial close present (just `}`) - need to add `%` or second `}`
    PartialClose,
    /// Full close present (`%}` or `}}`) - no closing needed
    FullClose,
}

/// Structured representation of a template completion
#[allow(dead_code)] // Will be used in future enhancements
#[derive(Debug, Clone)]
pub struct TemplateCompletion {
    /// The name of the tag/library/filter
    pub name: String,
    /// The library this comes from
    pub library: String,
    /// Documentation for the completion
    pub documentation: Option<String>,
    /// The kind of completion
    pub kind: TemplateCompletionKind,
    /// Optional snippet pattern (only used if client supports snippets)
    pub snippet_pattern: Option<String>,
}

/// The kind of template completion
#[allow(dead_code)] // Will be used in future enhancements
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TemplateCompletionKind {
    /// A template tag (e.g., for, if, block)
    Tag,
    /// A template library (e.g., staticfiles, i18n)
    Library,
    /// A filter (e.g., date, truncate)
    Filter,
    /// A variable attribute
    Variable,
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
    let context = analyze_template_context(&line_info.text, line_info.cursor_offset);

    // Generate completions based on context
    let mut completions = generate_completions(context, template_tags, supports_snippets);

    // Sort and deduplicate
    completions.sort_by(compare_completions);
    completions.dedup_by(|a, b| a.label == b.label);

    completions
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
fn analyze_template_context(line: &str, cursor_offset: usize) -> TemplateCompletionContext {
    if cursor_offset > line.chars().count() {
        return TemplateCompletionContext::None;
    }

    let chars: Vec<char> = line.chars().collect();
    let prefix = chars[..cursor_offset].iter().collect::<String>();
    let suffix = chars[cursor_offset..].iter().collect::<String>();

    // Check for template tag context {%
    if let Some(tag_start) = prefix.rfind("{%") {
        let closing = detect_closing_brace(&suffix);
        let content_start = tag_start + 2;

        if content_start >= prefix.len() {
            // Cursor right after {%, completing tag name
            return TemplateCompletionContext::TagName {
                partial: String::new(),
                needs_space: true,
                closing,
            };
        }

        let content = &prefix[content_start..];
        let trimmed = content.trim();

        // Parse the tag content to understand context
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.is_empty() {
            // Just whitespace after {%
            return TemplateCompletionContext::TagName {
                partial: String::new(),
                needs_space: false,
                closing,
            };
        }

        let tag_name = parts[0];

        // Check if we're still completing the tag name
        if parts.len() == 1 && !content.ends_with(' ') {
            // {% fo| %} or {% load| %} - still completing tag name
            return TemplateCompletionContext::TagName {
                partial: tag_name.to_string(),
                needs_space: !content.starts_with(' '),
                closing,
            };
        }

        // Special handling for specific tags
        return match tag_name {
            "load" | "import" => {
                if parts.len() == 1 && content.ends_with(' ') {
                    // {% load | %} - expecting library name
                    TemplateCompletionContext::LibraryName {
                        partial: String::new(),
                        closing,
                    }
                } else if parts.len() > 1 {
                    // {% load some_lib par| %} - completing library name
                    let last_part = parts.last().unwrap();
                    TemplateCompletionContext::LibraryName {
                        partial: (*last_part).to_string(),
                        closing,
                    }
                } else {
                    // Fallback to tag name
                    TemplateCompletionContext::TagName {
                        partial: tag_name.to_string(),
                        needs_space: !content.starts_with(' '),
                        closing,
                    }
                }
            }
            _ => {
                if parts.len() == 1 && content.ends_with(' ') {
                    // {% for | %} - ready for arguments
                    TemplateCompletionContext::TagArgument {
                        tag: tag_name.to_string(),
                        position: 0,
                        partial: String::new(),
                        closing,
                    }
                } else if parts.len() > 1 {
                    // Multiple parts, we're in arguments
                    let arg_position = parts.len() - 2; // -1 for tag name, -1 for current partial
                    let partial = (*parts.last().unwrap()).to_string();
                    TemplateCompletionContext::TagArgument {
                        tag: tag_name.to_string(),
                        position: arg_position,
                        partial,
                        closing,
                    }
                } else {
                    // Fallback to tag name
                    TemplateCompletionContext::TagName {
                        partial: tag_name.to_string(),
                        needs_space: !content.starts_with(' '),
                        closing,
                    }
                }
            }
        };
    }

    // TODO: Add support for {{ variable/filter context

    TemplateCompletionContext::None
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

/// Get snippet pattern for a Django template tag
fn get_tag_snippet_pattern(tag_name: &str) -> Option<&'static str> {
    match tag_name {
        // Control flow tags
        "for" => Some("for ${1:item} in ${2:items}"),
        "if" => Some("if ${1:condition}"),
        "elif" => Some("elif ${1:condition}"),
        "ifchanged" => Some("ifchanged ${1:variable}"),
        "ifequal" => Some("ifequal ${1:var1} ${2:var2}"),
        "ifnotequal" => Some("ifnotequal ${1:var1} ${2:var2}"),

        // Block tags
        "block" => Some("block ${1:name}"),
        "extends" => Some("extends \"${1:template.html}\""),
        "include" => Some("include \"${1:template.html}\""),

        // Variable tags
        "with" => Some("with ${1:var}=${2:value}"),
        "cycle" => Some("cycle ${1:val1} ${2:val2}"),

        // Template loading
        "load" => Some("load ${1:library}"),

        // URL tags
        "url" => Some("url '${1:view_name}'"),
        "static" => Some("static '${1:path}'"),

        // Comment tags
        "comment" => Some("comment \"${1:optional_note}\""),

        // Internationalization
        "trans" => Some("trans \"${1:message}\""),
        "blocktrans" => Some("blocktrans"),
        "blocktranslate" => Some("blocktranslate"),
        "get_current_language" => Some("get_current_language as ${1:language}"),

        // Other common tags
        "autoescape" => Some("autoescape ${1:on|off}"),
        "filter" => Some("filter ${1:filter_expr}"),
        "firstof" => Some("firstof ${1:var1} ${2:var2}"),
        "regroup" => Some("regroup ${1:list} by ${2:attribute} as ${3:grouped}"),
        "templatetag" => Some("templatetag ${1:openblock|closeblock|openvariable|closevariable|openbrace|closebrace|opencomment|closecomment}"),
        "widthratio" => Some("widthratio ${1:this_value} ${2:max_value} ${3:max_width}"),

        // Cache tags
        "cache" => Some("cache ${1:timeout} ${2:cache_key}"),

        // Default - tags with no arguments or unknown tags get no snippet
        _ => None,
    }
}

/// Generate completions based on the analyzed context
fn generate_completions(
    context: TemplateCompletionContext,
    template_tags: Option<&TemplateTags>,
    supports_snippets: bool,
) -> Vec<CompletionItem> {
    match context {
        TemplateCompletionContext::TagName {
            partial,
            needs_space,
            closing,
        } => generate_tag_completions(
            &partial,
            needs_space,
            closing,
            template_tags,
            supports_snippets,
        ),
        TemplateCompletionContext::LibraryName { partial, closing } => {
            generate_library_completions(&partial, closing, template_tags, supports_snippets)
        }
        TemplateCompletionContext::TagArgument {
            tag,
            position,
            partial,
            closing,
        } => generate_argument_completions(
            &tag,
            position,
            &partial,
            closing,
            template_tags,
            supports_snippets,
        ),
        TemplateCompletionContext::None => Vec::new(),
    }
}

/// Generate tag name completions
fn generate_tag_completions(
    partial: &str,
    needs_space: bool,
    closing: ClosingBrace,
    template_tags: Option<&TemplateTags>,
    supports_snippets: bool,
) -> Vec<CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    let mut completions = Vec::new();

    for tag in tags.iter() {
        if !tag.name().starts_with(partial) {
            continue;
        }

        let (insert_text, insert_format) = if supports_snippets {
            // Try to get snippet pattern
            if let Some(pattern) = get_tag_snippet_pattern(tag.name()) {
                let mut text = String::new();
                if needs_space {
                    text.push(' ');
                }
                text.push_str(pattern);

                // Add closing based on what's already present
                match closing {
                    ClosingBrace::None => text.push_str(" %}"),
                    ClosingBrace::PartialClose => text.push_str(" %"),
                    ClosingBrace::FullClose => {}
                }

                (text, InsertTextFormat::SNIPPET)
            } else {
                // No snippet pattern, use plain text
                build_plain_insert_text(tag.name(), needs_space, closing)
            }
        } else {
            // Client doesn't support snippets
            build_plain_insert_text(tag.name(), needs_space, closing)
        };

        let completion = CompletionItem {
            label: tag.name().clone(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(format!("from {}", tag.library())),
            documentation: tag.doc().map(|doc| Documentation::String(doc.clone())),
            insert_text: Some(insert_text),
            insert_text_format: Some(insert_format),
            filter_text: Some(tag.name().clone()),
            ..Default::default()
        };

        completions.push(completion);
    }

    completions
}

/// Build plain insert text without snippets
fn build_plain_insert_text(
    tag_name: &str,
    needs_space: bool,
    closing: ClosingBrace,
) -> (String, InsertTextFormat) {
    let mut text = String::new();

    if needs_space {
        text.push(' ');
    }

    text.push_str(tag_name);

    // Add default arguments for common tags (fallback when no snippets)
    match tag_name {
        "for" => text.push_str(" item in items"),
        "if" => text.push_str(" condition"),
        "block" => text.push_str(" name"),
        "load" => text.push_str(" library"),
        "extends" | "include" => text.push_str(" \"template.html\""),
        "with" => text.push_str(" var=value"),
        _ => {}
    }

    // Add closing
    match closing {
        ClosingBrace::None => text.push_str(" %}"),
        ClosingBrace::PartialClose => text.push_str(" %"),
        ClosingBrace::FullClose => {}
    }

    (text, InsertTextFormat::PLAIN_TEXT)
}

/// Generate library name completions (for {% load %} tags)
fn generate_library_completions(
    partial: &str,
    closing: ClosingBrace,
    template_tags: Option<&TemplateTags>,
    _supports_snippets: bool,
) -> Vec<CompletionItem> {
    let Some(tags) = template_tags else {
        return Vec::new();
    };

    // Collect unique library names
    let mut libraries: Vec<String> = tags.iter().map(|tag| tag.library().clone()).collect();

    libraries.sort();
    libraries.dedup();

    let mut completions = Vec::new();

    for library in libraries {
        if !library.starts_with(partial) || library == "builtins" {
            continue;
        }

        let mut insert_text = library.clone();

        // Add closing
        match closing {
            ClosingBrace::None => insert_text.push_str(" %}"),
            ClosingBrace::PartialClose => insert_text.push_str(" %"),
            ClosingBrace::FullClose => {}
        }

        let completion = CompletionItem {
            label: library.clone(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("Django template library".to_string()),
            insert_text: Some(insert_text),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            filter_text: Some(library),
            ..Default::default()
        };

        completions.push(completion);
    }

    completions
}

/// Generate argument completions for specific tags
fn generate_argument_completions(
    _tag: &str,
    _position: usize,
    _partial: &str,
    _closing: ClosingBrace,
    _template_tags: Option<&TemplateTags>,
    _supports_snippets: bool,
) -> Vec<CompletionItem> {
    // TODO: Implement context-aware argument completions
    // For now, return empty as this is an enhancement
    Vec::new()
}

/// Compare completions for sorting
fn compare_completions(a: &CompletionItem, b: &CompletionItem) -> Ordering {
    // Priority order:
    // 1. Common tags (for, if, block, include, extends, load)
    // 2. Other built-in tags
    // 3. Custom tags
    // 4. Alphabetical within each group

    let common_tags = [
        "for", "if", "elif", "else", "block", "include", "extends", "load", "with", "static", "url",
    ];

    let a_common = common_tags.contains(&a.label.as_str());
    let b_common = common_tags.contains(&b.label.as_str());

    match (a_common, b_common) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.label.cmp(&b.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_template_context_tag_name() {
        let line = "{% loa";
        let cursor_offset = 6; // After "loa"

        let context = analyze_template_context(line, cursor_offset);

        match context {
            TemplateCompletionContext::TagName {
                partial,
                needs_space,
                closing,
            } => {
                assert_eq!(partial, "loa");
                assert!(!needs_space);
                assert_eq!(closing, ClosingBrace::None);
            }
            _ => panic!("Expected TagName context"),
        }
    }

    #[test]
    fn test_analyze_template_context_needs_leading_space() {
        let line = "{%";
        let cursor_offset = 2; // Right after {%

        let context = analyze_template_context(line, cursor_offset);

        match context {
            TemplateCompletionContext::TagName {
                partial,
                needs_space,
                closing,
            } => {
                assert_eq!(partial, "");
                assert!(needs_space);
                assert_eq!(closing, ClosingBrace::None);
            }
            _ => panic!("Expected TagName context"),
        }
    }

    #[test]
    fn test_analyze_template_context_library_name() {
        let line = "{% load sta";
        let cursor_offset = 11; // After "sta"

        let context = analyze_template_context(line, cursor_offset);

        match context {
            TemplateCompletionContext::LibraryName { partial, closing } => {
                assert_eq!(partial, "sta");
                assert_eq!(closing, ClosingBrace::None);
            }
            _ => panic!("Expected LibraryName context"),
        }
    }

    #[test]
    fn test_analyze_template_context_with_closing() {
        let line = "{% load %}";
        let cursor_offset = 7; // After "load"

        let context = analyze_template_context(line, cursor_offset);

        match context {
            TemplateCompletionContext::TagName {
                partial, closing, ..
            } => {
                assert_eq!(partial, "load");
                assert_eq!(closing, ClosingBrace::FullClose);
            }
            _ => panic!("Expected TagName context"),
        }
    }

    #[test]
    fn test_analyze_template_context_partial_closing() {
        let line = "{% load }";
        let cursor_offset = 7; // After "load"

        let context = analyze_template_context(line, cursor_offset);

        match context {
            TemplateCompletionContext::TagName {
                partial, closing, ..
            } => {
                assert_eq!(partial, "load");
                assert_eq!(closing, ClosingBrace::PartialClose);
            }
            _ => panic!("Expected TagName context"),
        }
    }

    #[test]
    fn test_analyze_template_context_no_template() {
        let line = "Just regular HTML";
        let cursor_offset = 5;

        let context = analyze_template_context(line, cursor_offset);

        assert_eq!(context, TemplateCompletionContext::None);
    }

    #[test]
    fn test_snippet_patterns() {
        assert_eq!(
            get_tag_snippet_pattern("for"),
            Some("for ${1:item} in ${2:items}")
        );
        assert_eq!(get_tag_snippet_pattern("if"), Some("if ${1:condition}"));
        assert_eq!(get_tag_snippet_pattern("block"), Some("block ${1:name}"));
        assert_eq!(get_tag_snippet_pattern("csrf_token"), None);
    }

    #[test]
    fn test_snippet_vs_plain_completion() {
        use djls_project::TemplateTag;

        // Create a mock template tag
        let tags = TemplateTags::from(vec![
            TemplateTag::new(
                "for".to_string(),
                "builtins".to_string(),
                Some("For loop tag".to_string()),
            ),
            TemplateTag::new("csrf_token".to_string(), "builtins".to_string(), None),
        ]);

        // Test with snippet support
        let completions = generate_tag_completions(
            "fo",
            false,
            ClosingBrace::None,
            Some(&tags),
            true, // supports_snippets
        );

        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].label, "for");
        assert_eq!(
            completions[0].insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        assert!(completions[0]
            .insert_text
            .as_ref()
            .unwrap()
            .contains("${1:item}"));

        // Test without snippet support
        let completions =
            generate_tag_completions("fo", false, ClosingBrace::None, Some(&tags), false);

        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].label, "for");
        assert_eq!(
            completions[0].insert_text_format,
            Some(InsertTextFormat::PLAIN_TEXT)
        );
        assert!(completions[0]
            .insert_text
            .as_ref()
            .unwrap()
            .contains("for item in items"));
        assert!(!completions[0].insert_text.as_ref().unwrap().contains("${"));
    }

    #[test]
    fn test_library_completions() {
        use djls_project::TemplateTag;

        let tags = TemplateTags::from(vec![
            TemplateTag::new("static".to_string(), "staticfiles".to_string(), None),
            TemplateTag::new("url".to_string(), "builtins".to_string(), None),
            TemplateTag::new("trans".to_string(), "i18n".to_string(), None),
        ]);

        let completions =
            generate_library_completions("sta", ClosingBrace::None, Some(&tags), false);

        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].label, "staticfiles");
        assert_eq!(completions[0].kind, Some(CompletionItemKind::MODULE));
    }
}
