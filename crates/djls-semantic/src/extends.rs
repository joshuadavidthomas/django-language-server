use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub fn validate_extends(db: &dyn Db, nodelist: NodeList<'_>) {
    let mut contains_nontext = false;
    let mut seen_extends = false;

    for node in nodelist.nodelist(db) {
        match node {
            Node::Text { .. } | Node::Comment { .. } | Node::Error { .. } => {}
            Node::Tag { name, span, .. } if name == "extends" => {
                let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

                if seen_extends {
                    ValidationErrorAccumulator(ValidationError::MultipleExtends {
                        span: marker_span,
                    })
                    .accumulate(db);
                } else {
                    if contains_nontext {
                        ValidationErrorAccumulator(ValidationError::ExtendsMustBeFirst {
                            span: marker_span,
                        })
                        .accumulate(db);
                    }
                    seen_extends = true;
                }
            }
            Node::Tag { .. } | Node::Variable { .. } => {
                contains_nontext = true;
            }
        }
    }
}
