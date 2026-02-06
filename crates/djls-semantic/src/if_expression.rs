use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::Db;
use crate::OpaqueRegions;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Validate `{% if %}` and `{% elif %}` expression syntax.
///
/// Ports Django's `smartif.py` Pratt parser to detect compile-time expression
/// syntax errors: operator in operand position, missing right operand, missing
/// operator between operands, dangling unary operator.
///
/// Produces S114 (`ExpressionSyntaxError`) diagnostics.
pub fn validate_if_expressions(
    db: &dyn Db,
    nodelist: NodeList<'_>,
    opaque_regions: &OpaqueRegions,
) {
    for node in nodelist.nodelist(db) {
        let Node::Tag {
            name, bits, span, ..
        } = node
        else {
            continue;
        };

        if opaque_regions.is_opaque(span.start()) {
            continue;
        }

        if name != "if" && name != "elif" {
            continue;
        }

        if let Some(message) = validate_expression(bits) {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            ValidationErrorAccumulator(ValidationError::ExpressionSyntaxError {
                tag: name.clone(),
                message,
                span: marker_span,
            })
            .accumulate(db);
        }
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

// ── Token types ──────────────────────────────────────────────────

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

// ── Tokenizer ────────────────────────────────────────────────────

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

// ── Pratt parser ─────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn validate(expr: &str) -> Option<String> {
        validate_expression(&tokens(expr))
    }

    // ── Valid expressions ─────────────────────────────────────

    #[test]
    fn simple_literal() {
        assert_eq!(validate("x"), None);
    }

    #[test]
    fn and_expression() {
        assert_eq!(validate("x and y"), None);
    }

    #[test]
    fn or_expression() {
        assert_eq!(validate("x or y"), None);
    }

    #[test]
    fn not_expression() {
        assert_eq!(validate("not x"), None);
    }

    #[test]
    fn in_expression() {
        assert_eq!(validate("x in y"), None);
    }

    #[test]
    fn not_in_expression() {
        assert_eq!(validate("x not in y"), None);
    }

    #[test]
    fn is_expression() {
        assert_eq!(validate("x is y"), None);
    }

    #[test]
    fn is_not_expression() {
        assert_eq!(validate("x is not y"), None);
    }

    #[test]
    fn comparison_operators() {
        assert_eq!(validate("x == y"), None);
        assert_eq!(validate("x != y"), None);
        assert_eq!(validate("x > y"), None);
        assert_eq!(validate("x >= y"), None);
        assert_eq!(validate("x < y"), None);
        assert_eq!(validate("x <= y"), None);
    }

    #[test]
    fn complex_expression() {
        assert_eq!(validate("x and not y or z in w"), None);
    }

    #[test]
    fn nested_not() {
        assert_eq!(validate("not not x"), None);
    }

    #[test]
    fn chained_and_or() {
        assert_eq!(validate("a and b or c and d"), None);
    }

    #[test]
    fn comparison_chain() {
        // Django's parser allows this (unlike Python proper)
        assert_eq!(validate("x == y and y != z"), None);
    }

    // ── Invalid expressions ───────────────────────────────────

    #[test]
    fn operator_in_operand_position() {
        let result = validate("and x");
        assert_eq!(
            result,
            Some("Not expecting 'and' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn missing_right_operand() {
        let result = validate("x ==");
        assert_eq!(
            result,
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn missing_operator_between_operands() {
        let result = validate("x y");
        assert_eq!(
            result,
            Some("Unused 'y' at end of if expression.".to_string())
        );
    }

    #[test]
    fn dangling_not() {
        let result = validate("not");
        assert_eq!(
            result,
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn empty_expression() {
        let result = validate_expression(&[]);
        assert_eq!(
            result,
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn or_in_prefix_position() {
        let result = validate("or x");
        assert_eq!(
            result,
            Some("Not expecting 'or' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn double_operator() {
        let result = validate("x and and y");
        assert_eq!(
            result,
            Some("Not expecting 'and' in this position in if tag.".to_string())
        );
    }

    #[test]
    fn trailing_operator() {
        let result = validate("x and");
        assert_eq!(
            result,
            Some("Unexpected end of expression in if tag.".to_string())
        );
    }

    #[test]
    fn in_as_prefix() {
        let result = validate("in x");
        assert_eq!(
            result,
            Some("Not expecting 'in' in this position in if tag.".to_string())
        );
    }

    // ── Integration with validate_if_expressions ──────────────

    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_templates::parse_template;
    use djls_workspace::FileSystem;
    use djls_workspace::InMemoryFileSystem;

    use crate::blocks::TagIndex;
    use crate::templatetags::django_builtin_specs;
    use crate::validate_nodelist;
    use crate::TagSpecs;
    use crate::ValidationErrorAccumulator;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            }
        }

        fn add_file(&self, path: &str, content: &str) {
            self.fs
                .lock()
                .unwrap()
                .add_file(path.into(), content.to_string());
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn create_file(&self, path: &Utf8Path) -> File {
            File::new(self, path.to_owned(), 0)
        }

        fn get_file(&self, _path: &Utf8Path) -> Option<File> {
            None
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            self.fs.lock().unwrap().read_to_string(path)
        }
    }

    #[salsa::db]
    impl djls_templates::Db for TestDatabase {}

    #[salsa::db]
    impl crate::Db for TestDatabase {
        fn tag_specs(&self) -> TagSpecs {
            django_builtin_specs()
        }

        fn tag_index(&self) -> TagIndex<'_> {
            TagIndex::from_specs(self)
        }

        fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
            None
        }

        fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
            djls_conf::DiagnosticsConfig::default()
        }

        fn inspector_inventory(&self) -> Option<djls_project::TemplateTags> {
            None
        }

        fn filter_arity_specs(&self) -> crate::filter_arity::FilterAritySpecs {
            crate::filter_arity::FilterAritySpecs::new()
        }
    }

    fn collect_expression_errors(db: &TestDatabase, source: &str) -> Vec<ValidationError> {
        let path = "test.html";
        db.add_file(path, source);
        let file = db.create_file(Utf8Path::new(path));
        let nodelist = parse_template(db, file).expect("should parse");
        validate_nodelist(db, nodelist);

        validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
            .into_iter()
            .map(|acc| acc.0.clone())
            .filter(|err| matches!(err, ValidationError::ExpressionSyntaxError { .. }))
            .collect()
    }

    #[test]
    fn if_and_x_produces_s114() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if and x %}a{% endif %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::ExpressionSyntaxError { tag, message, .. }
                    if tag == "if" && message.contains("and")
            ),
            "Expected ExpressionSyntaxError for 'and', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn if_x_y_produces_s114() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x y %}a{% endif %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::ExpressionSyntaxError { tag, message, .. }
                    if tag == "if" && message.contains("Unused 'y'")
            ),
            "Expected ExpressionSyntaxError for unused 'y', got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn if_x_eq_produces_s114() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x == %}a{% endif %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::ExpressionSyntaxError { tag, message, .. }
                    if tag == "if" && message.contains("Unexpected end")
            ),
            "Expected ExpressionSyntaxError for missing operand, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn elif_validated() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x %}a\n{% elif and y %}b\n{% endif %}");

        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                &errors[0],
                ValidationError::ExpressionSyntaxError { tag, message, .. }
                    if tag == "elif" && message.contains("and")
            ),
            "Expected ExpressionSyntaxError for elif, got: {:?}",
            errors[0]
        );
    }

    #[test]
    fn valid_if_no_errors() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x and not y or z in w %}a{% endif %}");

        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    #[test]
    fn opaque_region_skips_if_validation() {
        let db = TestDatabase::new();
        let errors =
            collect_expression_errors(&db, "{% verbatim %}{% if and x %}{% endverbatim %}");

        assert!(
            errors.is_empty(),
            "Expected no errors inside verbatim, got: {errors:?}"
        );
    }

    #[test]
    fn not_in_valid() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x not in y %}a{% endif %}");

        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    #[test]
    fn is_not_valid() {
        let db = TestDatabase::new();
        let errors = collect_expression_errors(&db, "{% if x is not None %}a{% endif %}");

        assert!(errors.is_empty(), "Expected no errors, got: {errors:?}");
    }

    #[test]
    fn multiple_errors_in_template() {
        let db = TestDatabase::new();
        let errors =
            collect_expression_errors(&db, "{% if and x %}a{% endif %}\n{% if or y %}b{% endif %}");

        assert_eq!(errors.len(), 2, "Expected 2 errors, got: {errors:?}");
    }
}
