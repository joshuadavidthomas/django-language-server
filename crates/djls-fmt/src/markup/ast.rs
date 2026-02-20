// Vendored from markup_fmt v0.26.0
// Stripped to HTML + Jinja/Django + XML only

#[derive(Debug)]
pub enum Attribute<'s> {
    JinjaBlock(JinjaBlock<'s, Attribute<'s>>),
    JinjaComment(JinjaComment<'s>),
    JinjaTag(JinjaTag<'s>),
    Native(NativeAttribute<'s>),
}

#[derive(Debug)]
pub struct Cdata<'s> {
    pub raw: &'s str,
}

#[derive(Debug)]
pub struct Comment<'s> {
    pub raw: &'s str,
}

#[derive(Debug)]
pub struct Doctype<'s> {
    pub keyword: &'s str,
    pub value: &'s str,
}

#[derive(Debug)]
pub struct Element<'s> {
    pub tag_name: &'s str,
    pub attrs: Vec<Attribute<'s>>,
    pub first_attr_same_line: bool,
    pub children: Vec<Node<'s>>,
    pub self_closing: bool,
    pub void_element: bool,
}

#[derive(Debug)]
pub struct JinjaBlock<'s, T> {
    pub body: Vec<JinjaTagOrChildren<'s, T>>,
}

#[derive(Debug)]
pub struct JinjaComment<'s> {
    pub raw: &'s str,
}

#[derive(Debug)]
pub struct JinjaInterpolation<'s> {
    pub expr: &'s str,
    pub start: usize,
    pub trim_prev: bool,
    pub trim_next: bool,
}

#[derive(Debug)]
pub struct JinjaTag<'s> {
    pub content: &'s str,
    pub start: usize,
}

#[derive(Debug)]
pub enum JinjaTagOrChildren<'s, T> {
    Tag(JinjaTag<'s>),
    Children(Vec<T>),
}

#[derive(Debug)]
pub struct NativeAttribute<'s> {
    pub name: &'s str,
    pub value: Option<(&'s str, usize)>,
    pub quote: Option<char>,
}

#[derive(Debug)]
pub struct Node<'s> {
    pub kind: NodeKind<'s>,
    pub raw: &'s str,
}

#[derive(Debug)]
pub enum NodeKind<'s> {
    Cdata(Cdata<'s>),
    Comment(Comment<'s>),
    Doctype(Doctype<'s>),
    Element(Element<'s>),
    JinjaBlock(JinjaBlock<'s, Node<'s>>),
    JinjaComment(JinjaComment<'s>),
    JinjaInterpolation(JinjaInterpolation<'s>),
    JinjaTag(JinjaTag<'s>),
    Text(TextNode<'s>),
    XmlDecl(XmlDecl<'s>),
}

#[derive(Debug)]
pub struct Root<'s> {
    pub children: Vec<Node<'s>>,
}

#[derive(Debug)]
pub struct TextNode<'s> {
    pub raw: &'s str,
    pub line_breaks: usize,
    pub start: usize,
}

#[derive(Debug)]
pub struct XmlDecl<'s> {
    pub attrs: Vec<NativeAttribute<'s>>,
}
