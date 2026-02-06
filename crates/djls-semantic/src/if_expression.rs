//! Static syntax validation for `{% if %}` / `{% elif %}` expressions.
//!
//! Mirrors Django's `smartif.py` parser to catch compile-time expression
//! syntax errors (operator/operand placement, dangling operators, unused
//! trailing tokens).
//!
//! It intentionally does *not* attempt to validate the syntax of individual
//! operands (which are parsed by Django's `compile_filter()` at runtime).

#[derive(Debug, Clone)]
enum IfToken {
    Literal(String),
    Operator(Operator),
    End,
}

#[derive(Debug, Clone, Copy)]
struct Operator {
    id: &'static str,
    lbp: u8,
    is_prefix: bool,
}

impl Operator {
    const fn infix(id: &'static str, lbp: u8) -> Self {
        Self {
            id,
            lbp,
            is_prefix: false,
        }
    }

    const fn prefix(id: &'static str, lbp: u8) -> Self {
        Self {
            id,
            lbp,
            is_prefix: true,
        }
    }
}

/// Operator precedence table (matches Django's `smartif.py`)
const OPERATORS: &[(&str, Operator)] = &[
    ("or", Operator::infix("or", 6)),
    ("and", Operator::infix("and", 7)),
    ("not", Operator::prefix("not", 8)),
    ("in", Operator::infix("in", 9)),
    ("not in", Operator::infix("not in", 9)),
    ("is", Operator::infix("is", 10)),
    ("is not", Operator::infix("is not", 10)),
    ("==", Operator::infix("==", 10)),
    ("!=", Operator::infix("!=", 10)),
    (">", Operator::infix(">", 10)),
    (">=", Operator::infix(">=", 10)),
    ("<", Operator::infix("<", 10)),
    ("<=", Operator::infix("<=", 10)),
];

fn lookup_operator(token: &str) -> Option<Operator> {
    OPERATORS
        .iter()
        .find(|(k, _)| *k == token)
        .map(|(_, op)| *op)
}

struct IfExpressionParser {
    tokens: Vec<IfToken>,
    pos: usize,
    current: IfToken,
}

impl IfExpressionParser {
    fn new(bits: &[String]) -> Self {
        let mut mapped = Vec::new();
        let mut i = 0;

        while i < bits.len() {
            let token = &bits[i];

            // Handle two-word operators: "is not" and "not in"
            let (combined, advance) =
                if token == "is" && bits.get(i + 1).is_some_and(|t| t == "not") {
                    ("is not", 1)
                } else if token == "not" && bits.get(i + 1).is_some_and(|t| t == "in") {
                    ("not in", 1)
                } else {
                    (token.as_str(), 0)
                };

            i += advance;

            let if_token = if let Some(op) = lookup_operator(combined) {
                IfToken::Operator(op)
            } else {
                IfToken::Literal(bits[i].clone())
            };

            mapped.push(if_token);
            i += 1;
        }

        let current = mapped.first().cloned().unwrap_or(IfToken::End);
        Self {
            tokens: mapped,
            pos: 0,
            current,
        }
    }

    fn advance(&mut self) {
        self.pos += 1;
        self.current = self
            .tokens
            .get(self.pos)
            .cloned()
            .unwrap_or(IfToken::End);
    }

    fn parse(&mut self) -> Result<(), String> {
        self.expression(0)?;

        if !matches!(self.current, IfToken::End) {
            let display = match &self.current {
                IfToken::Literal(s) => s.clone(),
                IfToken::Operator(op) => op.id.to_string(),
                IfToken::End => unreachable!(),
            };
            return Err(format!(
                "Unused '{display}' at end of if expression."
            ));
        }
        Ok(())
    }

    fn expression(&mut self, rbp: u8) -> Result<(), String> {
        let t = std::mem::replace(&mut self.current, IfToken::End);
        self.advance();

        match &t {
            IfToken::Literal(_) => {}
            IfToken::Operator(op) if op.is_prefix => {
                self.expression(op.lbp)?;
            }
            IfToken::Operator(op) => {
                return Err(format!(
                    "Not expecting '{}' in this position in if tag.",
                    op.id
                ));
            }
            IfToken::End => {
                return Err("Unexpected end of expression in if tag.".to_string());
            }
        }

        loop {
            let lbp = match &self.current {
                IfToken::Operator(op) => op.lbp,
                _ => 0,
            };

            if rbp >= lbp {
                break;
            }

            let t = std::mem::replace(&mut self.current, IfToken::End);
            self.advance();

            if let IfToken::Operator(op) = t {
                if op.is_prefix {
                    return Err(format!(
                        "Not expecting '{}' as infix operator in if tag.",
                        op.id
                    ));
                }
                self.expression(op.lbp)?;
            }
        }
        Ok(())
    }
}

/// Validate expression tokens for `{% if %}` / `{% elif %}`.
///
/// Returns an error message matching Django's style, or `None` if valid.
#[must_use]
pub fn validate_if_expression(bits: &[String]) -> Option<String> {
    if bits.is_empty() {
        return Some("Unexpected end of expression in if tag.".to_string());
    }
    let mut parser = IfExpressionParser::new(bits);
    parser.parse().err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).to_string()).collect()
    }

    // Valid expressions

    #[test]
    fn valid_simple_literal() {
        assert!(validate_if_expression(&bits(&["x"])).is_none());
    }

    #[test]
    fn valid_not_prefix() {
        assert!(validate_if_expression(&bits(&["not", "x"])).is_none());
    }

    #[test]
    fn valid_and() {
        assert!(validate_if_expression(&bits(&["x", "and", "y"])).is_none());
    }

    #[test]
    fn valid_or() {
        assert!(validate_if_expression(&bits(&["x", "or", "y"])).is_none());
    }

    #[test]
    fn valid_equality() {
        assert!(validate_if_expression(&bits(&["x", "==", "y"])).is_none());
    }

    #[test]
    fn valid_inequality() {
        assert!(validate_if_expression(&bits(&["x", "!=", "y"])).is_none());
    }

    #[test]
    fn valid_greater_than() {
        assert!(validate_if_expression(&bits(&["x", ">", "y"])).is_none());
    }

    #[test]
    fn valid_greater_equal() {
        assert!(validate_if_expression(&bits(&["x", ">=", "y"])).is_none());
    }

    #[test]
    fn valid_less_than() {
        assert!(validate_if_expression(&bits(&["x", "<", "y"])).is_none());
    }

    #[test]
    fn valid_less_equal() {
        assert!(validate_if_expression(&bits(&["x", "<=", "y"])).is_none());
    }

    #[test]
    fn valid_and_not() {
        assert!(validate_if_expression(&bits(&["x", "and", "not", "y"])).is_none());
    }

    #[test]
    fn valid_not_in() {
        assert!(validate_if_expression(&bits(&["x", "not", "in", "y"])).is_none());
    }

    #[test]
    fn valid_is() {
        assert!(validate_if_expression(&bits(&["x", "is", "y"])).is_none());
    }

    #[test]
    fn valid_is_not() {
        assert!(validate_if_expression(&bits(&["x", "is", "not", "y"])).is_none());
    }

    #[test]
    fn valid_in() {
        assert!(validate_if_expression(&bits(&["x", "in", "y"])).is_none());
    }

    #[test]
    fn valid_filter_expression() {
        assert!(validate_if_expression(&bits(&["x|length", ">=", "5"])).is_none());
    }

    #[test]
    fn valid_complex_chain() {
        assert!(
            validate_if_expression(&bits(&["x", "and", "y", "or", "not", "z"])).is_none()
        );
    }

    #[test]
    fn valid_comparison_chain() {
        assert!(
            validate_if_expression(&bits(&[
                "x", "==", "y", "and", "a", "!=", "b", "or", "c", "in", "d"
            ]))
            .is_none()
        );
    }

    // Invalid expressions

    #[test]
    fn invalid_empty() {
        assert_eq!(
            validate_if_expression(&[]),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_operator_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["and", "x"])),
            Some("Not expecting 'and' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_missing_rhs_eq() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "=="])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_missing_rhs_in() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "in"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_not_alone() {
        assert_eq!(
            validate_if_expression(&bits(&["not"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_trailing_token() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "y"])),
            Some("Unused 'y' at end of if expression.".to_string())
        );
    }

    #[test]
    fn invalid_consecutive_infix_operators() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "and", "or", "y"])),
            Some("Not expecting 'or' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_not_in_trailing() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "not", "in"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_is_not_trailing() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "is", "not"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_in_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["in", "x"])),
            Some("Not expecting 'in' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_not_as_infix() {
        // "not" is prefix-only, so "x not y" should fail
        // After parsing "x", "not" has lbp=8 > rbp=0, so it's consumed as infix
        // Then .led() equivalent fires: "Not expecting 'not' as infix operator"
        assert_eq!(
            validate_if_expression(&bits(&["x", "not", "y"])),
            Some("Not expecting 'not' as infix operator in if tag.".to_string())
        );
    }

    // Complex expression chains

    #[test]
    fn valid_mixed_precedence() {
        // x and y or not z — tests mixed and/or/not precedence
        assert!(
            validate_if_expression(&bits(&["x", "and", "y", "or", "not", "z"])).is_none()
        );
    }

    #[test]
    fn valid_double_not() {
        // not not x — prefix chaining
        assert!(validate_if_expression(&bits(&["not", "not", "x"])).is_none());
    }

    #[test]
    fn valid_not_with_comparison() {
        // not x == y
        assert!(
            validate_if_expression(&bits(&["not", "x", "==", "y"])).is_none()
        );
    }

    #[test]
    fn valid_complex_and_or_not() {
        // a == b and c != d or not e
        assert!(
            validate_if_expression(&bits(&[
                "a", "==", "b", "and", "c", "!=", "d", "or", "not", "e"
            ]))
            .is_none()
        );
    }

    #[test]
    fn valid_is_and_is_not_together() {
        // x is y and a is not b
        assert!(
            validate_if_expression(&bits(&[
                "x", "is", "y", "and", "a", "is", "not", "b"
            ]))
            .is_none()
        );
    }

    #[test]
    fn valid_in_and_not_in_together() {
        // x in y and a not in b
        assert!(
            validate_if_expression(&bits(&[
                "x", "in", "y", "and", "a", "not", "in", "b"
            ]))
            .is_none()
        );
    }

    #[test]
    fn valid_all_comparison_ops() {
        // a == b and c != d and e > f and g >= h and i < j and k <= l
        assert!(
            validate_if_expression(&bits(&[
                "a", "==", "b", "and", "c", "!=", "d", "and", "e", ">", "f", "and", "g",
                ">=", "h", "and", "i", "<", "j", "and", "k", "<=", "l"
            ]))
            .is_none()
        );
    }

    #[test]
    fn valid_not_before_in() {
        // not x in y — "not" is prefix, then "x in y" is infix
        assert!(
            validate_if_expression(&bits(&["not", "x", "in", "y"])).is_none()
        );
    }

    // Additional invalid cases

    #[test]
    fn invalid_or_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["or", "x"])),
            Some("Not expecting 'or' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_double_and() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "and", "and", "y"])),
            Some("Not expecting 'and' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_trailing_and() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "and"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_trailing_or() {
        assert_eq!(
            validate_if_expression(&bits(&["x", "or"])),
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_eq_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["==", "x"])),
            Some("Not expecting '==' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_is_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["is", "x"])),
            Some("Not expecting 'is' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn invalid_is_not_at_start() {
        assert_eq!(
            validate_if_expression(&bits(&["is", "not", "x"])),
            Some("Not expecting 'is not' in this position in if tag.".to_string())
        );
    }
}
