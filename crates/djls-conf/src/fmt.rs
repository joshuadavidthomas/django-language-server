use std::num::NonZeroU16;

use serde::Deserialize;
use serde::Deserializer;

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum IndentStyle {
    #[default]
    Spaces,
    Tabs,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    #[default]
    Auto,
    Html,
    Text,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct FormatConfig {
    #[serde(deserialize_with = "deserialize_nonzero_u16")]
    indent_width: NonZeroU16,
    indent_style: IndentStyle,
    content_type: ContentType,
    #[serde(deserialize_with = "deserialize_nonzero_u16")]
    print_width: NonZeroU16,
    sort_load_libraries: bool,
    merge_load_tags: bool,
    label_endblocks: bool,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            indent_width: NonZeroU16::new(4).expect("format default indent_width is non-zero"),
            indent_style: IndentStyle::Spaces,
            content_type: ContentType::Auto,
            print_width: NonZeroU16::new(80).expect("format default print_width is non-zero"),
            sort_load_libraries: true,
            merge_load_tags: true,
            label_endblocks: true,
        }
    }
}

impl FormatConfig {
    #[must_use]
    pub fn indent_width(&self) -> u16 {
        self.indent_width.get()
    }

    #[must_use]
    pub fn indent_style(&self) -> IndentStyle {
        self.indent_style
    }

    #[must_use]
    pub fn content_type(&self) -> ContentType {
        self.content_type
    }

    #[must_use]
    pub fn print_width(&self) -> u16 {
        self.print_width.get()
    }

    #[must_use]
    pub fn sort_load_libraries(&self) -> bool {
        self.sort_load_libraries
    }

    #[must_use]
    pub fn merge_load_tags(&self) -> bool {
        self.merge_load_tags
    }

    #[must_use]
    pub fn label_endblocks(&self) -> bool {
        self.label_endblocks
    }

    #[must_use]
    pub fn with_content_type(mut self, value: ContentType) -> Self {
        self.content_type = value;
        self
    }

    #[must_use]
    pub fn with_sort_load_libraries(mut self, value: bool) -> Self {
        self.sort_load_libraries = value;
        self
    }

    #[must_use]
    pub fn with_merge_load_tags(mut self, value: bool) -> Self {
        self.merge_load_tags = value;
        self
    }

    #[must_use]
    pub fn with_label_endblocks(mut self, value: bool) -> Self {
        self.label_endblocks = value;
        self
    }
}

fn deserialize_nonzero_u16<'de, D>(deserializer: D) -> Result<NonZeroU16, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value = u16::deserialize(deserializer)?;
    NonZeroU16::new(value).ok_or_else(|| D::Error::custom("expected a non-zero integer"))
}
