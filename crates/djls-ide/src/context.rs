use djls_source::File;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::parse_template;
use djls_templates::Node;

pub struct OffsetContext {
    pub file: File,
    pub offset: Offset,
    pub span: Span,
    pub kind: ContextKind,
}

pub enum ContextKind {
    TemplateReference(String),
    None,
}

impl OffsetContext {
    pub fn from_offset(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self {
                file,
                offset,
                span: Span::new(offset.get(), 0),
                kind: ContextKind::None,
            };
        };

        for node in nodelist.nodelist(db) {
            if !node.full_span().contains(offset) {
                continue;
            }

            let span = node.full_span();
            let context = match node {
                Node::Tag { name, bits, .. } if Self::is_loader_tag(name) => {
                    Self::extract_template_name(bits)
                        .map_or(ContextKind::None, ContextKind::TemplateReference)
                }
                _ => ContextKind::None,
            };

            return Self {
                file,
                offset,
                span,
                kind: context,
            };
        }

        Self {
            file,
            offset,
            span: Span::new(offset.get(), 0),
            kind: ContextKind::None,
        }
    }

    fn is_loader_tag(tag_name: &str) -> bool {
        matches!(tag_name, "extends" | "include")
    }

    fn extract_template_name(bits: &[String]) -> Option<String> {
        bits.first().map(|s| {
            s.trim()
                .trim_start_matches('"')
                .trim_end_matches('"')
                .trim_start_matches('\'')
                .trim_end_matches('\'')
                .to_string()
        })
    }
}
