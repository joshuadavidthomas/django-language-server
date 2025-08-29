#[derive(Debug)]
pub enum ClosingBrace {
    None,
    PartialClose, // just }
    FullClose,    // %}
}

#[derive(Debug)]
pub struct TemplateTagContext {
    pub partial_tag: String,
    pub closing_brace: ClosingBrace,
    pub needs_leading_space: bool,
}
