// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug)]
pub struct SyntaxError {
    pub kind: SyntaxErrorKind,
    pub pos: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub enum SyntaxErrorKind {
    ExpectAttrName,
    ExpectAttrValue,
    ExpectCdata,
    ExpectChar(char),
    ExpectCloseTag {
        tag_name: String,
        line: usize,
        column: usize,
    },
    ExpectComment,
    ExpectDoctype,
    ExpectElement,
    ExpectIdentifier,
    ExpectJinjaBlockEnd,
    ExpectJinjaTag,
    ExpectKeyword(&'static str),
    ExpectSelfCloseTag,
    ExpectTagName,
    ExpectTextNode,
    ExpectXmlDecl,
}

impl fmt::Display for SyntaxErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason: Cow<_> = match self {
            SyntaxErrorKind::ExpectAttrName => "expected attribute name".into(),
            SyntaxErrorKind::ExpectAttrValue => "expected attribute value".into(),
            SyntaxErrorKind::ExpectCdata => "expected CDATA section".into(),
            SyntaxErrorKind::ExpectChar(c) => format!("expected char '{c}'").into(),
            SyntaxErrorKind::ExpectCloseTag {
                tag_name,
                line,
                column,
            } => format!(
                "expected close tag for opening tag <{tag_name}> from line {line}, column {column}"
            )
            .into(),
            SyntaxErrorKind::ExpectComment => "expected comment".into(),
            SyntaxErrorKind::ExpectDoctype => "expected HTML doctype".into(),
            SyntaxErrorKind::ExpectElement => "expected element".into(),
            SyntaxErrorKind::ExpectIdentifier => "expected identifier".into(),
            SyntaxErrorKind::ExpectJinjaBlockEnd => "expected Jinja block end".into(),
            SyntaxErrorKind::ExpectJinjaTag => "expected Jinja tag".into(),
            SyntaxErrorKind::ExpectKeyword(keyword) => {
                format!("expected keyword '{keyword}'").into()
            }
            SyntaxErrorKind::ExpectSelfCloseTag => "expected self close tag".into(),
            SyntaxErrorKind::ExpectTagName => "expected tag name".into(),
            SyntaxErrorKind::ExpectTextNode => "expected text node".into(),
            SyntaxErrorKind::ExpectXmlDecl => "expected XML declaration".into(),
        };

        write!(f, "{reason}")
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "syntax error '{}' at line {}, column {}",
            self.kind, self.line, self.column
        )
    }
}

impl Error for SyntaxError {}

#[derive(Debug)]
pub enum FormatError<E> {
    Syntax(SyntaxError),
    External(Vec<E>),
}

impl<E> fmt::Display for FormatError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Syntax(e) => e.fmt(f),
            FormatError::External(errors) => {
                writeln!(f, "failed to format code with external formatter:")?;
                for error in errors {
                    writeln!(f, "{error}")?;
                }
                Ok(())
            }
        }
    }
}

impl<E> Error for FormatError<E> where E: Error {}
