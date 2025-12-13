use std::collections::HashMap;

use serde::Deserialize;

// DEPRECATION: Remove in v6.2.0 (after v6.0.0 and v6.1.0)
pub mod legacy;

/// Root `TagSpec` document (v0.6.0)
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TagSpecDef {
    /// Specification version (defaults to "0.6.0")
    #[serde(default = "default_version")]
    pub version: String,
    /// Template engine (defaults to "django")
    #[serde(default = "default_engine")]
    pub engine: String,
    /// Engine version constraint (PEP 440 for Django)
    #[serde(default)]
    pub requires_engine: Option<String>,
    /// References to parent documents for overlay composition
    #[serde(default)]
    pub extends: Vec<String>,
    /// Tag libraries grouped by module
    #[serde(default)]
    pub libraries: Vec<TagLibraryDef>,
    /// Extra metadata for extensibility
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Tag library grouping tags by module
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TagLibraryDef {
    /// Dotted Python import path (e.g., "django.template.defaulttags")
    pub module: String,
    /// Engine version constraint for this library
    #[serde(default)]
    pub requires_engine: Option<String>,
    /// Tags exposed by this library
    #[serde(default)]
    pub tags: Vec<TagDef>,
    /// Extra metadata
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Individual tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TagDef {
    /// Tag name (e.g., "for", "if", "url")
    pub name: String,
    /// Tag type: block, loader, or standalone
    #[serde(rename = "type")]
    pub tag_type: TagTypeDef,
    /// End tag specification (auto-synthesized for block tags if omitted)
    #[serde(default)]
    pub end: Option<EndTagDef>,
    /// Intermediate tags (e.g., "elif", "else" for "if")
    #[serde(default)]
    pub intermediates: Vec<IntermediateTagDef>,
    /// Opening tag arguments
    #[serde(default)]
    pub args: Vec<TagArgDef>,
    /// Extra metadata
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Tag type classification
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TagTypeDef {
    /// Block tag with opening/closing tags
    Block,
    /// Loader tag (may optionally behave as block)
    Loader,
    /// Standalone tag (no closing tag)
    Standalone,
}

/// End tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EndTagDef {
    /// End tag name (e.g., "endfor", "endif")
    pub name: String,
    /// Whether the end tag must appear explicitly
    #[serde(default = "default_true")]
    pub required: bool,
    /// End tag arguments
    #[serde(default)]
    pub args: Vec<TagArgDef>,
    /// Extra metadata
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Intermediate tag specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct IntermediateTagDef {
    /// Intermediate tag name (e.g., "elif", "else", "empty")
    pub name: String,
    /// Intermediate tag arguments
    #[serde(default)]
    pub args: Vec<TagArgDef>,
    /// Minimum occurrence count
    #[serde(default)]
    pub min: Option<usize>,
    /// Maximum occurrence count
    #[serde(default)]
    pub max: Option<usize>,
    /// Positioning constraint
    #[serde(default = "default_position")]
    pub position: PositionDef,
    /// Extra metadata
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Intermediate tag positioning
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PositionDef {
    /// Can appear anywhere
    Any,
    /// Must be last before end tag
    Last,
}

/// Tag argument specification
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TagArgDef {
    /// Argument name
    pub name: String,
    /// Whether the argument is required
    #[serde(default = "default_true")]
    pub required: bool,
    /// Argument type: positional, keyword, or both
    #[serde(rename = "type", default = "default_arg_type")]
    pub arg_type: ArgTypeDef,
    /// Argument kind (semantic role)
    pub kind: ArgKindDef,
    /// Exact token count (null means variable)
    #[serde(default)]
    pub count: Option<usize>,
    /// Extra metadata
    #[serde(default)]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

/// Argument type (positional vs keyword)
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArgTypeDef {
    /// Can be positional or keyword
    Both,
    /// Must be positional
    Positional,
    /// Must be keyword
    Keyword,
}

/// Argument kind (semantic classification)
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArgKindDef {
    /// Any template expression or literal
    Any,
    /// Variable assignment (e.g., "as varname")
    Assignment,
    /// Choice from specific literals
    Choice,
    /// Literal token
    Literal,
    /// Boolean modifier (e.g., "reversed")
    Modifier,
    /// Mandatory syntactic token (e.g., "in")
    Syntax,
    /// Template variable or filter expression
    Variable,
}

fn default_version() -> String {
    "0.6.0".to_string()
}

fn default_engine() -> String {
    "django".to_string()
}

fn default_true() -> bool {
    true
}

fn default_position() -> PositionDef {
    PositionDef::Any
}

fn default_arg_type() -> ArgTypeDef {
    ArgTypeDef::Both
}
