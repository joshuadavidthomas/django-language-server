use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use salsa::Accumulator;

use crate::blocks::CloseValidation;
use crate::blocks::TagClass;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub fn validate_structure<'db>(db: &'db dyn Db, nodelist: djls_templates::NodeList<'db>) {
    let index = db.tag_index();
    let mut validator = StructureValidator {
        db,
        nodelist,
        index,
        stack: Vec::new(),
    };
    validator.validate();
}

struct OpenerTagFrame {
    name: String,
    bits: Vec<String>,
    span: Span,
}

struct StructureValidator<'db> {
    db: &'db dyn Db,
    nodelist: djls_templates::NodeList<'db>,
    index: crate::blocks::TagIndex<'db>,
    stack: Vec<OpenerTagFrame>,
}

impl StructureValidator<'_> {
    fn validate(&mut self) {
        for node in self.nodelist.nodelist(self.db).iter().cloned() {
            if let Node::Tag { name, bits, span } = node {
                let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
                match self.index.classify(self.db, &name) {
                    TagClass::Opener => {
                        self.stack.push(OpenerTagFrame {
                            name,
                            bits,
                            span: full_span,
                        });
                    }
                    TagClass::Intermediate { possible_openers } => {
                        self.handle_intermediate(&name, full_span, &possible_openers);
                    }
                    TagClass::Closer { opener_name } => {
                        self.handle_closer(&opener_name, &bits, full_span);
                    }
                    TagClass::Unknown => {}
                }
            }
        }

        while let Some(frame) = self.stack.pop() {
            ValidationErrorAccumulator(ValidationError::UnclosedTag {
                tag: frame.name,
                span: frame.span,
            })
            .accumulate(self.db);
        }
    }

    fn handle_intermediate(&mut self, name: &str, span: Span, possible_openers: &[String]) {
        if let Some(frame) = self.stack.last() {
            if possible_openers.contains(&frame.name) {
                // Intermediate is valid; keep stack unchanged
                return;
            }
        }

        let context = Self::format_intermediate_context(possible_openers);
        ValidationErrorAccumulator(ValidationError::OrphanedTag {
            tag: name.to_string(),
            context,
            span,
        })
        .accumulate(self.db);
    }

    fn handle_closer(&mut self, opener_canonical: &str, closer_bits: &[String], span: Span) {
        let Some(frame_idx) = &self
            .stack
            .iter()
            .rposition(|frame| frame.name == opener_canonical)
        else {
            ValidationErrorAccumulator(ValidationError::UnbalancedStructure {
                opening_tag: opener_canonical.to_string(),
                expected_closing: String::new(),
                opening_span: span,
                closing_span: None,
            })
            .accumulate(self.db);
            return;
        };

        while self.stack.len() > frame_idx + 1 {
            if let Some(unclosed) = self.stack.pop() {
                ValidationErrorAccumulator(ValidationError::UnclosedTag {
                    tag: unclosed.name,
                    span: unclosed.span,
                })
                .accumulate(self.db);
            }
        }

        let frame = self.stack.pop().unwrap();
        match self
            .index
            .validate_close(self.db, opener_canonical, &frame.bits, closer_bits)
        {
            CloseValidation::Valid => {}
            CloseValidation::ArgumentMismatch {
                arg: _,
                expected,
                got,
            } => {
                let name = if got.is_empty() { expected } else { got };
                ValidationErrorAccumulator(ValidationError::UnmatchedBlockName { name, span })
                    .accumulate(self.db);
                ValidationErrorAccumulator(ValidationError::UnclosedTag {
                    tag: frame.name.clone(),
                    span: frame.span,
                })
                .accumulate(self.db);
                self.stack.push(frame);
            }
            CloseValidation::MissingRequiredArg { arg: _, expected } => {
                let expected_closing = format!("{} {}", frame.name, expected);
                ValidationErrorAccumulator(ValidationError::UnbalancedStructure {
                    opening_tag: frame.name.clone(),
                    expected_closing,
                    opening_span: frame.span,
                    closing_span: Some(span),
                })
                .accumulate(self.db);
                self.stack.push(frame);
            }
            CloseValidation::UnexpectedArg { arg, got } => {
                let name = if got.is_empty() { arg } else { got };
                ValidationErrorAccumulator(ValidationError::UnmatchedBlockName { name, span })
                    .accumulate(self.db);
                ValidationErrorAccumulator(ValidationError::UnclosedTag {
                    tag: frame.name.clone(),
                    span: frame.span,
                })
                .accumulate(self.db);
                self.stack.push(frame);
            }
            CloseValidation::NotABlock => {
                ValidationErrorAccumulator(ValidationError::UnbalancedStructure {
                    opening_tag: opener_canonical.to_string(),
                    expected_closing: opener_canonical.to_string(),
                    opening_span: frame.span,
                    closing_span: Some(span),
                })
                .accumulate(self.db);
                self.stack.push(frame);
            }
        }
    }

    fn format_intermediate_context(possible_openers: &[String]) -> String {
        match possible_openers.len() {
            0 => "a valid parent block".to_string(),
            1 => format!("'{}' block", possible_openers[0]),
            2 => format!(
                "'{}' or '{}' block",
                possible_openers[0], possible_openers[1]
            ),
            _ => {
                let mut parts = possible_openers
                    .iter()
                    .map(|name| format!("'{name}'"))
                    .collect::<Vec<_>>();
                let last = parts.pop().unwrap_or_default();
                let prefix = parts.join(", ");
                format!("one of {prefix}, or {last} blocks")
            }
        }
    }
}
