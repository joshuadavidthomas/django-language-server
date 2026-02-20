// Vendored from markup_fmt v0.26.0

#[derive(Clone)]
pub(crate) struct State<'s> {
    pub(crate) current_tag_name: Option<&'s str>,
    pub(crate) is_root: bool,
    pub(crate) in_svg: bool,
    pub(crate) indent_level: u16,
}
