use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_if_expression_rule(db: &dyn Db, name: &str, bits: &[String], span: Span) {
    if let Some(message) = validate_expression(bits) {
        let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
        ValidationErrorAccumulator(ValidationError::ExpressionSyntaxError {
            tag: name.to_string(),
            message,
            span: marker_span,
        })
        .accumulate(db);
    }
}

/// Validate expression tokens for `{% if %}` / `{% elif %}`.
///
/// Returns an error message matching Django's style, or `None` if valid.
fn validate_expression(tokens: &[String]) -> Option<String> {
    if tokens.is_empty() {
        return Some("Unexpected end of expression in if tag.".to_string());
    }

    let mapped = tokenize(tokens);
    let mut parser = IfExpressionParser::new(mapped);
    parser.parse().err()
}

// Token types

#[derive(Debug, Clone)]
enum Token {
    Literal(String),
    Operator(Operator),
    End,
}

impl Token {
    fn lbp(&self) -> u32 {
        match self {
            Token::Literal(_) | Token::End => 0,
            Token::Operator(op) => op.lbp(),
        }
    }

    fn display_name(&self) -> String {
        match self {
            Token::Literal(s) => s.clone(),
            Token::Operator(op) => op.name().to_string(),
            Token::End => "end".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Operator {
    Or,
    And,
    Not,
    In,
    NotIn,
    Is,
    IsNot,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Operator {
    fn lbp(self) -> u32 {
        match self {
            Operator::Or => 6,
            Operator::And => 7,
            Operator::Not => 8,
            Operator::In | Operator::NotIn => 9,
            Operator::Is
            | Operator::IsNot
            | Operator::Eq
            | Operator::Ne
            | Operator::Gt
            | Operator::Ge
            | Operator::Lt
            | Operator::Le => 10,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Operator::Or => "or",
            Operator::And => "and",
            Operator::Not => "not",
            Operator::In => "in",
            Operator::NotIn => "not in",
            Operator::Is => "is",
            Operator::IsNot => "is not",
            Operator::Eq => "==",
            Operator::Ne => "!=",
            Operator::Gt => ">",
            Operator::Ge => ">=",
            Operator::Lt => "<",
            Operator::Le => "<=",
        }
    }

    fn is_prefix(self) -> bool {
        matches!(self, Operator::Not)
    }

    fn is_infix(self) -> bool {
        !matches!(self, Operator::Not)
    }
}

// Tokenizer

fn tokenize(tokens: &[String]) -> Vec<Token> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let token = &tokens[i];
        let mapped = match token.as_str() {
            "is" if i + 1 < tokens.len() && tokens[i + 1] == "not" => {
                i += 1;
                Token::Operator(Operator::IsNot)
            }
            "not" if i + 1 < tokens.len() && tokens[i + 1] == "in" => {
                i += 1;
                Token::Operator(Operator::NotIn)
            }
            "or" => Token::Operator(Operator::Or),
            "and" => Token::Operator(Operator::And),
            "not" => Token::Operator(Operator::Not),
            "in" => Token::Operator(Operator::In),
            "is" => Token::Operator(Operator::Is),
            "==" => Token::Operator(Operator::Eq),
            "!=" => Token::Operator(Operator::Ne),
            ">" => Token::Operator(Operator::Gt),
            ">=" => Token::Operator(Operator::Ge),
            "<" => Token::Operator(Operator::Lt),
            "<=" => Token::Operator(Operator::Le),
            _ => Token::Literal(token.clone()),
        };
        result.push(mapped);
        i += 1;
    }

    result
}

// Pratt parser

struct IfExpressionParser {
    tokens: Vec<Token>,
    pos: usize,
    current: Token,
}

impl IfExpressionParser {
    fn new(tokens: Vec<Token>) -> Self {
        let mut parser = Self {
            tokens,
            pos: 0,
            current: Token::End,
        };
        parser.current = parser.next_token();
        parser
    }

    fn next_token(&mut self) -> Token {
        if self.pos >= self.tokens.len() {
            return Token::End;
        }
        let tok = self.tokens[self.pos].clone();
        self.pos += 1;
        tok
    }

    fn parse(&mut self) -> Result<(), String> {
        self.expression(0)?;
        if !matches!(self.current, Token::End) {
            return Err(format!(
                "Unused '{}' at end of if expression.",
                self.current.display_name()
            ));
        }
        Ok(())
    }

    fn expression(&mut self, rbp: u32) -> Result<(), String> {
        let t = std::mem::replace(&mut self.current, Token::End);
        self.current = self.next_token();
        self.nud(&t)?;
        while rbp < self.current.lbp() {
            let t = std::mem::replace(&mut self.current, Token::End);
            self.current = self.next_token();
            self.led(&t)?;
        }
        Ok(())
    }

    /// Null denotation: handle token in prefix position.
    fn nud(&mut self, token: &Token) -> Result<(), String> {
        match token {
            Token::Literal(_) => Ok(()),
            Token::Operator(op) if op.is_prefix() => self.expression(op.lbp()),
            Token::Operator(op) => Err(format!(
                "Not expecting '{}' in this position in if tag.",
                op.name()
            )),
            Token::End => Err("Unexpected end of expression in if tag.".to_string()),
        }
    }

    /// Left denotation: handle token in infix position.
    fn led(&mut self, token: &Token) -> Result<(), String> {
        match token {
            Token::Operator(op) if op.is_infix() => self.expression(op.lbp()),
            Token::Operator(op) => Err(format!(
                "Not expecting '{}' as infix operator in if tag.",
                op.name()
            )),
            Token::Literal(s) => Err(format!("Unused '{s}' at end of if expression.")),
            Token::End => Err("Unexpected end of expression in if tag.".to_string()),
        }
    }
}
