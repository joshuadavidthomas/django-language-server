use serde::Deserialize;

/// A single tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TagSpecDef {
    /// Tag name (e.g., "for", "if", "cache")
    pub name: String,
    /// Module where this tag is defined (e.g., "django.template.defaulttags")
    pub module: String,
    /// Optional end tag specification
    #[serde(default)]
    pub end_tag: Option<EndTagDef>,
    /// Optional intermediate tags (e.g., "elif", "else" for "if" tag)
    #[serde(default)]
    pub intermediate_tags: Vec<IntermediateTagDef>,
    /// Tag arguments specification
    #[serde(default)]
    pub args: Vec<TagArgDef>,
}

/// End tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EndTagDef {
    /// End tag name (e.g., "endfor", "endif")
    pub name: String,
    /// Whether the end tag is optional
    #[serde(default)]
    pub optional: bool,
    /// Optional arguments for the end tag
    #[serde(default)]
    pub args: Vec<TagArgDef>,
}

/// Intermediate tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct IntermediateTagDef {
    /// Intermediate tag name (e.g., "elif", "else")
    pub name: String,
    /// Optional arguments for the end tag
    #[serde(default)]
    pub args: Vec<TagArgDef>,
}

/// Tag argument specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TagArgDef {
    /// Argument name
    pub name: String,
    /// Whether the argument is required
    #[serde(default = "default_true")]
    pub required: bool,
    /// Argument type
    #[serde(rename = "type")]
    pub arg_type: ArgTypeDef,
}

/// Argument type specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ArgTypeDef {
    /// Simple type like "variable", "string", etc.
    Simple(SimpleArgTypeDef),
    /// Choice from a list of values
    Choice { choice: Vec<String> },
}

/// Simple argument types
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SimpleArgTypeDef {
    Literal,
    Variable,
    String,
    Expression,
    Assignment,
    VarArgs,
}

fn default_true() -> bool {
    true
}
