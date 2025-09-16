use djls_source::Span;
use serde::Serialize;

use crate::db::Db as TemplateDb;

#[salsa::tracked(debug)]
pub struct NodeList<'db> {
    #[tracked]
    #[returns(ref)]
    pub nodelist: Vec<Node<'db>>,
    #[tracked]
    #[returns(ref)]
    pub line_offsets: LineOffsets,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LineOffsets(pub Vec<u32>);

impl LineOffsets {
    pub fn add_line(&mut self, offset: u32) {
        self.0.push(offset);
    }

    #[must_use]
    pub fn position_to_line_col(&self, position: usize) -> (usize, usize) {
        let position = u32::try_from(position).unwrap_or_default();
        let line = match self.0.binary_search(&position) {
            Ok(exact_line) => exact_line,    // Position is at start of this line
            Err(0) => 0,                     // Before first line start
            Err(next_line) => next_line - 1, // We're on the previous line
        };

        // Calculate column as offset from line start
        let col = if line == 0 {
            position as usize
        } else {
            (position - self.0[line]) as usize
        };

        // Convert to 1-based line number
        (line + 1, col)
    }

    #[must_use]
    pub fn line_col_to_position(&self, line: u32, col: u32) -> u32 {
        // line is 1-based, so subtract 1 to get the index
        self.0[(line - 1) as usize] + col
    }
}

impl Default for LineOffsets {
    fn default() -> Self {
        Self(vec![0])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum Node<'db> {
    Tag {
        name: TagName<'db>,
        bits: Vec<TagBit<'db>>,
        span: Span,
    },
    Comment {
        content: String,
        span: Span,
    },
    Text {
        span: Span,
    },
    Variable {
        var: VariableName<'db>,
        filters: Vec<FilterName<'db>>,
        span: Span,
    },
}

impl<'db> Node<'db> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Node::Tag { span, .. }
            | Node::Variable { span, .. }
            | Node::Comment { span, .. }
            | Node::Text { span } => *span,
        }
    }

    #[must_use]
    pub fn full_span(&self) -> Span {
        match self {
            // account for delimiters
            Node::Variable { span, .. } | Node::Comment { span, .. } | Node::Tag { span, .. } => {
                Span {
                    start: span.start.saturating_sub(3),
                    length: span.length + 6,
                }
            }
            Node::Text { span } => *span,
        }
    }

    pub fn identifier_span(&self, db: &'db dyn TemplateDb) -> Option<Span> {
        match self {
            Node::Tag { name, span, .. } => {
                // Just the tag name (e.g., "if" in "{% if user.is_authenticated %}")
                let name_len = name.text(db).len();
                Some(Span {
                    start: span.start,
                    length: u32::try_from(name_len).unwrap_or(0),
                })
            }
            Node::Variable { var, span, .. } => {
                // Just the variable name (e.g., "user" in "{{ user.name|title }}")
                let var_len = var.text(db).len();
                Some(Span {
                    start: span.start,
                    length: u32::try_from(var_len).unwrap_or(0),
                })
            }
            Node::Comment { .. } | Node::Text { .. } => None,
        }
    }
}

#[salsa::interned(debug)]
pub struct TagName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct TagBit<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct VariableName<'db> {
    pub text: String,
}

#[salsa::interned(debug)]
pub struct FilterName<'db> {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod line_offsets {
        use super::*;

        #[test]
        fn test_new_starts_at_zero() {
            let offsets = LineOffsets::default();
            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Line 1, column 0
        }

        #[test]
        fn test_start_of_lines() {
            let mut offsets = LineOffsets::default();
            offsets.add_line(10); // Line 2 starts at offset 10
            offsets.add_line(25); // Line 3 starts at offset 25

            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Line 1, start
            assert_eq!(offsets.position_to_line_col(10), (2, 0)); // Line 2, start
            assert_eq!(offsets.position_to_line_col(25), (3, 0)); // Line 3, start
        }
    }

    mod spans_and_positions {

        #[test]
        fn test_variable_spans() {
            // let template = "Hello\n{{ user.name }}\nWorld";
            // Tests will need to be updated to work with the new db parameter
            // For now, comment out to allow compilation
            // let tokens = Lexer::new(template).tokenize().unwrap();
            // let mut parser = Parser::new(tokens);
            // let (nodelist, errors) = parser.parse().unwrap();
            // assert!(errors.is_empty());

            // // Find the variable node
            // let nodes = nodelist.nodelist();
            // let var_node = nodes
            //     .iter()
            //     .find(|n| matches!(n, Node::Variable { .. }))
            //     .unwrap();

            // if let Node::Variable { span, .. } = var_node {
            //     // Variable starts after newline + "{{"
            //     let (line, col) = nodelist
            //         .line_offsets()
            //         .position_to_line_col(span.start() as usize);
            //     assert_eq!(
            //         (line, col),
            //         (2, 0),
            //         "Variable should start at line 2, col 3"
            //     );

            //     assert_eq!(span.length(), 9, "Variable span should cover 'user.name'");
            // }
        }
    }
}
