//! Snippet generation for Django template tags
//!
//! This module generates LSP snippet patterns for Django template tags.
//! NOTE: This is currently stubbed out. Full implementation using `ExtractedArg`
//! will be completed in M9 Phase 4.

use djls_semantic::TagSpec;

/// Generate an LSP snippet pattern from a tag spec.
///
/// NOTE: Currently returns empty string. Full implementation using
/// `ExtractedArg` will be completed in M9 Phase 4.
#[must_use]
pub fn generate_snippet_from_spec(_spec: &TagSpec) -> String {
    // TODO(M9 Phase 4): Implement using ExtractedArg from extraction
    String::new()
}

/// Generate a complete LSP snippet for a tag including the tag name
#[must_use]
pub fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let args_snippet = generate_snippet_from_spec(spec);

    if args_snippet.is_empty() {
        // Tag with no arguments
        tag_name.to_string()
    } else {
        // Tag with arguments
        format!("{tag_name} {args_snippet}")
    }
}

/// Generate a complete LSP snippet for a tag including the tag name and closing tag if needed
#[must_use]
pub fn generate_snippet_for_tag_with_end(tag_name: &str, spec: &TagSpec) -> String {
    // Special handling for block tag to mirror the name in endblock
    if tag_name == "block" {
        // LSP snippets support placeholder mirroring using the same number
        // ${1:name} in opening tag will be mirrored to ${1} in closing tag
        let snippet = String::from("block ${1:name} %}\n$0\n{% endblock ${1} %}");
        return snippet;
    }

    let mut snippet = generate_snippet_for_tag(tag_name, spec);

    // If this tag has a required end tag, include it in the snippet
    if let Some(end_tag) = &spec.end_tag {
        if end_tag.required {
            // Add closing %} for the opening tag, newline, cursor position, newline, then end tag
            snippet.push_str(" %}\n$0\n{% ");
            snippet.push_str(&end_tag.name);
            snippet.push_str(" %}");
        }
    }

    snippet
}

/// Generate a partial snippet starting from a specific argument position
/// This is useful when the user has already typed some arguments
#[must_use]
pub fn generate_partial_snippet(_spec: &TagSpec, _starting_from_position: usize) -> String {
    // TODO(M9 Phase 4): Implement using ExtractedArg from extraction
    String::new()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use djls_semantic::EndTag;
    use djls_semantic::TagSpec;

    use super::*;

    #[test]
    fn test_snippet_for_block_tag() {
        let spec = TagSpec {
            module: "django.template.loader_tags".into(),
            end_tag: Some(EndTag {
                name: "endblock".into(),
                required: true,
            }),
            intermediate_tags: Cow::Borrowed(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        let snippet = generate_snippet_for_tag_with_end("block", &spec);
        assert_eq!(snippet, "block ${1:name} %}\n$0\n{% endblock ${1} %}");
    }

    #[test]
    fn test_snippet_with_end_tag() {
        let spec = TagSpec {
            module: "django.template.defaulttags".into(),
            end_tag: Some(EndTag {
                name: "endautoescape".into(),
                required: true,
            }),
            intermediate_tags: Cow::Borrowed(&[]),
            opaque: false,
            extracted_rules: Vec::new(),
        };

        let snippet = generate_snippet_for_tag_with_end("autoescape", &spec);
        assert_eq!(
            snippet,
            "autoescape %}\n$0\n{% endautoescape %}"
        );
    }
}
