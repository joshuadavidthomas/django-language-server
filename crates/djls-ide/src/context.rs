use djls_source::File;
use djls_source::Offset;
use djls_templates::parse_template;
use djls_templates::Node;

pub(crate) enum OffsetContext {
    TemplateReference(String),
    None,
}

impl OffsetContext {
    pub(crate) fn from_offset(db: &dyn djls_semantic::Db, file: File, offset: Offset) -> Self {
        let Some(nodelist) = parse_template(db, file) else {
            return Self::None;
        };

        for node in nodelist.nodelist(db) {
            if !node.full_span().contains(offset) {
                continue;
            }

            return match node {
                Node::Tag { name, bits, .. } if matches!(name.as_str(), "extends" | "include") => {
                    bits.first()
                        .map(|s| {
                            s.trim()
                                .trim_start_matches('"')
                                .trim_end_matches('"')
                                .trim_start_matches('\'')
                                .trim_end_matches('\'')
                                .to_string()
                        })
                        .map_or(Self::None, Self::TemplateReference)
                }
                _ => Self::None,
            };
        }

        Self::None
    }
}
