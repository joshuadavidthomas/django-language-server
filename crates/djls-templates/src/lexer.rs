use crate::tokens::Token;
use crate::tokens::TokenType;

pub struct Lexer {
    source: String,
    chars: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
}

impl Lexer {
    #[must_use]
    pub fn new(source: &str) -> Self {
        Lexer {
            source: String::from(source),
            chars: source.chars().collect(),
            start: 0,
            current: 0,
            line: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();

        while !self.is_at_end() {
            self.start = self.current;

            let token_type = match self.peek() {
                '{' => match self.peek_next() {
                    '%' => self.lex_django_construct("%}", TokenType::Block),
                    '{' => self.lex_django_construct("}}", TokenType::Variable),
                    '#' => self.lex_django_construct("#}", TokenType::Comment),
                    _ => self.lex_text(),
                },
                c if c.is_whitespace() => self.lex_whitespace(c),
                _ => self.lex_text(),
            };

            let token = Token::new(token_type, self.line, Some(self.start));

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

        let eof_token = Token::new(TokenType::Eof, self.line, None);
        tokens.push(eof_token);

        tokens
    }

    fn lex_django_construct(
        &mut self,
        end: &str,
        token_type: fn(String) -> TokenType,
    ) -> TokenType {
        self.consume_n(2);

        match self.consume_until(end) {
            Ok(content) => {
                self.consume_n(2);
                token_type(content)
            }
            Err(err_content) => {
                self.synchronize();
                TokenType::Error(err_content)
            }
        }
    }

    fn lex_whitespace(&mut self, c: char) -> TokenType {
        if c == '\n' || c == '\r' {
            self.consume(); // \r or \n
            if c == '\r' && self.peek() == '\n' {
                self.consume(); // \n of \r\n
            }
            TokenType::Newline
        } else {
            self.consume(); // Consume the first whitespace
            while !self.is_at_end() && self.peek().is_whitespace() {
                if self.peek() == '\n' || self.peek() == '\r' {
                    break;
                }
                self.consume();
            }
            let whitespace_count = self.current - self.start;
            TokenType::Whitespace(whitespace_count)
        }
    }

    fn lex_text(&mut self) -> TokenType {
        let mut text = String::new();
        while !self.is_at_end() {
            let c = self.peek();
            if c == '{' || c == '\n' {
                break;
            }
            text.push(c);
            self.consume();
        }
        TokenType::Text(text)
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
        let index = if offset < 0 {
            match self.current.checked_sub(offset.unsigned_abs()) {
                Some(idx) => idx,
                None => return '\0',
            }
        } else {
            match self.current.checked_add(offset as usize) {
                Some(idx) => idx,
                None => return '\0',
            }
        };

        if index >= self.chars.len() {
            '\0'
        } else {
            self.chars[index]
        }
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

    #[test]
    fn test_tokenize_html() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_comments() {
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
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_script() {
        let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
       comment */
    console.log(x);
</script>"#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_style() {
        let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_nested_delimiters() {
        let source = r"{{ user.name }}
{% if true %}
{# comment #}
<!-- html comment -->
<div>text</div>";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_everything() {
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
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }
}
