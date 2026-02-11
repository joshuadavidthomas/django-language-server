use djls_source::Span;
use serde::Serialize;

use crate::quotes::for_each_unquoted;

/// A parsed filter expression within a Django variable node.
///
/// Represents a single filter in a chain like `{{ value|default:'nothing'|title }}`.
/// Each filter has a name, an optional argument, and a span covering its position
/// within the source text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Filter {
    pub name: String,
    pub arg: Option<String>,
    pub span: Span,
}

impl Filter {
    #[must_use]
    pub fn new(name: String, arg: Option<String>, span: Span) -> Self {
        Self { name, arg, span }
    }
}

/// Saturating conversion from `usize` to `u32`, clamping at `u32::MAX`.
fn usize_to_u32(val: usize) -> u32 {
    u32::try_from(val).unwrap_or(u32::MAX)
}

/// Split a variable expression (the content between `{{ }}`) into segments
/// separated by `|`, respecting quoted strings.
///
/// Returns an iterator of `(segment_str, byte_offset_within_content)` pairs.
pub(crate) fn split_variable_expression(content: &str) -> impl Iterator<Item = (&str, u32)> {
    let mut segments = Vec::new();
    let mut start = 0;

    for_each_unquoted(
        content,
        |ch| ch == '|',
        false,
        |idx| {
            segments.push((&content[start..idx], usize_to_u32(start)));
            start = idx + 1;
            false
        },
    );

    segments.push((&content[start..], usize_to_u32(start)));
    segments.into_iter()
}

/// Parse a single raw filter string (e.g. `default:'nothing'` or `title`) into a
/// structured `Filter`. The `base_offset` is the byte offset of the start of this
/// filter segment in the source file.
pub(crate) fn parse_filter(raw: &str, base_offset: u32) -> Option<Filter> {
    let trimmed_start = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();

    let filter_offset = base_offset + usize_to_u32(trimmed_start);

    let mut colon_pos = None;

    for_each_unquoted(
        trimmed,
        |ch| ch == ':',
        false,
        |idx| {
            colon_pos = Some(idx);
            true
        },
    );

    let (name, arg) = match colon_pos {
        Some(pos) => {
            let name = trimmed[..pos].trim();
            let arg = trimmed[pos + 1..].trim();
            let arg = if arg.is_empty() {
                None
            } else {
                Some(arg.to_string())
            };
            (name.to_string(), arg)
        }
        None => (trimmed.to_string(), None),
    };

    if name.is_empty() {
        return None;
    }

    let span = Span::new(filter_offset, usize_to_u32(trimmed.len()));
    Some(Filter::new(name, arg, span))
}
