use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub fn validate_extends(db: &dyn Db, nodelist: NodeList<'_>) {
    let mut contains_nontext = false;
    let mut first_extends_span = None;

    for node in nodelist.nodelist(db) {
        match node {
            Node::Text { .. } | Node::Comment { .. } | Node::Error { .. } => {}
            Node::Tag { name, span, .. } if name == "extends" => {
                if first_extends_span.is_some() {
                    ValidationErrorAccumulator(ValidationError::MultipleExtends { span: *span })
                        .accumulate(db);
                } else {
                    if contains_nontext {
                        ValidationErrorAccumulator(ValidationError::ExtendsMustBeFirst {
                            span: *span,
                        })
                        .accumulate(db);
                    }
                    first_extends_span = Some(*span);
                }
            }
            Node::Tag { .. } | Node::Variable { .. } => {
                contains_nontext = true;
            }
        }
    }
}
