// Vendored from markup_fmt v0.26.0 — MIT License (see LICENSE)
// Stripped to HTML + Jinja/Django + XML only
// Django fixes applied to block tag list

use std::cmp::Ordering;
use std::iter::Peekable;
use std::ops::ControlFlow;
use std::str::CharIndices;

use crate::markup::ast::*;
use crate::markup::error::SyntaxError;
use crate::markup::error::SyntaxErrorKind;
use crate::markup::helpers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Html,
    Jinja,
    Xml,
}

pub struct Parser<'s> {
    source: &'s str,
    language: Language,
    chars: Peekable<CharIndices<'s>>,
}

impl<'s> Parser<'s> {
    pub fn new(source: &'s str, language: Language) -> Self {
        Self {
            source,
            language,
            chars: source.char_indices().peekable(),
        }
    }

    fn try_parse<F, R>(&mut self, f: F) -> PResult<R>
    where
        F: FnOnce(&mut Self) -> PResult<R>,
    {
        let chars = self.chars.clone();
        let result = f(self);
        if result.is_err() {
            self.chars = chars;
        }
        result
    }

    #[inline]
    fn peek_pos(&mut self) -> usize {
        self.chars
            .peek()
            .map(|(i, _)| *i)
            .unwrap_or(self.source.len())
    }

    fn emit_error(&mut self, kind: SyntaxErrorKind) -> SyntaxError {
        let pos = self.peek_pos();
        self.emit_error_with_pos(kind, pos)
    }

    fn emit_error_with_pos(&self, kind: SyntaxErrorKind, pos: usize) -> SyntaxError {
        let (line, column) = self.pos_to_line_col(pos);
        SyntaxError {
            kind,
            pos,
            line,
            column,
        }
    }

    fn pos_to_line_col(&self, pos: usize) -> (usize, usize) {
        let search = memchr::memchr_iter(b'\n', self.source.as_bytes()).try_fold(
            (1, 0),
            |(line, prev_offset), offset| match pos.cmp(&offset) {
                Ordering::Less | Ordering::Equal => ControlFlow::Break((line, prev_offset)),
                Ordering::Greater => ControlFlow::Continue((line + 1, offset)),
            },
        );
        match search {
            ControlFlow::Break((line, offset)) => (line, pos - offset + 1),
            ControlFlow::Continue((line, _)) => (line, 0),
        }
    }

    fn skip_ws(&mut self) {
        while self
            .chars
            .next_if(|(_, c)| c.is_ascii_whitespace())
            .is_some()
        {}
    }

    fn try_consume_str(&mut self, s: &str) -> Option<(usize, char)> {
        let mut chars = self.chars.clone();
        let mut last = None;

        for expected in s.chars() {
            match chars.next() {
                Some((idx, c)) if c == expected => {
                    last = Some((idx, c));
                }
                _ => return None,
            }
        }

        self.chars = chars;
        last
    }

    fn try_consume_str_ignore_case(&mut self, s: &str) -> Option<(usize, char)> {
        let mut chars = self.chars.clone();
        let mut last = None;

        for expected in s.chars() {
            match chars.next() {
                Some((idx, c)) if c.eq_ignore_ascii_case(&expected) => {
                    last = Some((idx, c));
                }
                _ => return None,
            }
        }

        self.chars = chars;
        last
    }

    fn with_taken<T, F>(&mut self, parser: F) -> PResult<(T, &'s str)>
    where
        F: FnOnce(&mut Self) -> PResult<T>,
    {
        let start = self.peek_pos();
        let parsed = parser(self)?;
        let end = self.peek_pos();
        Ok((parsed, unsafe { self.source.get_unchecked(start..end) }))
    }

    fn parse_attr(&mut self) -> PResult<Attribute<'s>> {
        match self.language {
            Language::Html | Language::Xml => self.parse_native_attr().map(Attribute::Native),
            Language::Jinja => {
                self.skip_ws();
                let result = if matches!(self.chars.peek(), Some((_, '{'))) {
                    let mut chars = self.chars.clone();
                    chars.next();
                    match chars.next() {
                        Some((_, '{')) => self.parse_native_attr().map(Attribute::Native),
                        Some((_, '#')) => self.parse_jinja_comment().map(Attribute::JinjaComment),
                        _ => self.parse_jinja_tag_or_block(None, &mut Parser::parse_attr),
                    }
                } else {
                    self.parse_native_attr().map(Attribute::Native)
                };
                if result.is_ok() {
                    self.skip_ws();
                }
                result
            }
        }
    }

    fn parse_attr_name(&mut self) -> PResult<&'s str> {
        if matches!(self.language, Language::Jinja) {
            let Some((start, mut end)) = (match self.chars.peek() {
                Some((i, '{')) => {
                    let start = *i;
                    let mut chars = self.chars.clone();
                    chars.next();
                    if let Some((_, '{')) = chars.next() {
                        let end =
                            start + self.parse_mustache_interpolation()?.0.len() + "{{}}".len();
                        Some((start, end))
                    } else {
                        None
                    }
                }
                Some((_, c)) if is_attr_name_char(*c) => self
                    .chars
                    .next()
                    .map(|(start, c)| (start, start + c.len_utf8())),
                _ => None,
            }) else {
                return Err(self.emit_error(SyntaxErrorKind::ExpectAttrName));
            };

            while let Some((_, c)) = self.chars.peek() {
                if is_attr_name_char(*c) && *c != '{' {
                    end += c.len_utf8();
                    self.chars.next();
                } else if *c == '{' {
                    let mut chars = self.chars.clone();
                    chars.next();
                    match chars.next() {
                        Some((_, '%')) => {
                            break;
                        }
                        Some((_, '{')) => {
                            end += self.parse_mustache_interpolation()?.0.len() + "{{}}".len();
                        }
                        Some((_, c)) => {
                            end += c.len_utf8();
                            self.chars.next();
                        }
                        None => break,
                    }
                } else {
                    break;
                }
            }

            unsafe { Ok(self.source.get_unchecked(start..end)) }
        } else {
            let Some((start, start_char)) = self.chars.next_if(|(_, c)| is_attr_name_char(*c))
            else {
                return Err(self.emit_error(SyntaxErrorKind::ExpectAttrName));
            };
            let mut end = start + start_char.len_utf8();

            while let Some((_, c)) = self.chars.next_if(|(_, c)| is_attr_name_char(*c)) {
                end += c.len_utf8();
            }

            unsafe { Ok(self.source.get_unchecked(start..end)) }
        }
    }

    fn parse_attr_value(&mut self) -> PResult<(&'s str, usize)> {
        let quote = self.chars.next_if(|(_, c)| *c == '"' || *c == '\'');

        if let Some((start, quote)) = quote {
            let can_interpolate = matches!(self.language, Language::Jinja);
            let start = start + 1;
            let mut end = start;
            let mut chars_stack = vec![];
            loop {
                match self.chars.next() {
                    Some((i, c)) if c == quote => {
                        if chars_stack.is_empty() || !can_interpolate {
                            end = i;
                            break;
                        } else if chars_stack.last().is_some_and(|last| *last == c) {
                            chars_stack.pop();
                        } else {
                            chars_stack.push(c);
                        }
                    }
                    Some((_, '{')) if can_interpolate => {
                        chars_stack.push('{');
                    }
                    Some((_, '}'))
                        if can_interpolate
                            && chars_stack.last().is_some_and(|last| *last == '{') =>
                    {
                        chars_stack.pop();
                    }
                    Some(..) => continue,
                    None => break,
                }
            }
            Ok((unsafe { self.source.get_unchecked(start..end) }, start))
        } else {
            fn is_unquoted_attr_value_char(c: char) -> bool {
                !c.is_ascii_whitespace() && !matches!(c, '"' | '\'' | '=' | '<' | '>' | '`')
            }

            let start = match self.chars.peek() {
                Some((i, c)) if is_unquoted_attr_value_char(*c) => *i,
                _ => return Err(self.emit_error(SyntaxErrorKind::ExpectAttrValue)),
            };

            let mut end = start;
            loop {
                match self.chars.peek() {
                    Some((i, '{')) if matches!(self.language, Language::Jinja) => {
                        end = *i;
                        let mut chars = self.chars.clone();
                        chars.next();
                        match chars.peek() {
                            Some((_, '%')) => {
                                if self
                                    .parse_jinja_tag_or_block(None, &mut Parser::parse_node)
                                    .is_ok()
                                {
                                    end =
                                        self.chars.peek().map(|(i, _)| i - 1).ok_or_else(|| {
                                            self.emit_error(SyntaxErrorKind::ExpectAttrValue)
                                        })?;
                                } else {
                                    self.chars.next();
                                }
                            }
                            Some((_, '{')) => {
                                chars.next();
                                let (interpolation, _) = self.parse_mustache_interpolation()?;
                                end += interpolation.len() + "{{}}".len() - 1;
                            }
                            _ => {
                                self.chars.next();
                            }
                        }
                    }
                    Some((i, c)) if is_unquoted_attr_value_char(*c) => {
                        end = *i;
                        self.chars.next();
                    }
                    _ => break,
                }
            }

            Ok((unsafe { self.source.get_unchecked(start..=end) }, start))
        }
    }

    fn parse_cdata(&mut self) -> PResult<Cdata<'s>> {
        let Some((start, _)) = self.try_consume_str("<![CDATA[") else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectCdata));
        };
        let start = start + 1;

        let mut end = start;
        loop {
            match self.chars.next() {
                Some((i, ']')) => {
                    let mut chars = self.chars.clone();
                    if chars
                        .next_if(|(_, c)| *c == ']')
                        .and_then(|_| chars.next_if(|(_, c)| *c == '>'))
                        .is_some()
                    {
                        end = i;
                        self.chars = chars;
                        break;
                    }
                }
                Some(..) => continue,
                None => break,
            }
        }

        Ok(Cdata {
            raw: unsafe { self.source.get_unchecked(start..end) },
        })
    }

    fn parse_comment(&mut self) -> PResult<Comment<'s>> {
        let Some((start, _)) = self
            .chars
            .next_if(|(_, c)| *c == '<')
            .and_then(|_| self.try_consume_str("!--"))
        else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectComment));
        };
        let start = start + 1;

        let mut end = start;
        loop {
            match self.chars.next() {
                Some((i, '-')) => {
                    let mut chars = self.chars.clone();
                    if chars
                        .next_if(|(_, c)| *c == '-')
                        .and_then(|_| chars.next_if(|(_, c)| *c == '>'))
                        .is_some()
                    {
                        end = i;
                        self.chars = chars;
                        break;
                    }
                }
                Some(..) => continue,
                None => break,
            }
        }

        Ok(Comment {
            raw: unsafe { self.source.get_unchecked(start..end) },
        })
    }

    fn parse_doctype(&mut self) -> PResult<Doctype<'s>> {
        let keyword_start = if let Some((start, _)) = self.try_consume_str("<!") {
            start + 1
        } else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectDoctype));
        };
        let keyword = if let Some((end, _)) = self.try_consume_str_ignore_case("doctype") {
            unsafe { self.source.get_unchecked(keyword_start..end + 1) }
        } else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectDoctype));
        };
        self.skip_ws();

        let value_start = if let Some((start, _)) = self.chars.peek() {
            *start
        } else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectDoctype));
        };
        while self.chars.next_if(|(_, c)| *c != '>').is_some() {}

        if let Some((value_end, _)) = self.chars.next_if(|(_, c)| *c == '>') {
            Ok(Doctype {
                keyword,
                value: unsafe { self.source.get_unchecked(value_start..value_end) }.trim_end(),
            })
        } else {
            Err(self.emit_error(SyntaxErrorKind::ExpectDoctype))
        }
    }

    fn parse_element(&mut self) -> PResult<Element<'s>> {
        let Some((element_start, _)) = self.chars.next_if(|(_, c)| *c == '<') else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectElement));
        };
        let tag_name = self.parse_tag_name()?;
        let void_element = helpers::is_void_element(tag_name, self.language);

        let mut attrs = vec![];
        let mut first_attr_same_line = true;
        loop {
            match self.chars.peek() {
                Some((_, '/')) => {
                    self.chars.next();
                    if self.chars.next_if(|(_, c)| *c == '>').is_some() {
                        return Ok(Element {
                            tag_name,
                            attrs,
                            first_attr_same_line,
                            children: vec![],
                            self_closing: true,
                            void_element,
                        });
                    }
                    return Err(self.emit_error(SyntaxErrorKind::ExpectSelfCloseTag));
                }
                Some((_, '>')) => {
                    self.chars.next();
                    if void_element {
                        return Ok(Element {
                            tag_name,
                            attrs,
                            first_attr_same_line,
                            children: vec![],
                            self_closing: false,
                            void_element,
                        });
                    }
                    break;
                }
                Some((_, '\n')) => {
                    if attrs.is_empty() {
                        first_attr_same_line = false;
                    }
                    self.chars.next();
                }
                Some((_, c)) if c.is_ascii_whitespace() => {
                    self.chars.next();
                }
                _ => {
                    attrs.push(self.parse_attr()?);
                }
            }
        }

        let mut children = vec![];
        let should_parse_raw = self.language != Language::Xml
            && (tag_name.eq_ignore_ascii_case("script")
                || tag_name.eq_ignore_ascii_case("style")
                || tag_name.eq_ignore_ascii_case("pre")
                || tag_name.eq_ignore_ascii_case("textarea"));
        if should_parse_raw {
            let text_node = self.parse_raw_text_node(tag_name)?;
            let raw = text_node.raw;
            if !raw.is_empty() {
                children.push(Node {
                    kind: NodeKind::Text(text_node),
                    raw,
                });
            }
        }

        loop {
            match self.chars.peek() {
                Some((_, '<')) => {
                    let mut chars = self.chars.clone();
                    chars.next();
                    if let Some((pos, _)) = chars.next_if(|(_, c)| *c == '/') {
                        self.chars = chars;
                        let close_tag_name = self.parse_tag_name()?;
                        if !close_tag_name.eq_ignore_ascii_case(tag_name) {
                            let (line, column) = self.pos_to_line_col(element_start);
                            return Err(self.emit_error_with_pos(
                                SyntaxErrorKind::ExpectCloseTag {
                                    tag_name: tag_name.into(),
                                    line,
                                    column,
                                },
                                pos,
                            ));
                        }
                        self.skip_ws();
                        if self.chars.next_if(|(_, c)| *c == '>').is_some() {
                            break;
                        }
                        let (line, column) = self.pos_to_line_col(element_start);
                        return Err(self.emit_error(SyntaxErrorKind::ExpectCloseTag {
                            tag_name: tag_name.into(),
                            line,
                            column,
                        }));
                    }
                    children.push(self.parse_node()?);
                }
                Some(..) => {
                    if should_parse_raw {
                        let text_node = self.parse_raw_text_node(tag_name)?;
                        let raw = text_node.raw;
                        if !raw.is_empty() {
                            children.push(Node {
                                kind: NodeKind::Text(text_node),
                                raw,
                            });
                        }
                    } else {
                        children.push(self.parse_node()?);
                    }
                }
                None => {
                    let (line, column) = self.pos_to_line_col(element_start);
                    return Err(self.emit_error(SyntaxErrorKind::ExpectCloseTag {
                        tag_name: tag_name.into(),
                        line,
                        column,
                    }));
                }
            }
        }

        Ok(Element {
            tag_name,
            attrs,
            first_attr_same_line,
            children,
            self_closing: false,
            void_element,
        })
    }

    fn parse_identifier(&mut self) -> PResult<&'s str> {
        fn is_identifier_char(c: char) -> bool {
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() || c == '\\'
        }

        let Some((start, _)) = self.chars.next_if(|(_, c)| is_identifier_char(*c)) else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectIdentifier));
        };
        let mut end = start;

        while let Some((i, _)) = self.chars.next_if(|(_, c)| is_identifier_char(*c)) {
            end = i;
        }

        unsafe { Ok(self.source.get_unchecked(start..=end)) }
    }

    fn parse_jinja_block_children<T, F>(&mut self, children_parser: &mut F) -> PResult<Vec<T>>
    where
        T: HasJinjaFlowControl<'s>,
        F: FnMut(&mut Self) -> PResult<T>,
    {
        let mut children = vec![];
        loop {
            match self.chars.peek() {
                Some((_, '{')) => {
                    let mut chars = self.chars.clone();
                    chars.next();
                    if chars.next_if(|(_, c)| *c == '%').is_some() {
                        break;
                    }
                    children.push(children_parser(self)?);
                }
                Some(..) => {
                    children.push(children_parser(self)?);
                }
                None => return Err(self.emit_error(SyntaxErrorKind::ExpectJinjaBlockEnd)),
            }
        }
        Ok(children)
    }

    fn parse_jinja_comment(&mut self) -> PResult<JinjaComment<'s>> {
        let Some((start, _)) = self.try_consume_str("{#") else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectComment));
        };
        let start = start + 1;

        let end;
        loop {
            match self.chars.next() {
                Some((i, '#')) => {
                    let mut chars = self.chars.clone();
                    if chars.next_if(|(_, c)| *c == '}').is_some() {
                        end = i;
                        self.chars = chars;
                        break;
                    }
                }
                Some(..) => continue,
                None => {
                    end = self.source.len();
                    break;
                }
            }
        }

        Ok(JinjaComment {
            raw: unsafe { self.source.get_unchecked(start..end) },
        })
    }

    fn parse_jinja_tag(&mut self) -> PResult<JinjaTag<'s>> {
        let Some((start, _)) = self.try_consume_str("{%") else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectJinjaTag));
        };
        let start = start + 1;

        let mut end = start;
        loop {
            match self.chars.next() {
                Some((i, '%')) => {
                    if self.chars.next_if(|(_, c)| *c == '}').is_some() {
                        end = i;
                        break;
                    }
                }
                Some(..) => continue,
                None => break,
            }
        }

        Ok(JinjaTag {
            content: unsafe { self.source.get_unchecked(start..end) },
            start,
        })
    }

    fn parse_jinja_tag_or_block<T, F>(
        &mut self,
        first_tag: Option<JinjaTag<'s>>,
        children_parser: &mut F,
    ) -> PResult<T::Intermediate>
    where
        T: HasJinjaFlowControl<'s>,
        F: FnMut(&mut Self) -> PResult<T>,
    {
        let first_tag = if let Some(first_tag) = first_tag {
            first_tag
        } else {
            self.parse_jinja_tag()?
        };
        let tag_name = parse_jinja_tag_name(&first_tag);

        // Django block tags: tags that have matching {% end<tag> %} closers.
        //
        // Changes from upstream markup_fmt (Jinja mode):
        // - Removed: "trans" (self-closing in Django, block in Jinja)
        // - Added: "blocktrans", "blocktranslate", "verbatim", "spaceless",
        //   "cache", "ifchanged", "comment"
        if matches!(
            tag_name,
            "for"
                | "if"
                | "macro"
                | "call"
                | "filter"
                | "block"
                | "apply"
                | "autoescape"
                | "embed"
                | "with"
                | "raw"
                | "blocktrans"
                | "blocktranslate"
                | "verbatim"
                | "spaceless"
                | "cache"
                | "ifchanged"
                | "comment"
        ) || tag_name == "set" && !first_tag.content.contains('=')
        {
            let mut body = vec![JinjaTagOrChildren::Tag(first_tag)];

            loop {
                let mut children = self.parse_jinja_block_children(children_parser)?;
                if !children.is_empty() {
                    if let Some(JinjaTagOrChildren::Children(nodes)) = body.last_mut() {
                        nodes.append(&mut children);
                    } else {
                        body.push(JinjaTagOrChildren::Children(children));
                    }
                }
                if let Ok(next_tag) = self.parse_jinja_tag() {
                    let next_tag_name = parse_jinja_tag_name(&next_tag);
                    if next_tag_name
                        .strip_prefix("end")
                        .is_some_and(|name| name == tag_name)
                    {
                        body.push(JinjaTagOrChildren::Tag(next_tag));
                        break;
                    }
                    // Intermediate tags: elif/elseif/else for if/for,
                    // "empty" for Django's {% for %}...{% empty %}...{% endfor %}
                    if (tag_name == "if" || tag_name == "for")
                        && matches!(next_tag_name, "elif" | "elseif" | "else" | "empty")
                    {
                        body.push(JinjaTagOrChildren::Tag(next_tag));
                    } else if let Some(JinjaTagOrChildren::Children(nodes)) = body.last_mut() {
                        nodes.push(
                            self.with_taken(|parser| {
                                parser.parse_jinja_tag_or_block(Some(next_tag), children_parser)
                            })
                            .map(|(kind, raw)| T::build(kind, raw))?,
                        );
                    } else {
                        body.push(JinjaTagOrChildren::Children(vec![self
                            .with_taken(|parser| {
                                parser.parse_jinja_tag_or_block(Some(next_tag), children_parser)
                            })
                            .map(|(kind, raw)| T::build(kind, raw))?]));
                    }
                } else {
                    break;
                }
            }
            Ok(T::from_block(JinjaBlock { body }))
        } else {
            Ok(T::from_tag(first_tag))
        }
    }

    fn parse_mustache_interpolation(&mut self) -> PResult<(&'s str, usize)> {
        let Some((start, _)) = self.try_consume_str("{{") else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectJinjaTag));
        };
        let start = start + 1;

        let mut braces_stack = 0usize;
        let end;
        loop {
            match self.chars.next() {
                Some((_, '{')) => braces_stack += 1,
                Some((i, '}')) => {
                    if braces_stack == 0 {
                        if self.chars.next_if(|(_, c)| *c == '}').is_some() {
                            end = i;
                            break;
                        }
                    } else {
                        braces_stack -= 1;
                    }
                }
                Some(..) => continue,
                None => {
                    end = self.source.len();
                    break;
                }
            }
        }

        Ok((unsafe { self.source.get_unchecked(start..end) }, start))
    }

    fn parse_native_attr(&mut self) -> PResult<NativeAttribute<'s>> {
        let name = self.parse_attr_name()?;
        self.skip_ws();
        let mut quote = None;
        let value = if self.chars.next_if(|(_, c)| *c == '=').is_some() {
            self.skip_ws();
            quote = self
                .chars
                .peek()
                .and_then(|(_, c)| (*c == '\'' || *c == '"').then_some(*c));
            Some(self.parse_attr_value()?)
        } else {
            None
        };
        Ok(NativeAttribute { name, value, quote })
    }

    fn parse_node(&mut self) -> PResult<Node<'s>> {
        let (kind, raw) = self.with_taken(Parser::parse_node_kind)?;
        Ok(Node { kind, raw })
    }

    fn parse_node_kind(&mut self) -> PResult<NodeKind<'s>> {
        match self.chars.peek() {
            Some((_, '<')) => {
                let mut chars = self.chars.clone();
                chars.next();
                match chars.next() {
                    Some((_, c))
                        if is_html_tag_name_char(c)
                            || is_special_tag_name_char(c, self.language) =>
                    {
                        self.parse_element().map(NodeKind::Element)
                    }
                    Some((_, '!')) => self
                        .try_parse(Parser::parse_comment)
                        .map(NodeKind::Comment)
                        .or_else(|_| self.try_parse(Parser::parse_doctype).map(NodeKind::Doctype))
                        .or_else(|_| self.try_parse(Parser::parse_cdata).map(NodeKind::Cdata))
                        .or_else(|_| self.parse_text_node().map(NodeKind::Text)),
                    Some((_, '?')) if self.language == Language::Xml => {
                        self.parse_xml_decl().map(NodeKind::XmlDecl)
                    }
                    _ => self.parse_text_node().map(NodeKind::Text),
                }
            }
            Some((_, '{')) => {
                let mut chars = self.chars.clone();
                chars.next();
                match chars.next() {
                    Some((_, '{')) => match self.language {
                        Language::Html | Language::Xml => {
                            self.parse_text_node().map(NodeKind::Text)
                        }
                        Language::Jinja => {
                            self.parse_mustache_interpolation().map(|(expr, start)| {
                                let (trim_prev, expr) = if let Some(rest) = expr.strip_prefix('-') {
                                    (true, rest)
                                } else {
                                    (false, expr)
                                };
                                let (trim_next, expr) = if let Some(rest) = expr.strip_suffix('-') {
                                    (true, rest)
                                } else {
                                    (false, expr)
                                };
                                NodeKind::JinjaInterpolation(JinjaInterpolation {
                                    expr,
                                    start: if trim_prev { start + 1 } else { start },
                                    trim_prev,
                                    trim_next,
                                })
                            })
                        }
                    },
                    Some((_, '#')) if matches!(self.language, Language::Jinja) => {
                        self.parse_jinja_comment().map(NodeKind::JinjaComment)
                    }
                    Some((_, '%')) if matches!(self.language, Language::Jinja) => {
                        self.parse_jinja_tag_or_block(None, &mut Parser::parse_node)
                    }
                    _ => self.parse_text_node().map(NodeKind::Text),
                }
            }
            Some(..) => self.parse_text_node().map(NodeKind::Text),
            None => Err(self.emit_error(SyntaxErrorKind::ExpectElement)),
        }
    }

    fn parse_raw_text_node(&mut self, tag_name: &str) -> PResult<TextNode<'s>> {
        let start = self.peek_pos();

        let allow_nested = tag_name.eq_ignore_ascii_case("pre");
        let mut nested = 0u16;
        let mut line_breaks = 0;
        let end;
        loop {
            match self.chars.peek() {
                Some((i, '<')) => {
                    let i = *i;
                    let mut chars = self.chars.clone();
                    chars.next();
                    if chars.next_if(|(_, c)| *c == '/').is_some()
                        && chars
                            .by_ref()
                            .zip(tag_name.chars())
                            .all(|((_, a), b)| a.eq_ignore_ascii_case(&b))
                    {
                        if nested == 0 {
                            end = i;
                            break;
                        } else {
                            nested -= 1;
                            self.chars = chars;
                            continue;
                        }
                    } else if allow_nested
                        && chars
                            .by_ref()
                            .zip(tag_name.chars())
                            .all(|((_, a), b)| a.eq_ignore_ascii_case(&b))
                    {
                        nested += 1;
                        self.chars = chars;
                        continue;
                    }
                    self.chars.next();
                }
                Some((_, c)) => {
                    if *c == '\n' {
                        line_breaks += 1;
                    }
                    self.chars.next();
                }
                None => {
                    end = self.source.len();
                    break;
                }
            }
        }

        Ok(TextNode {
            raw: unsafe { self.source.get_unchecked(start..end) },
            line_breaks,
            start,
        })
    }

    pub fn parse_root(&mut self) -> PResult<Root<'s>> {
        let mut children = vec![];
        while self.chars.peek().is_some() {
            children.push(self.parse_node()?);
        }

        Ok(Root { children })
    }

    fn parse_tag_name(&mut self) -> PResult<&'s str> {
        let (start, mut end) = match self.chars.peek() {
            Some((i, c)) if is_html_tag_name_char(*c) => {
                let c = *c;
                let start = *i;
                self.chars.next();
                (start, start + c.len_utf8())
            }
            Some((i, '{')) if matches!(self.language, Language::Jinja) => (*i, *i + 1),
            _ => return Err(self.emit_error(SyntaxErrorKind::ExpectTagName)),
        };

        while let Some((i, c)) = self.chars.peek() {
            if is_html_tag_name_char(*c) {
                end = *i + c.len_utf8();
                self.chars.next();
            } else if *c == '{' && matches!(self.language, Language::Jinja) {
                let current_i = *i;
                let mut chars = self.chars.clone();
                chars.next();
                if chars.next_if(|(_, c)| *c == '{').is_some() {
                    end = current_i + self.parse_mustache_interpolation()?.0.len() + "{{}}".len();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        unsafe { Ok(self.source.get_unchecked(start..end)) }
    }

    fn parse_text_node(&mut self) -> PResult<TextNode<'s>> {
        let Some((start, first_char)) = self.chars.next() else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectTextNode));
        };

        let mut line_breaks = if first_char == '\n' { 1 } else { 0 };
        let end;
        loop {
            match self.chars.peek() {
                Some((i, '{')) => match self.language {
                    Language::Html | Language::Xml => {
                        self.chars.next();
                    }
                    Language::Jinja => {
                        let i = *i;
                        let mut chars = self.chars.clone();
                        chars.next();
                        if chars
                            .next_if(|(_, c)| *c == '%' || *c == '{' || *c == '#')
                            .is_some()
                        {
                            end = i;
                            break;
                        }
                        self.chars.next();
                    }
                },
                Some((i, '<')) => {
                    let i = *i;
                    let mut chars = self.chars.clone();
                    chars.next();
                    match chars.next() {
                        Some((_, c))
                            if is_html_tag_name_char(c)
                                || is_special_tag_name_char(c, self.language)
                                || c == '/'
                                || c == '!' =>
                        {
                            end = i;
                            break;
                        }
                        _ => {
                            self.chars.next();
                        }
                    }
                }
                Some((_, c)) => {
                    if *c == '\n' {
                        line_breaks += 1;
                    }
                    self.chars.next();
                }
                None => {
                    end = self.source.len();
                    break;
                }
            }
        }

        Ok(TextNode {
            raw: unsafe { self.source.get_unchecked(start..end) },
            line_breaks,
            start,
        })
    }

    fn parse_xml_decl(&mut self) -> PResult<XmlDecl<'s>> {
        if self
            .try_consume_str("<?xml")
            .and_then(|_| self.chars.next_if(|(_, c)| c.is_ascii_whitespace()))
            .is_none()
        {
            return Err(self.emit_error(SyntaxErrorKind::ExpectXmlDecl));
        };

        let mut attrs = vec![];
        loop {
            match self.chars.peek() {
                Some((_, '?')) => {
                    self.chars.next();
                    if self.chars.next_if(|(_, c)| *c == '>').is_some() {
                        break;
                    }
                    return Err(self.emit_error(SyntaxErrorKind::ExpectChar('>')));
                }
                Some((_, c)) if c.is_ascii_whitespace() => {
                    self.chars.next();
                }
                _ => {
                    attrs.push(self.parse_native_attr()?);
                }
            }
        }
        Ok(XmlDecl { attrs })
    }
}

fn is_html_tag_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c == '-'
        || c == '_'
        || c == '.'
        || c == ':'
        || !c.is_ascii()
        || c == '\\'
}

fn is_special_tag_name_char(c: char, language: Language) -> bool {
    match language {
        Language::Jinja => c == '{',
        Language::Html | Language::Xml => false,
    }
}

fn is_attr_name_char(c: char) -> bool {
    !matches!(c, '"' | '\'' | '>' | '/' | '=') && !c.is_ascii_whitespace()
}

fn parse_jinja_tag_name<'s>(tag: &JinjaTag<'s>) -> &'s str {
    let trimmed = tag.content.trim_start_matches(['+', '-']).trim_start();
    trimmed
        .split_once(|c: char| c.is_ascii_whitespace())
        .map(|(name, _)| name)
        .unwrap_or(trimmed)
}

pub type PResult<T> = Result<T, SyntaxError>;

trait HasJinjaFlowControl<'s>: Sized {
    type Intermediate;

    fn build(intermediate: Self::Intermediate, raw: &'s str) -> Self;
    fn from_tag(tag: JinjaTag<'s>) -> Self::Intermediate;
    fn from_block(block: JinjaBlock<'s, Self>) -> Self::Intermediate;
}

impl<'s> HasJinjaFlowControl<'s> for Node<'s> {
    type Intermediate = NodeKind<'s>;

    fn build(intermediate: Self::Intermediate, raw: &'s str) -> Self {
        Node {
            kind: intermediate,
            raw,
        }
    }

    fn from_tag(tag: JinjaTag<'s>) -> Self::Intermediate {
        NodeKind::JinjaTag(tag)
    }

    fn from_block(block: JinjaBlock<'s, Self>) -> Self::Intermediate {
        NodeKind::JinjaBlock(block)
    }
}

impl<'s> HasJinjaFlowControl<'s> for Attribute<'s> {
    type Intermediate = Attribute<'s>;

    fn build(intermediate: Self::Intermediate, _: &'s str) -> Self {
        intermediate
    }

    fn from_tag(tag: JinjaTag<'s>) -> Self::Intermediate {
        Attribute::JinjaTag(tag)
    }

    fn from_block(block: JinjaBlock<'s, Self>) -> Self::Intermediate {
        Attribute::JinjaBlock(block)
    }
}

pub fn parse_as_interpolated(
    text: &'_ str,
    base_start: usize,
    language: Language,
    _attr: bool,
) -> (Vec<&'_ str>, Vec<(&'_ str, usize)>) {
    let mut statics = Vec::with_capacity(1);
    let mut dynamics = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut pos = 0;
    let mut brace_stack = 0u8;
    while let Some((i, c)) = chars.next() {
        match c {
            '{' => {
                if brace_stack > 0 {
                    brace_stack += 1;
                    continue;
                }
                match language {
                    Language::Jinja => {
                        if chars.next_if(|(_, c)| *c == '{').is_some() {
                            statics.push(unsafe { text.get_unchecked(pos..i) });
                            pos = i;
                            brace_stack += 1;
                        }
                    }
                    Language::Html | Language::Xml => {}
                }
            }
            '}' => {
                if brace_stack > 1 {
                    brace_stack -= 1;
                    continue;
                }
                match language {
                    Language::Jinja => {
                        if chars.next_if(|(_, c)| *c == '}').is_some() {
                            dynamics.push((
                                unsafe { text.get_unchecked(pos + 2..i) },
                                base_start + pos + 2,
                            ));
                            pos = i + 2;
                            brace_stack = 0;
                        }
                    }
                    Language::Html | Language::Xml => {}
                }
            }
            _ => {}
        }
    }
    statics.push(unsafe { text.get_unchecked(pos..) });
    (statics, dynamics)
}
