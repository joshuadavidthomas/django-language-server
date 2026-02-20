// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

use std::borrow::Cow;

use itertools::Itertools;
use tiny_pretty::Doc;

use crate::markup::ast::*;
use crate::markup::config::ClosingTagLineBreakForEmpty;
use crate::markup::config::DoctypeKeywordCase;
use crate::markup::config::Quotes;
use crate::markup::config::ScriptFormatter;
use crate::markup::config::WhitespaceSensitivity;
use crate::markup::ctx::Ctx;
use crate::markup::ctx::Hints;
use crate::markup::helpers;
use crate::markup::parser::parse_as_interpolated;
use crate::markup::state::State;
use crate::markup::Language;

pub(super) trait DocGen<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>;
}

impl<'s> DocGen<'s> for Attribute<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        match self {
            Attribute::Native(native_attribute) => native_attribute.doc(ctx, state),
            Attribute::JinjaBlock(jinja_block) => jinja_block.doc(ctx, state),
            Attribute::JinjaComment(jinja_comment) => jinja_comment.doc(ctx, state),
            Attribute::JinjaTag(jinja_tag) => jinja_tag.doc(ctx, state),
        }
    }
}

impl<'s> DocGen<'s> for Cdata<'s> {
    fn doc<E, F>(&self, _: &mut Ctx<'s, E, F>, _: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::text("<![CDATA[")
            .concat(reflow_raw(self.raw))
            .append(Doc::text("]]>"))
    }
}

impl<'s> DocGen<'s> for Comment<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, _: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        if ctx.options.format_comments {
            Doc::text("<!--")
                .append(Doc::line_or_space())
                .concat(reflow_with_indent(self.raw.trim(), true))
                .nest(ctx.indent_width)
                .append(Doc::line_or_space())
                .append(Doc::text("-->"))
                .group()
        } else {
            Doc::text("<!--")
                .concat(reflow_raw(self.raw))
                .append(Doc::text("-->"))
        }
    }
}

impl<'s> DocGen<'s> for Doctype<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, _: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::text("<!")
            .append(match ctx.options.doctype_keyword_case {
                DoctypeKeywordCase::Ignore => Doc::text(self.keyword),
                DoctypeKeywordCase::Upper => Doc::text("DOCTYPE"),
                DoctypeKeywordCase::Lower => Doc::text("doctype"),
            })
            .append(Doc::space())
            .append(Doc::text(if self.value.eq_ignore_ascii_case("html") {
                "html"
            } else {
                self.value
            }))
            .append(Doc::text(">"))
    }
}

impl<'s> DocGen<'s> for Element<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        let tag_name = self
            .tag_name
            .split_once(':')
            .and_then(|(namespace, name)| namespace.eq_ignore_ascii_case("html").then_some(name))
            .unwrap_or(self.tag_name);
        let formatted_tag_name = match ctx.language {
            Language::Html | Language::Jinja
                if css_dataset::tags::STANDARD_HTML_TAGS
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(self.tag_name)) =>
            {
                Cow::from(self.tag_name.to_ascii_lowercase())
            }
            _ => Cow::from(self.tag_name),
        };
        let mut state = State {
            current_tag_name: Some(tag_name),
            is_root: false,
            in_svg: tag_name.eq_ignore_ascii_case("svg"),
            indent_level: state.indent_level,
        };

        let self_closing = if helpers::is_void_element(tag_name, ctx.language) {
            ctx.options
                .html_void_self_closing
                .unwrap_or(self.self_closing)
        } else if helpers::is_html_tag(tag_name, ctx.language) {
            ctx.options
                .html_normal_self_closing
                .unwrap_or(self.self_closing)
        } else if helpers::is_svg_tag(self.tag_name, ctx.language) {
            ctx.options.svg_self_closing.unwrap_or(self.self_closing)
        } else if helpers::is_mathml_tag(self.tag_name, ctx.language) {
            ctx.options.mathml_self_closing.unwrap_or(self.self_closing)
        } else {
            self.self_closing
        };
        let is_whitespace_sensitive = !state.in_svg && ctx.is_whitespace_sensitive(tag_name);
        let is_empty = is_empty_element(&self.children, is_whitespace_sensitive);

        let mut docs = Vec::with_capacity(5);

        docs.push(Doc::text("<"));
        docs.push(Doc::text(formatted_tag_name.clone()));

        match &*self.attrs {
            [] => {
                if self_closing && (self.void_element || is_empty) {
                    docs.push(Doc::text(" />"));
                    return Doc::list(docs).group();
                }
                if self.void_element {
                    docs.push(Doc::text(">"));
                    return Doc::list(docs).group();
                }
                if is_empty || !is_whitespace_sensitive {
                    docs.push(Doc::text(">"));
                } else {
                    docs.push(Doc::line_or_nil().append(Doc::text(">")).group());
                }
            }
            [attr]
                if ctx.options.single_attr_same_line
                    && !is_whitespace_sensitive
                    && !is_multi_line_attr(attr) =>
            {
                docs.push(Doc::space());
                docs.push(attr.doc(ctx, &state));
                if self_closing && is_empty {
                    docs.push(Doc::text(" />"));
                    return Doc::list(docs);
                } else {
                    docs.push(Doc::text(">"));
                };
                if self.void_element {
                    return Doc::list(docs);
                }
            }
            _ => {
                let attrs_sep = if self.first_attr_same_line {
                    Doc::line_or_space()
                } else if self.attrs.len() <= 1 {
                    if ctx.options.single_attr_same_line {
                        Doc::line_or_space()
                    } else {
                        Doc::hard_line()
                    }
                } else if !ctx.options.prefer_attrs_single_line
                    && ctx
                        .options
                        .max_attrs_per_line
                        .is_none_or(|value| value.get() <= 1)
                {
                    Doc::hard_line()
                } else {
                    Doc::line_or_space()
                };
                let attrs = if let Some(max) = ctx.options.max_attrs_per_line {
                    Doc::line_or_space()
                        .concat(itertools::intersperse(
                            self.attrs.chunks(max.into()).map(|chunk| {
                                Doc::list(
                                    itertools::intersperse(
                                        chunk.iter().map(|attr| attr.doc(ctx, &state)),
                                        attrs_sep.clone(),
                                    )
                                    .collect(),
                                )
                                .group()
                            }),
                            Doc::hard_line(),
                        ))
                        .nest(ctx.indent_width)
                } else {
                    Doc::list(
                        self.attrs
                            .iter()
                            .flat_map(|attr| [attrs_sep.clone(), attr.doc(ctx, &state)].into_iter())
                            .collect(),
                    )
                    .nest(ctx.indent_width)
                };

                if self_closing && (self.void_element || is_empty) {
                    docs.push(attrs);
                    docs.push(Doc::line_or_space());
                    docs.push(Doc::text("/>"));
                    return Doc::list(docs).group();
                }
                if self.void_element {
                    docs.push(attrs);
                    if !ctx.options.closing_bracket_same_line {
                        docs.push(Doc::line_or_nil());
                    }
                    docs.push(Doc::text(">"));
                    return Doc::list(docs).group();
                }
                if ctx.options.closing_bracket_same_line {
                    docs.push(attrs.append(Doc::text(">")).group());
                } else {
                    if is_whitespace_sensitive
                        && self.children.first().is_some_and(|child| {
                            if let NodeKind::Text(text_node) = &child.kind {
                                !text_node.raw.starts_with(|c: char| c.is_ascii_whitespace())
                            } else {
                                false
                            }
                        })
                        && self.children.last().is_some_and(|child| {
                            if let NodeKind::Text(text_node) = &child.kind {
                                !text_node.raw.ends_with(|c: char| c.is_ascii_whitespace())
                            } else {
                                false
                            }
                        })
                    {
                        docs.push(
                            attrs
                                .group()
                                .append(Doc::line_or_nil())
                                .append(Doc::text(">")),
                        );
                    } else {
                        docs.push(
                            attrs
                                .append(Doc::line_or_nil())
                                .append(Doc::text(">"))
                                .group(),
                        );
                    }
                }
            }
        }

        let has_two_more_non_text_children =
            has_two_more_non_text_children(&self.children, ctx.language);

        let (leading_ws, trailing_ws) = if is_empty
            || ctx.language == Language::Xml
                && matches!(
                    &*self.children,
                    [Node {
                        kind: NodeKind::Text(..),
                        ..
                    }]
                ) {
            (Doc::nil(), Doc::nil())
        } else if is_whitespace_sensitive {
            (
                format_ws_sensitive_leading_ws(&self.children),
                format_ws_sensitive_trailing_ws(&self.children),
            )
        } else if has_two_more_non_text_children {
            (Doc::hard_line(), Doc::hard_line())
        } else {
            (
                format_ws_insensitive_leading_ws(&self.children),
                format_ws_insensitive_trailing_ws(&self.children),
            )
        };

        if tag_name.eq_ignore_ascii_case("script") && ctx.language != Language::Xml {
            if let [Node {
                kind: NodeKind::Text(text_node),
                ..
            }] = &*self.children
            {
                if text_node.raw.chars().all(|c| c.is_ascii_whitespace()) {
                    docs.push(Doc::hard_line());
                } else {
                    let type_attr = self.attrs.iter().find_map(|attr| match attr {
                        Attribute::Native(native) if native.name.eq_ignore_ascii_case("type") => {
                            native.value.map(|(value, _)| value.to_ascii_lowercase())
                        }
                        _ => None,
                    });
                    match type_attr.as_deref() {
                        Some(
                            "module"
                            | "application/javascript"
                            | "text/javascript"
                            | "application/ecmascript"
                            | "text/ecmascript"
                            | "application/x-javascript"
                            | "application/x-ecmascript"
                            | "text/x-javascript"
                            | "text/x-ecmascript"
                            | "text/jsx"
                            | "text/babel",
                        )
                        | None => {
                            let is_script_indent = ctx.script_indent();
                            if is_script_indent {
                                state.indent_level += 1;
                            }
                            let lang = self
                                .attrs
                                .iter()
                                .find_map(|attr| match attr {
                                    Attribute::Native(native)
                                        if native.name.eq_ignore_ascii_case("lang") =>
                                    {
                                        native.value.map(|(value, _)| value)
                                    }
                                    _ => None,
                                })
                                .unwrap_or("js");
                            let lang = if self.attrs.iter().any(|attr| match attr {
                                Attribute::Native(native)
                                    if native.name.eq_ignore_ascii_case("type") =>
                                {
                                    native.value.is_some_and(|(value, _)| value == "module")
                                }
                                _ => false,
                            }) {
                                match lang {
                                    "ts" => "mts",
                                    "js" => "mjs",
                                    lang => lang,
                                }
                            } else {
                                lang
                            };
                            let formatted =
                                ctx.format_script(text_node.raw, lang, text_node.start, &state);
                            let doc = if matches!(
                                ctx.options.script_formatter,
                                Some(ScriptFormatter::Dprint)
                            ) {
                                Doc::hard_line().concat(reflow_owned(formatted.trim()))
                            } else {
                                Doc::hard_line().concat(reflow_with_indent(formatted.trim(), true))
                            };
                            if is_script_indent {
                                docs.push(doc.nest(ctx.indent_width));
                            } else {
                                docs.push(doc);
                            }
                        }
                        Some(
                            "importmap"
                            | "application/json"
                            | "text/json"
                            | "application/ld+json"
                            | "speculationrules",
                        ) => {
                            let formatted = ctx.format_json(text_node.raw, text_node.start, &state);
                            docs.push(
                                Doc::hard_line().concat(reflow_with_indent(formatted.trim(), true)),
                            );
                        }
                        Some(..) => {
                            docs.push(Doc::hard_line());
                            docs.extend(reflow_raw(text_node.raw.trim_matches('\n')));
                        }
                    }
                    docs.push(Doc::hard_line());
                }
            }
        } else if tag_name.eq_ignore_ascii_case("style") && ctx.language != Language::Xml {
            if let [Node {
                kind: NodeKind::Text(text_node),
                ..
            }] = &*self.children
            {
                if text_node.raw.chars().all(|c| c.is_ascii_whitespace()) {
                    docs.push(Doc::hard_line());
                } else {
                    let lang = self
                        .attrs
                        .iter()
                        .find_map(|attr| match attr {
                            Attribute::Native(native_attribute)
                                if native_attribute.name.eq_ignore_ascii_case("lang") =>
                            {
                                native_attribute.value.map(|(value, _)| value)
                            }
                            _ => None,
                        })
                        .unwrap_or("css");
                    let (statics, dynamics) =
                        parse_as_interpolated(text_node.raw, text_node.start, ctx.language, false);
                    const PLACEHOLDER: &str = "_saya0909_";
                    let masked = statics.join(PLACEHOLDER);
                    let formatted = ctx.format_style(&masked, lang, text_node.start, &state);
                    let doc = Doc::hard_line().concat(reflow_with_indent(
                        formatted
                            .split(PLACEHOLDER)
                            .map(Cow::from)
                            .interleave(dynamics.iter().map(|(expr, start)| match ctx.language {
                                Language::Jinja => Cow::from(format!(
                                    "{{{{ {} }}}}",
                                    ctx.format_jinja(expr, *start, true, &state),
                                )),
                                Language::Html | Language::Xml => unreachable!(),
                            }))
                            .collect::<String>()
                            .trim(),
                        lang != "sass",
                    ));
                    docs.push(
                        if ctx.style_indent() {
                            doc.nest(ctx.indent_width)
                        } else {
                            doc
                        }
                        .append(Doc::hard_line()),
                    );
                }
            }
        } else if tag_name.eq_ignore_ascii_case("pre") || tag_name.eq_ignore_ascii_case("textarea")
        {
            if let [Node {
                kind: NodeKind::Text(text_node),
                ..
            }] = &self.children[..]
            {
                docs.extend(reflow_raw(text_node.raw));
            }
        } else if is_empty {
            if !is_whitespace_sensitive {
                match ctx.options.closing_tag_line_break_for_empty {
                    ClosingTagLineBreakForEmpty::Always => docs.push(Doc::hard_line()),
                    ClosingTagLineBreakForEmpty::Fit => docs.push(Doc::line_or_nil()),
                    ClosingTagLineBreakForEmpty::Never => {}
                };
            }
        } else if !is_whitespace_sensitive && has_two_more_non_text_children {
            state.indent_level += 1;
            docs.push(leading_ws.nest(ctx.indent_width));
            docs.push(
                format_children_with_inserting_linebreak(&self.children, ctx, &state)
                    .nest(ctx.indent_width),
            );
            docs.push(trailing_ws);
        } else if is_whitespace_sensitive
            && matches!(&self.children[..], [Node { kind: NodeKind::Text(text_node), .. }] if is_all_ascii_whitespace(text_node.raw))
        {
            docs.push(Doc::line_or_space());
        } else {
            let should_not_indent = is_whitespace_sensitive
                && self.children.iter().all(|child| {
                    matches!(
                        &child.kind,
                        NodeKind::Comment(..) | NodeKind::JinjaInterpolation(..)
                    )
                });
            if !should_not_indent {
                state.indent_level += 1;
            }
            let children_doc = leading_ws.append(format_children_without_inserting_linebreak(
                &self.children,
                ctx,
                &state,
            ));
            if should_not_indent {
                docs.push(children_doc);
            } else {
                docs.push(children_doc.nest(ctx.indent_width));
            }
            docs.push(trailing_ws);
        }

        docs.push(Doc::text(format!("</{formatted_tag_name}>")));

        Doc::list(docs).group()
    }
}

impl<'s> DocGen<'s> for JinjaBlock<'s, Attribute<'s>> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::list(
            self.body
                .iter()
                .map(|child| match child {
                    JinjaTagOrChildren::Tag(tag) => tag.doc(ctx, state),
                    JinjaTagOrChildren::Children(children) => Doc::line_or_nil()
                        .concat(itertools::intersperse(
                            children.iter().map(|attr| attr.doc(ctx, state)),
                            Doc::line_or_space(),
                        ))
                        .nest(ctx.indent_width)
                        .append(Doc::line_or_nil()),
                })
                .collect(),
        )
    }
}

impl<'s> DocGen<'s> for JinjaBlock<'s, Node<'s>> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::list(
            self.body
                .iter()
                .map(|child| match child {
                    JinjaTagOrChildren::Tag(tag) => tag.doc(ctx, state),
                    JinjaTagOrChildren::Children(children) => {
                        format_control_structure_block_children(children, ctx, state)
                    }
                })
                .collect(),
        )
    }
}

impl<'s> DocGen<'s> for JinjaComment<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, _: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        if ctx.options.format_comments {
            Doc::text("{#")
                .append(Doc::line_or_space())
                .concat(reflow_with_indent(self.raw.trim(), true))
                .nest(ctx.indent_width)
                .append(Doc::line_or_space())
                .append(Doc::text("#}"))
                .group()
        } else {
            Doc::text("{#")
                .concat(reflow_raw(self.raw))
                .append(Doc::text("#}"))
        }
    }
}

impl<'s> DocGen<'s> for JinjaInterpolation<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::text("{{")
            .append(if self.trim_prev {
                Doc::text("-")
            } else {
                Doc::nil()
            })
            .append(Doc::line_or_space())
            .concat(reflow_with_indent(
                ctx.format_jinja(self.expr, self.start, true, state).trim(),
                true,
            ))
            .nest(ctx.indent_width)
            .append(Doc::line_or_space())
            .append(if self.trim_next {
                Doc::text("-")
            } else {
                Doc::nil()
            })
            .append(Doc::text("}}"))
            .group()
    }
}

impl<'s> DocGen<'s> for JinjaTag<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        let (prefix, content) = if let Some(content) = self.content.strip_prefix('-') {
            ("-", content)
        } else if let Some(content) = self.content.strip_prefix('+') {
            ("+", content)
        } else {
            ("", self.content)
        };
        let (content, suffix) = if let Some(content) = content.strip_suffix('-') {
            (content, "-")
        } else if let Some(content) = content.strip_suffix('+') {
            (content, "+")
        } else {
            (content, "")
        };

        let mut docs = Vec::with_capacity(5);
        docs.push(Doc::text("{%"));
        docs.push(Doc::text(prefix));
        docs.push(Doc::line_or_space());
        docs.extend(reflow_with_indent(
            ctx.format_jinja(content, self.start + prefix.len(), false, state)
                .trim(),
            true,
        ));
        Doc::list(docs)
            .nest(ctx.indent_width)
            .append(Doc::line_or_space())
            .append(Doc::text(suffix))
            .append(Doc::text("%}"))
            .group()
    }
}

impl<'s> DocGen<'s> for NativeAttribute<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, _state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        let name = Doc::text(self.name);
        if let Some((value, _value_start)) = self.value {
            let value = if !matches!(ctx.language, Language::Xml) && self.name.starts_with("on") {
                // For event handlers, pass through without formatting
                Cow::from(value)
            } else {
                Cow::from(value)
            };
            let quote;
            let mut docs = Vec::with_capacity(5);
            docs.push(name);
            docs.push(Doc::text("="));
            if self.name.eq_ignore_ascii_case("class") {
                quote = compute_attr_value_quote(&value, self.quote, ctx);
                let value = value.trim();
                let maybe_line_break = if value.contains('\n') {
                    Doc::hard_line()
                } else {
                    Doc::nil()
                };
                docs.push(
                    maybe_line_break
                        .clone()
                        .concat(itertools::intersperse(
                            value
                                .trim()
                                .lines()
                                .filter(|line| !line.is_empty())
                                .map(|line| Doc::text(line.split_ascii_whitespace().join(" "))),
                            Doc::hard_line(),
                        ))
                        .nest(ctx.indent_width),
                );
                docs.push(maybe_line_break);
            } else if self.name.eq_ignore_ascii_case("style") {
                let (statics, dynamics) =
                    parse_as_interpolated(&value, _value_start, ctx.language, true);
                const PLACEHOLDER: &str = "_mnk0430_";
                let formatted =
                    ctx.format_style_attr(&statics.join(PLACEHOLDER), _value_start, _state);
                quote = compute_attr_value_quote(&formatted, self.quote, ctx);
                docs.push(Doc::text(
                    formatted
                        .split(PLACEHOLDER)
                        .map(Cow::from)
                        .interleave(dynamics.iter().map(|(expr, start)| match ctx.language {
                            Language::Jinja => Cow::from(format!(
                                "{{{{ {} }}}}",
                                ctx.format_jinja(expr, *start, true, _state),
                            )),
                            Language::Html | Language::Xml => unreachable!(),
                        }))
                        .collect::<String>(),
                ));
            } else if self.name.eq_ignore_ascii_case("accept")
                && !matches!(ctx.language, Language::Xml)
                && _state
                    .current_tag_name
                    .is_some_and(|name| name.eq_ignore_ascii_case("input"))
            {
                quote = compute_attr_value_quote(&value, self.quote, ctx);
                if helpers::has_template_interpolation(&value, ctx.language) {
                    docs.extend(reflow_owned(&value));
                } else {
                    docs.push(Doc::text(
                        value
                            .split(',')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .join(", "),
                    ));
                }
            } else {
                quote = compute_attr_value_quote(&value, self.quote, ctx);
                docs.extend(reflow_owned(&value));
            }
            docs.insert(2, quote.clone());
            docs.push(quote);
            Doc::list(docs)
        } else {
            name
        }
    }
}

impl<'s> DocGen<'s> for NodeKind<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        match self {
            NodeKind::Cdata(cdata) => cdata.doc(ctx, state),
            NodeKind::Comment(comment) => comment.doc(ctx, state),
            NodeKind::Doctype(doctype) => doctype.doc(ctx, state),
            NodeKind::Element(element) => element.doc(ctx, state),
            NodeKind::JinjaBlock(jinja_block) => jinja_block.doc(ctx, state),
            NodeKind::JinjaComment(jinja_comment) => jinja_comment.doc(ctx, state),
            NodeKind::JinjaInterpolation(jinja_interpolation) => {
                jinja_interpolation.doc(ctx, state)
            }
            NodeKind::JinjaTag(jinja_tag) => jinja_tag.doc(ctx, state),
            NodeKind::Text(text_node) => text_node.doc(ctx, state),
            NodeKind::XmlDecl(xml_decl) => xml_decl.doc(ctx, state),
        }
    }
}

impl<'s> DocGen<'s> for Root<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        let is_whole_document_like = self.children.iter().any(|child| match &child.kind {
            NodeKind::Doctype(..) => true,
            NodeKind::Element(element) => element.tag_name.eq_ignore_ascii_case("html"),
            _ => false,
        });
        let is_whitespace_sensitive = matches!(
            ctx.options.whitespace_sensitivity,
            WhitespaceSensitivity::Css | WhitespaceSensitivity::Strict
        );
        let has_two_more_non_text_children =
            has_two_more_non_text_children(&self.children, ctx.language);

        if is_whole_document_like
            && !matches!(
                ctx.options.whitespace_sensitivity,
                WhitespaceSensitivity::Strict
            )
            || !is_whitespace_sensitive && has_two_more_non_text_children
            || ctx.language == Language::Xml
        {
            format_children_with_inserting_linebreak(&self.children, ctx, state)
                .append(Doc::hard_line())
        } else {
            format_children_without_inserting_linebreak(&self.children, ctx, state)
                .append(Doc::hard_line())
        }
    }
}

impl<'s> DocGen<'s> for TextNode<'s> {
    fn doc<E, F>(&self, _: &mut Ctx<'s, E, F>, _: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        let trimmed = self
            .raw
            .trim_start_matches(|c: char| c.is_ascii_whitespace())
            .trim_end_matches(|c: char| c.is_ascii_whitespace());
        Doc::list(
            itertools::intersperse(
                trimmed
                    .split('\n')
                    .map(|s| s.strip_suffix('\r').unwrap_or(s))
                    .enumerate()
                    .map(|(i, s)| {
                        let s = s.trim_matches(|c: char| c.is_ascii_whitespace());
                        if i == 0 || !s.is_empty() {
                            Doc::text(s.to_owned())
                        } else {
                            Doc::nil()
                        }
                    }),
                Doc::soft_line(),
            )
            .collect(),
        )
    }
}

impl<'s> DocGen<'s> for XmlDecl<'s> {
    fn doc<E, F>(&self, ctx: &mut Ctx<'s, E, F>, state: &State<'s>) -> Doc<'s>
    where
        F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
    {
        Doc::text("<?xml")
            .concat(
                self.attrs
                    .iter()
                    .flat_map(|attr| [Doc::line_or_space(), attr.doc(ctx, state)].into_iter()),
            )
            .nest(ctx.indent_width)
            .append(Doc::text("?>"))
            .group()
    }
}

fn reflow_raw(s: &str) -> impl Iterator<Item = Doc<'_>> {
    itertools::intersperse(
        s.split('\n')
            .map(|s| Doc::text(s.strip_suffix('\r').unwrap_or(s))),
        Doc::empty_line(),
    )
}

fn reflow_owned<'i, 'o: 'i>(s: &'i str) -> impl Iterator<Item = Doc<'o>> + 'i {
    itertools::intersperse(
        s.split('\n')
            .map(|s| Doc::text(s.strip_suffix('\r').unwrap_or(s).to_owned())),
        Doc::empty_line(),
    )
}

fn reflow_with_indent<'i, 'o: 'i>(
    s: &'i str,
    detect_indent: bool,
) -> impl Iterator<Item = Doc<'o>> + 'i {
    let indent = if detect_indent {
        helpers::detect_indent(s)
    } else {
        0
    };
    let mut pair_stack = vec![];
    s.split('\n').enumerate().flat_map(move |(i, s)| {
        let s = s.strip_suffix('\r').unwrap_or(s);
        let trimmed = if s.starts_with([' ', '\t']) {
            s.get(indent..).unwrap_or(s)
        } else {
            s
        };
        let should_keep_raw = matches!(pair_stack.last(), Some('`'));

        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '`' | '\'' | '"' => {
                    let last = pair_stack.last();
                    if last.is_some_and(|last| *last == c) {
                        pair_stack.pop();
                    } else if matches!(last, Some('$' | '{') | None) {
                        pair_stack.push(c);
                    }
                }
                '$' if matches!(pair_stack.last(), Some('`')) => {
                    if chars.next_if(|next| *next == '{').is_some() {
                        pair_stack.push('$');
                    }
                }
                '{' if !matches!(pair_stack.last(), Some('`' | '\'' | '"' | '/')) => {
                    pair_stack.push('{');
                }
                '}' if matches!(pair_stack.last(), Some('$' | '{')) => {
                    pair_stack.pop();
                }
                '/' if !matches!(pair_stack.last(), Some('\'' | '"' | '`')) => {
                    if chars.next_if(|next| *next == '*').is_some() {
                        pair_stack.push('*');
                    } else if chars.next_if(|next| *next == '/').is_some() {
                        break;
                    }
                }
                '*' => {
                    if chars.next_if(|next| *next == '/').is_some() {
                        pair_stack.pop();
                    }
                }
                '\\' if matches!(pair_stack.last(), Some('\'' | '"' | '`')) => {
                    chars.next();
                }
                _ => {}
            }
        }

        [
            if i == 0 {
                Doc::nil()
            } else if trimmed.trim().is_empty() || should_keep_raw {
                Doc::empty_line()
            } else {
                Doc::hard_line()
            },
            if should_keep_raw {
                Doc::text(s.to_owned())
            } else {
                Doc::text(trimmed.to_owned())
            },
        ]
        .into_iter()
    })
}

fn is_empty_element(children: &[Node], is_whitespace_sensitive: bool) -> bool {
    match &children {
        [] => true,
        [Node {
            kind: NodeKind::Text(text_node),
            ..
        }] => {
            !is_whitespace_sensitive
                && text_node
                    .raw
                    .trim_matches(|c: char| c.is_ascii_whitespace())
                    .is_empty()
        }
        _ => false,
    }
}

fn is_all_ascii_whitespace(s: &str) -> bool {
    !s.is_empty() && s.as_bytes().iter().all(|byte| byte.is_ascii_whitespace())
}

fn is_multi_line_attr(attr: &Attribute) -> bool {
    match attr {
        Attribute::Native(attr) => attr
            .value
            .is_some_and(|(value, _)| value.trim().contains('\n')),
        Attribute::JinjaComment(JinjaComment { raw: value, .. })
        | Attribute::JinjaTag(JinjaTag { content: value, .. }) => value.contains('\n'),
        Attribute::JinjaBlock(..) => true,
    }
}

fn should_ignore_node<'s, E, F>(index: usize, nodes: &[Node], ctx: &Ctx<'s, E, F>) -> bool
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    match index.checked_sub(1).and_then(|i| nodes.get(i)) {
        Some(Node {
            kind: NodeKind::Comment(comment),
            ..
        }) => has_ignore_directive(comment, ctx),
        Some(Node {
            kind: NodeKind::Text(text_node),
            ..
        }) if is_all_ascii_whitespace(text_node.raw) => {
            if let Some(Node {
                kind: NodeKind::Comment(comment),
                ..
            }) = index.checked_sub(2).and_then(|i| nodes.get(i))
            {
                has_ignore_directive(comment, ctx)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn has_ignore_directive<'s, E, F>(comment: &Comment, ctx: &Ctx<'s, E, F>) -> bool
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    comment
        .raw
        .trim_start()
        .strip_prefix(&ctx.options.ignore_comment_directive)
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_whitespace()) || rest.is_empty())
}

fn should_add_whitespace_before_text_node<'s>(
    text_node: &TextNode<'s>,
    is_first: bool,
) -> Option<Doc<'s>> {
    let trimmed = text_node
        .raw
        .trim_end_matches(|c: char| c.is_ascii_whitespace());
    if !is_first && trimmed.starts_with(|c: char| c.is_ascii_whitespace()) {
        let line_breaks_count = text_node
            .raw
            .chars()
            .take_while(|c| c.is_ascii_whitespace())
            .filter(|c| *c == '\n')
            .count();
        match line_breaks_count {
            0 => Some(Doc::soft_line()),
            1 => Some(Doc::hard_line()),
            _ => Some(Doc::empty_line().append(Doc::hard_line())),
        }
    } else {
        None
    }
}

fn should_add_whitespace_after_text_node<'s>(
    text_node: &TextNode<'s>,
    is_last: bool,
) -> Option<Doc<'s>> {
    let trimmed = text_node
        .raw
        .trim_start_matches(|c: char| c.is_ascii_whitespace());
    if !is_last && trimmed.ends_with(|c: char| c.is_ascii_whitespace()) {
        let line_breaks_count = text_node
            .raw
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_whitespace())
            .filter(|c| *c == '\n')
            .count();
        match line_breaks_count {
            0 => Some(Doc::soft_line()),
            1 => Some(Doc::hard_line()),
            _ => Some(Doc::empty_line().append(Doc::hard_line())),
        }
    } else {
        None
    }
}

fn has_two_more_non_text_children(children: &[Node], language: Language) -> bool {
    children
        .iter()
        .filter(|child| !is_text_like(child, language))
        .count()
        > 1
}

fn format_attr_value(value: impl AsRef<str>, quotes: &Quotes) -> Doc<'_> {
    let value = value.as_ref();
    let quote = if value.contains('"') {
        Doc::text("'")
    } else if value.contains('\'') {
        Doc::text("\"")
    } else if let Quotes::Double = quotes {
        Doc::text("\"")
    } else {
        Doc::text("'")
    };
    quote
        .clone()
        .concat(reflow_with_indent(value, true))
        .append(quote)
}

fn format_children_with_inserting_linebreak<'s, E, F>(
    children: &[Node<'s>],
    ctx: &mut Ctx<'s, E, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    Doc::list(
        children
            .iter()
            .enumerate()
            .fold(
                (Vec::with_capacity(children.len() * 2), true),
                |(mut docs, is_prev_text_like), (i, child)| {
                    let is_current_text_like = is_text_like(child, ctx.language);
                    if should_ignore_node(i, children, ctx) {
                        let raw = child.raw.trim_end_matches([' ', '\t']);
                        let last_line_break_removed = raw.strip_suffix(['\n', '\r']);
                        docs.extend(reflow_raw(last_line_break_removed.unwrap_or(raw)));
                        if i < children.len() - 1 && last_line_break_removed.is_some() {
                            docs.push(Doc::hard_line());
                        }
                    } else {
                        let maybe_hard_line = if is_prev_text_like || is_current_text_like {
                            None
                        } else {
                            Some(Doc::hard_line())
                        };
                        match &child.kind {
                            NodeKind::Text(text_node) => {
                                let is_first = i == 0;
                                let is_last = i + 1 == children.len();
                                if is_all_ascii_whitespace(text_node.raw) {
                                    if !is_first && !is_last {
                                        if text_node.line_breaks > 1 {
                                            docs.push(Doc::empty_line());
                                        }
                                        docs.push(Doc::hard_line());
                                    }
                                } else {
                                    if let Some(hard_line) = maybe_hard_line {
                                        docs.push(hard_line);
                                    } else if let Some(doc) =
                                        should_add_whitespace_before_text_node(text_node, is_first)
                                    {
                                        docs.push(doc);
                                    }
                                    docs.push(text_node.doc(ctx, state));
                                    if let Some(doc) =
                                        should_add_whitespace_after_text_node(text_node, is_last)
                                    {
                                        docs.push(doc);
                                    }
                                }
                            }
                            child => {
                                if let Some(hard_line) = maybe_hard_line {
                                    docs.push(hard_line);
                                }
                                docs.push(child.doc(ctx, state));
                            }
                        }
                    }
                    (docs, is_current_text_like)
                },
            )
            .0,
    )
    .group()
}

fn is_text_like(node: &Node, language: Language) -> bool {
    match &node.kind {
        NodeKind::Element(element) => {
            helpers::is_whitespace_sensitive_tag(element.tag_name, language)
        }
        NodeKind::Text(..) | NodeKind::JinjaInterpolation(..) => true,
        _ => false,
    }
}

fn format_children_without_inserting_linebreak<'s, E, F>(
    children: &[Node<'s>],
    ctx: &mut Ctx<'s, E, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    Doc::list(
        children
            .iter()
            .enumerate()
            .fold(
                (Vec::with_capacity(children.len() * 2), true),
                |(mut docs, is_prev_text_like), (i, child)| {
                    if should_ignore_node(i, children, ctx) {
                        let raw = child.raw.trim_end_matches([' ', '\t']);
                        let last_line_break_removed = raw.strip_suffix(['\n', '\r']);
                        docs.extend(reflow_raw(last_line_break_removed.unwrap_or(raw)));
                        if i < children.len() - 1 && last_line_break_removed.is_some() {
                            docs.push(Doc::hard_line());
                        }
                    } else if let NodeKind::Text(text_node) = &child.kind {
                        let is_first = i == 0;
                        let is_last = i + 1 == children.len();
                        if !is_first && !is_last && is_all_ascii_whitespace(text_node.raw) {
                            match text_node.line_breaks {
                                0 => {
                                    if !is_prev_text_like
                                        && children
                                            .get(i + 1)
                                            .is_some_and(|next| !is_text_like(next, ctx.language))
                                    {
                                        docs.push(Doc::line_or_space());
                                    } else {
                                        docs.push(Doc::soft_line());
                                    }
                                }
                                1 => docs.push(Doc::hard_line()),
                                _ => {
                                    docs.push(Doc::empty_line());
                                    docs.push(Doc::hard_line());
                                }
                            }
                            return (docs, true);
                        }

                        if let Some(doc) =
                            should_add_whitespace_before_text_node(text_node, is_first)
                        {
                            docs.push(doc);
                        }
                        docs.push(text_node.doc(ctx, state));
                        if let Some(doc) = should_add_whitespace_after_text_node(text_node, is_last)
                        {
                            docs.push(doc);
                        }
                    } else {
                        docs.push(child.kind.doc(ctx, state))
                    }
                    (docs, is_text_like(child, ctx.language))
                },
            )
            .0,
    )
    .group()
}

fn format_control_structure_block_children<'s, E, F>(
    children: &[Node<'s>],
    ctx: &mut Ctx<'s, E, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    match children {
        [Node {
            kind: NodeKind::Text(text_node),
            ..
        }] if is_all_ascii_whitespace(text_node.raw) => Doc::line_or_space(),
        _ => format_ws_sensitive_leading_ws(children)
            .append(format_children_without_inserting_linebreak(
                children, ctx, state,
            ))
            .nest(ctx.indent_width)
            .append(format_ws_sensitive_trailing_ws(children)),
    }
}

fn compute_attr_value_quote<'s, E, F>(
    attr_value: &str,
    initial_quote: Option<char>,
    ctx: &mut Ctx<'s, E, F>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, E>,
{
    let has_single = attr_value.contains('\'');
    let has_double = attr_value.contains('"');
    if has_double && has_single {
        if let Some(quote) = initial_quote {
            Doc::text(quote.to_string())
        } else if let Quotes::Double = ctx.options.quotes {
            Doc::text("\"")
        } else {
            Doc::text("'")
        }
    } else if has_double {
        Doc::text("'")
    } else if has_single {
        Doc::text("\"")
    } else if let Quotes::Double = ctx.options.quotes {
        Doc::text("\"")
    } else {
        Doc::text("'")
    }
}

fn format_ws_sensitive_leading_ws<'s>(children: &[Node<'s>]) -> Doc<'s> {
    if let Some(Node {
        kind: NodeKind::Text(text_node),
        ..
    }) = children.first()
    {
        if text_node.raw.starts_with(|c: char| c.is_ascii_whitespace()) {
            if text_node.line_breaks > 0 {
                Doc::hard_line()
            } else {
                Doc::line_or_space()
            }
        } else {
            Doc::nil()
        }
    } else {
        Doc::nil()
    }
}

fn format_ws_sensitive_trailing_ws<'s>(children: &[Node<'s>]) -> Doc<'s> {
    if let Some(Node {
        kind: NodeKind::Text(text_node),
        ..
    }) = children.last()
    {
        if text_node.raw.ends_with(|c: char| c.is_ascii_whitespace()) {
            if text_node.line_breaks > 0 {
                Doc::hard_line()
            } else {
                Doc::line_or_space()
            }
        } else {
            Doc::nil()
        }
    } else {
        Doc::nil()
    }
}

fn format_ws_insensitive_leading_ws<'s>(children: &[Node<'s>]) -> Doc<'s> {
    match children.first() {
        Some(Node {
            kind: NodeKind::Text(text_node),
            ..
        }) if text_node.line_breaks > 0 => Doc::hard_line(),
        _ => Doc::line_or_nil(),
    }
}

fn format_ws_insensitive_trailing_ws<'s>(children: &[Node<'s>]) -> Doc<'s> {
    match children.last() {
        Some(Node {
            kind: NodeKind::Text(text_node),
            ..
        }) if text_node.line_breaks > 0 => Doc::hard_line(),
        _ => Doc::line_or_nil(),
    }
}
