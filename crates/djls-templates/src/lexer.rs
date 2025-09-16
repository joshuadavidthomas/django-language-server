use crate::db::Db as TemplateDb;
use crate::nodelist::LineOffsets;
use crate::tokens::Token;
use crate::tokens::TokenContent;

const BLOCK_TAG_START: &str = "{%";
const BLOCK_TAG_END: &str = "%}";
const VARIABLE_TAG_START: &str = "{{";
const VARIABLE_TAG_END: &str = "}}";
const COMMENT_TAG_START: &str = "{#";
const COMMENT_TAG_END: &str = "#}";

pub struct Lexer<'db> {
    db: &'db dyn TemplateDb,
    source: String,
    start: usize,
    current: usize,
}

impl<'db> Lexer<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, source: &str) -> Self {
        Lexer {
            db,
            source: String::from(source),
            start: 0,
            current: 0,
        }
    }

    pub fn tokenize(&mut self) -> (Vec<Token<'db>>, LineOffsets) {
        let mut tokens = Vec::new();
        let mut line_offsets = LineOffsets::default();

        while !self.is_at_end() {
            self.start = self.current;

            let token = match self.peek() {
                '{' => match self.peek_next() {
                    '%' => self.lex_django_construct(BLOCK_TAG_END, |content, offset| {
                        Token::Block { content, offset }
                    }),
                    '{' => self.lex_django_construct(VARIABLE_TAG_END, |content, offset| {
                        Token::Variable { content, offset }
                    }),
                    '#' => self.lex_django_construct(COMMENT_TAG_END, |content, offset| {
                        Token::Comment { content, offset }
                    }),
                    _ => self.lex_text(),
                },
                c if c.is_whitespace() => self.lex_whitespace(c),
                _ => self.lex_text(),
            };

            match self.peek_previous() {
                '\n' => line_offsets.add_line(u32::try_from(self.current).unwrap_or(u32::MAX)),
                '\r' => {
                    line_offsets.add_line(u32::try_from(self.current).unwrap_or(u32::MAX));
                    if self.peek() == '\n' {
                        self.current += 1;
                    }
                }
                _ => {}
            }

            tokens.push(token);
        }

        tokens.push(Token::Eof);

        (tokens, line_offsets)
    }

    fn lex_django_construct(
        &mut self,
        end: &str,
        token_fn: impl FnOnce(TokenContent<'db>, usize) -> Token<'db>,
    ) -> Token<'db> {
        let offset = self.start + 3;

        self.consume_n(2);

        match self.consume_until(end) {
            Ok(text) => {
                self.consume_n(2);
                let content = TokenContent::new(self.db, text);
                token_fn(content, offset)
            }
            Err(err_text) => {
                self.synchronize();
                let content = TokenContent::new(self.db, err_text);
                Token::Error { content, offset }
            }
        }
    }

    fn lex_whitespace(&mut self, c: char) -> Token<'db> {
        let offset = self.start;

        if c == '\n' || c == '\r' {
            self.consume(); // \r or \n
            if c == '\r' && self.peek() == '\n' {
                self.consume(); // \n of \r\n
            }
            Token::Newline { offset }
        } else {
            self.consume(); // Consume the first whitespace
            while !self.is_at_end() && self.peek().is_whitespace() {
                if self.peek() == '\n' || self.peek() == '\r' {
                    break;
                }
                self.consume();
            }
            let count = self.current - self.start;
            Token::Whitespace { count, offset }
        }
    }

    fn lex_text(&mut self) -> Token<'db> {
        let text_start = self.current;

        while !self.is_at_end() {
            if self.source[self.current..].starts_with(BLOCK_TAG_START)
                || self.source[self.current..].starts_with(VARIABLE_TAG_START)
                || self.source[self.current..].starts_with(COMMENT_TAG_START)
                || self.source[self.current..].starts_with('\n')
            {
                break;
            }
            self.consume();
        }

        let text = &self.source[text_start..self.current];
        let content = TokenContent::new(self.db, text.to_string());
        Token::Text {
            content,
            offset: self.start,
        }
    }

    #[inline]
    fn peek(&self) -> char {
        self.source[self.current..].chars().next().unwrap_or('\0')
    }

    fn peek_next(&self) -> char {
        let mut chars = self.source[self.current..].chars();
        chars.next(); // Skip current
        chars.next().unwrap_or('\0')
    }

    fn peek_previous(&self) -> char {
        if self.current == 0 {
            return '\0';
        }
        let mut pos = self.current - 1;
        while !self.source.is_char_boundary(pos) && pos > 0 {
            pos -= 1;
        }
        self.source[pos..].chars().next().unwrap_or('\0')
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    #[inline]
    fn consume(&mut self) {
        if let Some(ch) = self.source[self.current..].chars().next() {
            self.current += ch.len_utf8();
        }
    }

    fn consume_n(&mut self, count: usize) {
        for _ in 0..count {
            self.consume();
        }
    }

    fn consume_until(&mut self, delimiter: &str) -> Result<String, String> {
        let offset = self.current;

        while self.current < self.source.len() {
            if self.source[self.current..].starts_with(delimiter) {
                return Ok(self.source[offset..self.current].trim().to_string());
            }
            self.consume();
        }

        Err(self.source[offset..self.current].trim().to_string())
    }

    fn synchronize(&mut self) {
        const SYNC_POINTS: &[u8] = b"{\n\r";

        while !self.is_at_end() {
            if SYNC_POINTS.contains(&self.source.as_bytes()[self.current]) {
                return;
            }
            self.consume();
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;
    use crate::tokens::TokenSnapshotVec;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_source::Db for TestDatabase {
        fn read_file_source(&self, path: &Utf8Path) -> Result<String, std::io::Error> {
            std::fs::read_to_string(path)
        }
    }

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        // Template parsing only - semantic analysis moved to djls-semantic
    }

    #[test]
    fn test_tokenize_html() {
        let db = TestDatabase::new();
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let db = TestDatabase::new();
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_block() {
        let db = TestDatabase::new();
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_comments() {
        let db = TestDatabase::new();
        let source = r"<!-- HTML comment -->
{# Django comment #}
<script>
    // JS single line comment
    /* JS multi-line
       comment */
</script>
<style>
    /* CSS comment */
</style>";
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_script() {
        let db = TestDatabase::new();
        let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
       comment */
    console.log(x);
</script>"#;
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_style() {
        let db = TestDatabase::new();
        let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#;
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_nested_delimiters() {
        let db = TestDatabase::new();
        let source = r"{{ user.name }}
{% if true %}
{# comment #}
<!-- html comment -->
<div>text</div>";
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_everything() {
        let db = TestDatabase::new();
        let source = r#"<!DOCTYPE html>
<html>
<head>
    <style type="text/css">
        /* Style header */
        .header { color: blue; }
    </style>
    <script type="text/javascript">
        // Init app
        const app = {
            /* Config */
            debug: true
        };
    </script>
</head>
<body>
    <!-- Header section -->
    <div class="header" id="main" data-value="123" disabled>
        {% if user.is_authenticated %}
            {# Welcome message #}
            <h1>Welcome, {{ user.name|default:"Guest"|title }}!</h1>
            {% if user.is_staff %}
                <span>Admin</span>
            {% else %}
                <span>User</span>
            {% endif %}
        {% endif %}
    </div>
</body>
</html>"#;
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_unclosed_style() {
        let db = TestDatabase::new();
        let source = "<style>body { color: blue; ";
        let mut lexer = Lexer::new(&db, source);
        let (tokens, _) = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }
}
