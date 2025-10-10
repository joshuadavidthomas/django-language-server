use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;

#[allow(dead_code)]
pub(crate) enum OffsetContext {
    TemplateReference(String),
    BlockDefinition {
        name: String,
        span: Span,
    },
    BlockReference {
        name: String,
        span: Span,
    },
    Tag {
        name: String,
        args: Vec<String>,
        span: Span,
    },
    Variable {
        name: String,
        filters: Vec<String>,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Text {
        span: Span,
    },
    None,
}

impl OffsetContext {
    pub(crate) fn from_offset(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        let node = nodelist
            .nodelist(db)
            .iter()
            .find(|node| node.full_span().contains(offset));

        match node {
            Some(Node::Tag { name, bits, span }) => Self::from_tag(name, bits, *span),
            Some(Node::Variable { var, filters, span }) => Self::Variable {
                name: var.clone(),
                filters: filters.clone(),
                span: *span,
            },
            Some(Node::Comment { content, span }) => Self::Comment {
                content: content.clone(),
                span: *span,
            },
            Some(Node::Text { span }) => Self::Text { span: *span },
            Some(Node::Error { .. }) | None => Self::None,
        }
    }

    fn from_tag(name: &str, bits: &[String], span: Span) -> Self {
        match name {
            "extends" | "include" => bits
                .first()
                .map_or(Self::None, |s| Self::parse_template_reference(s)),

            "block" => {
                let block_name = bits.first().cloned().unwrap_or_default();
                Self::BlockDefinition {
                    name: block_name,
                    span,
                }
            }

            "endblock" => {
                let block_name = bits.first().cloned().unwrap_or_default();
                Self::BlockReference {
                    name: block_name,
                    span,
                }
            }

            _ => Self::Tag {
                name: name.to_string(),
                args: bits.to_vec(),
                span,
            },
        }
    }

    fn parse_template_reference(raw: &str) -> Self {
        let trimmed = raw.trim();
        let unquoted = trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| {
                trimmed
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
            })
            .unwrap_or(trimmed);
        Self::TemplateReference(unquoted.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offset_context_variants_exist() {
        let contexts = vec![
            OffsetContext::TemplateReference("test.html".to_string()),
            OffsetContext::BlockDefinition {
                name: "content".to_string(),
                span: Span::new(0, 10),
            },
            OffsetContext::BlockReference {
                name: "content".to_string(),
                span: Span::new(0, 10),
            },
            OffsetContext::Tag {
                name: "if".to_string(),
                args: vec!["user.is_authenticated".to_string()],
                span: Span::new(0, 10),
            },
            OffsetContext::Variable {
                name: "user".to_string(),
                filters: vec!["title".to_string()],
                span: Span::new(0, 10),
            },
            OffsetContext::Comment {
                content: "TODO".to_string(),
                span: Span::new(0, 10),
            },
            OffsetContext::Text {
                span: Span::new(0, 10),
            },
            OffsetContext::None,
        ];
        assert_eq!(contexts.len(), 8);
    }

    #[test]
    fn test_parse_template_reference_strips_double_quotes() {
        let result = OffsetContext::parse_template_reference("\"base.html\"");
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "base.html"
        ));
    }

    #[test]
    fn test_parse_template_reference_strips_single_quotes() {
        let result = OffsetContext::parse_template_reference("'base.html'");
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "base.html"
        ));
    }

    #[test]
    fn test_parse_template_reference_strips_quotes_and_whitespace() {
        let result = OffsetContext::parse_template_reference("  \"base.html\"  ");
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "base.html"
        ));
    }

    #[test]
    fn test_parse_template_reference_handles_unquoted() {
        let result = OffsetContext::parse_template_reference("base.html");
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "base.html"
        ));
    }

    #[test]
    fn test_from_tag_handles_extends() {
        let result =
            OffsetContext::from_tag("extends", &["\"base.html\"".to_string()], Span::new(0, 10));
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "base.html"
        ));
    }

    #[test]
    fn test_from_tag_handles_include() {
        let result = OffsetContext::from_tag(
            "include",
            &["\"partial.html\"".to_string()],
            Span::new(0, 10),
        );
        assert!(matches!(
            result,
            OffsetContext::TemplateReference(s) if s == "partial.html"
        ));
    }

    #[test]
    fn test_from_tag_handles_block() {
        let result = OffsetContext::from_tag("block", &["content".to_string()], Span::new(0, 10));
        assert!(matches!(
            result,
            OffsetContext::BlockDefinition { name, .. } if name == "content"
        ));
    }

    #[test]
    fn test_from_tag_handles_endblock() {
        let result =
            OffsetContext::from_tag("endblock", &["content".to_string()], Span::new(0, 10));
        assert!(matches!(
            result,
            OffsetContext::BlockReference { name, .. } if name == "content"
        ));
    }

    #[test]
    fn test_from_tag_handles_generic_tag() {
        let result = OffsetContext::from_tag(
            "if",
            &["user.is_authenticated".to_string()],
            Span::new(0, 10),
        );
        assert!(matches!(
            result,
            OffsetContext::Tag { name, args, .. } if name == "if" && args == vec!["user.is_authenticated"]
        ));
    }

    #[test]
    fn test_from_tag_handles_empty_block_name() {
        let result = OffsetContext::from_tag("block", &[], Span::new(0, 10));
        assert!(matches!(
            result,
            OffsetContext::BlockDefinition { name, .. } if name.is_empty()
        ));
    }
}
