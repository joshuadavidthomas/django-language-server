use crate::db::Db as TemplateDb;
use crate::tokens::Token;
use crate::tokens::TokenContent;

pub struct Lexer<'db> {
    db: &'db dyn TemplateDb,
    source: String,
    chars: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
}

impl<'db> Lexer<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, source: &str) -> Self {
        Lexer {
            db,
            source: String::from(source),
            chars: source.chars().collect(),
            start: 0,
            current: 0,
            line: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token<'db>> {
        let mut tokens = Vec::new();

        while !self.is_at_end() {
            self.start = self.current;

            let token = match self.peek() {
                '{' => match self.peek_next() {
                    '%' => self.lex_django_construct("%}", |content, line, start| Token::Block {
                        content,
                        line,
                        start,
                    }),
                    '{' => {
                        self.lex_django_construct("}}", |content, line, start| Token::Variable {
                            content,
                            line,
                            start,
                        })
                    }
                    '#' => self.lex_django_construct("#}", |content, line, start| Token::Comment {
                        content,
                        line,
                        start,
                    }),
                    _ => self.lex_text(),
                },
                c if c.is_whitespace() => self.lex_whitespace(c),
                _ => self.lex_text(),
            };

            match self.peek_previous() {
                '\n' => self.line += 1,
                '\r' => {
                    self.line += 1;
                    if self.peek() == '\n' {
                        self.current += 1;
                    }
                }
                _ => {}
            }

            tokens.push(token);
        }

        tokens.push(Token::Eof { line: self.line });

        tokens
    }

    fn lex_django_construct(
        &mut self,
        end: &str,
        token_fn: impl FnOnce(TokenContent<'db>, usize, usize) -> Token<'db>,
    ) -> Token<'db> {
        let line = self.line;
        let start = self.start + 3;

        self.consume_n(2);

        match self.consume_until(end) {
            Ok(text) => {
                self.consume_n(2);
                let content = TokenContent::new(self.db, text);
                token_fn(content, line, start)
            }
            Err(err_text) => {
                self.synchronize();
                let content = TokenContent::new(self.db, err_text);
                Token::Error {
                    content,
                    line,
                    start,
                }
            }
        }
    }

    fn lex_whitespace(&mut self, c: char) -> Token<'db> {
        let line = self.line;
        let start = self.start;

        if c == '\n' || c == '\r' {
            self.consume(); // \r or \n
            if c == '\r' && self.peek() == '\n' {
                self.consume(); // \n of \r\n
            }
            Token::Newline { line, start }
        } else {
            self.consume(); // Consume the first whitespace
            while !self.is_at_end() && self.peek().is_whitespace() {
                if self.peek() == '\n' || self.peek() == '\r' {
                    break;
                }
                self.consume();
            }
            let count = self.current - self.start;
            Token::Whitespace { count, line, start }
        }
    }

    fn lex_text(&mut self) -> Token<'db> {
        let line = self.line;
        let start = self.start;

        let mut text = String::new();
        while !self.is_at_end() {
            let c = self.peek();

            if c == '{' {
                let next = self.peek_next();
                if next == '%' || next == '{' || next == '#' {
                    break;
                }
            } else if c == '\n' {
                break;
            }

            text.push(c);
            self.consume();
        }

        let content = TokenContent::new(self.db, text);
        Token::Text {
            content,
            line,
            start,
        }
    }

    fn peek(&self) -> char {
        self.peek_at(0)
    }

    fn peek_next(&self) -> char {
        self.peek_at(1)
    }

    fn peek_previous(&self) -> char {
        self.peek_at(-1)
    }

    fn peek_at(&self, offset: isize) -> char {
        let Some(index) = self.current.checked_add_signed(offset) else {
            return '\0';
        };
        self.chars.get(index).copied().unwrap_or('\0')
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    fn consume(&mut self) {
        if self.is_at_end() {
            return;
        }
        self.current += 1;
    }

    fn consume_n(&mut self, count: usize) {
        for _ in 0..count {
            self.consume();
        }
    }

    fn consume_until(&mut self, s: &str) -> Result<String, String> {
        let start = self.current;
        while !self.is_at_end() {
            if self.chars[self.current..self.chars.len()]
                .starts_with(s.chars().collect::<Vec<_>>().as_slice())
            {
                return Ok(self.source[start..self.current].trim().to_string());
            }
            self.consume();
        }
        Err(self.source[start..self.current].trim().to_string())
    }

    fn synchronize(&mut self) {
        let sync_chars = &['{', '\n', '\r'];

        while !self.is_at_end() {
            let current_char = self.peek();
            if sync_chars.contains(&current_char) {
                return;
            }
            self.consume();
        }
    }
}

#[cfg(test)]
mod tests {
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
    impl djls_workspace::Db for TestDatabase {
        fn fs(&self) -> std::sync::Arc<dyn djls_workspace::FileSystem> {
            use djls_workspace::InMemoryFileSystem;
            static FS: std::sync::OnceLock<std::sync::Arc<InMemoryFileSystem>> =
                std::sync::OnceLock::new();
            FS.get_or_init(|| std::sync::Arc::new(InMemoryFileSystem::default()))
                .clone()
        }

        fn read_file_content(&self, path: &std::path::Path) -> Result<String, std::io::Error> {
            std::fs::read_to_string(path)
        }
    }

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        fn tag_specs(&self) -> std::sync::Arc<crate::templatetags::TagSpecs> {
            std::sync::Arc::new(
                crate::templatetags::TagSpecs::load_builtin_specs()
                    .unwrap_or_else(|_| crate::templatetags::TagSpecs::default()),
            )
        }
    }

    #[test]
    fn test_tokenize_html() {
        let db = TestDatabase::new();
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(&db, source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let db = TestDatabase::new();
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(&db, source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_block() {
        let db = TestDatabase::new();
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(&db, source);
        let tokens = lexer.tokenize();
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
        let tokens = lexer.tokenize();
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
        let tokens = lexer.tokenize();
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
        let tokens = lexer.tokenize();
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
        let tokens = lexer.tokenize();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_unclosed_style() {
        let db = TestDatabase::new();
        let source = "<style>body { color: blue; ";
        let mut lexer = Lexer::new(&db, source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot(&db);
        insta::assert_yaml_snapshot!(snapshot);
    }
}
