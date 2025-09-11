use crate::ast::LineOffsets;
use crate::ast::Span;
use crate::templatetags::TagSpec;
use crate::templatetags::TagSpecs;
use crate::templatetags::TagType;

/// The new hierarchical SyntaxTree structure for Django templates.
/// This replaces the flat NodeList structure for LSP operations.
#[salsa::tracked]
pub struct SyntaxTree<'db> {
    #[tracked]
    pub root: SyntaxNodeId<'db>,
    #[tracked]
    #[returns(ref)]
    pub line_offsets: LineOffsets,
}

/// Enhanced syntax nodes with hierarchical structure
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SyntaxNode<'db> {
    Root { children: Vec<SyntaxNodeId<'db>> },
    Tag(TagNode<'db>),
    Text(TextNode),
    Variable(VariableNode<'db>),
    Comment(CommentNode),
    Error { message: String, span: Span },
}

/// Use salsa interned IDs for efficient tree traversal
#[salsa::interned(debug)]
pub struct SyntaxNodeId<'db> {
    pub node: SyntaxNode<'db>,
}

/// Enhanced `TagNode` with metadata derived from `TagSpecs`
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TagNode<'db> {
    pub name: TagName<'db>,
    pub bits: Vec<String>,
    pub span: Span,
    pub meta: TagMeta<'db>,
    pub children: Vec<SyntaxNodeId<'db>>,
}

#[salsa::interned(debug)]
pub struct TagName<'db> {
    pub text: String,
}

/// Comprehensive tag metadata integrating with `TagSpecs`
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TagMeta<'db> {
    pub tag_type: TagType,            // From existing TagType::for_name()
    pub shape: TagShape,              // Derived from TagSpec
    pub spec_id: Option<String>,      // Reference to TagSpec name
    pub branch_kind: Option<String>,  // "if"/"elif"/"else" for conditionals
    pub parsed_args: ParsedArgs<'db>, // Parsed according to TagSpec.args
}

/// Shape classification derived from `TagSpec` structure
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum TagShape {
    Singleton, // No end_tag in spec
    Block {
        ender: String,      // From TagSpec.end_tag.name
        has_branches: bool, // Has TagSpec.intermediate_tags
    },
    RawBlock {
        ender: String, // Special handling for comment/verbatim
    },
}

impl TagShape {
    /// Derive shape from existing `TagSpec`
    #[must_use]
    pub fn from_spec(spec: &TagSpec) -> Self {
        match &spec.end_tag {
            None => TagShape::Singleton,
            Some(end_tag) => {
                // Check if it's a raw block (comment, verbatim, spaceless)
                let is_raw = spec
                    .name
                    .as_ref()
                    .is_some_and(|n| matches!(n.as_str(), "comment" | "verbatim" | "spaceless"));

                if is_raw {
                    TagShape::RawBlock {
                        ender: end_tag.name.clone(),
                    }
                } else {
                    TagShape::Block {
                        ender: end_tag.name.clone(),
                        has_branches: spec.intermediate_tags.is_some(),
                    }
                }
            }
        }
    }
}

/// Parsed arguments according to `TagSpec` definition
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParsedArgs<'db> {
    pub positional: Vec<ParsedArg<'db>>,
    pub named: Vec<(String, ParsedArg<'db>)>, // Use Vec instead of HashMap for Hash impl
}

impl<'db> ParsedArgs<'db> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            positional: Vec::new(),
            named: Vec::new(),
        }
    }

    pub fn add_positional(&mut self, arg: ParsedArg<'db>) {
        self.positional.push(arg);
    }

    pub fn add_named(&mut self, name: String, arg: ParsedArg<'db>) {
        self.named.push((name, arg));
    }

    /// Get positional argument by index
    #[must_use]
    pub fn get_positional(&self, index: usize) -> Option<&ParsedArg<'db>> {
        self.positional.get(index)
    }

    /// Get named argument by key
    #[must_use]
    pub fn get_named(&self, key: &str) -> Option<&ParsedArg<'db>> {
        self.named
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, arg)| arg)
    }

    /// Get total number of arguments (positional + named)
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.positional.len() + self.named.len()
    }

    /// Get count of positional arguments
    #[must_use]
    pub fn positional_count(&self) -> usize {
        self.positional.len()
    }

    /// Get count of named arguments
    #[must_use]
    pub fn named_count(&self) -> usize {
        self.named.len()
    }

    /// Check if arguments are empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positional.is_empty() && self.named.is_empty()
    }

    /// Get all named argument keys
    #[must_use]
    pub fn named_keys(&self) -> Vec<&String> {
        self.named.iter().map(|(name, _)| name).collect()
    }

    /// Iterate over positional arguments
    pub fn iter_positional(&self) -> impl Iterator<Item = &ParsedArg<'db>> {
        self.positional.iter()
    }

    /// Iterate over named arguments
    pub fn iter_named(&self) -> impl Iterator<Item = (&String, &ParsedArg<'db>)> {
        self.named.iter().map(|(name, arg)| (name, arg))
    }
}

impl Default for ParsedArgs<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Individual parsed argument with type information
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum ParsedArg<'db> {
    Variable(VariableName<'db>),
    String(String),
    Expression(String), // TODO: Parse into expression AST
    Literal(String),
    Assignment { name: String, value: String },
}

impl<'db> ParsedArg<'db> {
    /// Get the string representation of this argument
    pub fn as_string(&self, db: &'db dyn crate::db::Db) -> String {
        match self {
            ParsedArg::Variable(var) => format!("${{{}}}", var.text(db)),
            ParsedArg::String(s) => format!("\"{s}\""),
            ParsedArg::Expression(expr) => expr.clone(),
            ParsedArg::Literal(lit) => lit.clone(),
            ParsedArg::Assignment { name, value } => format!("{name}={value}"),
        }
    }

    /// Check if this argument is a variable
    #[must_use]
    pub fn is_variable(&self) -> bool {
        matches!(self, ParsedArg::Variable(_))
    }

    /// Check if this argument is a string literal
    #[must_use]
    pub fn is_string(&self) -> bool {
        matches!(self, ParsedArg::String(_))
    }

    /// Check if this argument is an expression
    #[must_use]
    pub fn is_expression(&self) -> bool {
        matches!(self, ParsedArg::Expression(_))
    }

    /// Check if this argument is a literal value
    #[must_use]
    pub fn is_literal(&self) -> bool {
        matches!(self, ParsedArg::Literal(_))
    }

    /// Check if this argument is an assignment
    #[must_use]
    pub fn is_assignment(&self) -> bool {
        matches!(self, ParsedArg::Assignment { .. })
    }

    /// Get the variable name if this is a variable argument
    #[must_use]
    pub fn as_variable(&self) -> Option<&VariableName<'db>> {
        match self {
            ParsedArg::Variable(var) => Some(var),
            _ => None,
        }
    }

    /// Get the assignment parts if this is an assignment argument
    #[must_use]
    pub fn as_assignment(&self) -> Option<(&str, &str)> {
        match self {
            ParsedArg::Assignment { name, value } => Some((name, value)),
            _ => None,
        }
    }
}

#[salsa::interned(debug)]
pub struct VariableName<'db> {
    pub text: String,
}

/// Text node for template literal content
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TextNode {
    pub content: String,
    pub span: Span,
}

/// Variable node for Django template variables
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct VariableNode<'db> {
    pub var: VariableName<'db>,
    pub filters: Vec<FilterName<'db>>,
    pub span: Span,
}

#[salsa::interned(debug)]
pub struct FilterName<'db> {
    pub text: String,
}

/// Comment node for Django template comments
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct CommentNode {
    pub content: String,
    pub span: Span,
}

impl<'db> TagMeta<'db> {
    /// Create `TagMeta` from tag name and `TagSpecs`
    pub fn from_tag(
        db: &'db dyn crate::db::Db,
        name: &str,
        bits: &[String],
        tag_specs: &TagSpecs,
    ) -> Self {
        let tag_type = TagType::for_name(name, tag_specs);

        let (shape, spec_id, branch_kind) = if let Some(spec) = tag_specs.get(name) {
            let shape = TagShape::from_spec(spec);
            let spec_id = Some(name.to_string());

            // Determine branch kind for conditional tags
            let branch_kind = match name {
                "elif" | "else" => Some(name.to_string()),
                _ => None,
            };

            (shape, spec_id, branch_kind)
        } else {
            // Unknown tag, treat as singleton
            (TagShape::Singleton, None, None)
        };

        // Parse arguments according to TagSpec
        let parsed_args = Self::parse_arguments(db, bits, tag_specs.get(name));

        Self {
            tag_type,
            shape,
            spec_id,
            branch_kind,
            parsed_args,
        }
    }

    /// Parse tag arguments according to `TagSpec` definition
    fn parse_arguments(
        db: &'db dyn crate::db::Db,
        bits: &[String],
        spec: Option<&TagSpec>,
    ) -> ParsedArgs<'db> {
        let mut parsed_args = ParsedArgs::new();

        // If no spec is available, treat all bits as expressions
        let Some(spec) = spec else {
            for bit in bits {
                parsed_args.add_positional(ParsedArg::Expression(bit.clone()));
            }
            return parsed_args;
        };

        // Parse according to spec arguments
        let mut bit_index = 0;
        let mut positional_index = 0;

        while bit_index < bits.len() {
            let bit = &bits[bit_index];

            // Check for assignment (key=value)
            if let Some((key, value)) = bit.split_once('=') {
                parsed_args.add_named(
                    key.to_string(),
                    ParsedArg::Assignment {
                        name: key.to_string(),
                        value: value.to_string(),
                    },
                );
            } else {
                // Determine argument type based on spec
                let arg_type = spec.args.get(positional_index).map(|arg| &arg.arg_type);

                let parsed_arg = match arg_type {
                    Some(crate::templatetags::ArgType::Simple(simple_type)) => match simple_type {
                        crate::templatetags::SimpleArgType::Literal => {
                            ParsedArg::Literal(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Variable => {
                            ParsedArg::Variable(VariableName::new(db, bit.clone()))
                        }
                        crate::templatetags::SimpleArgType::String => {
                            ParsedArg::String(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Expression
                        | crate::templatetags::SimpleArgType::VarArgs => {
                            ParsedArg::Expression(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Assignment => ParsedArg::Assignment {
                            name: bit.clone(),
                            value: String::new(),
                        },
                    },
                    Some(crate::templatetags::ArgType::Choice { choice }) => {
                        // Validate against choices and treat as literal
                        if choice.contains(bit) {
                            ParsedArg::Literal(bit.clone())
                        } else {
                            // Invalid choice, treat as expression for error handling
                            ParsedArg::Expression(bit.clone())
                        }
                    }
                    None => {
                        // No more spec args, treat as expression
                        ParsedArg::Expression(bit.clone())
                    }
                };

                parsed_args.add_positional(parsed_arg);
                positional_index += 1;
            }

            bit_index += 1;
        }

        parsed_args
    }

    /// Check if this tag matches a specific `TagType`
    #[must_use]
    pub fn is_type(&self, tag_type: &TagType) -> bool {
        matches!(
            (&self.tag_type, tag_type),
            (TagType::Opener, TagType::Opener)
                | (TagType::Closer, TagType::Closer)
                | (TagType::Intermediate, TagType::Intermediate)
                | (TagType::Standalone, TagType::Standalone)
        )
    }

    /// Check if this tag can have children based on its shape
    #[must_use]
    pub fn can_have_children(&self) -> bool {
        matches!(
            self.shape,
            TagShape::Block { .. } | TagShape::RawBlock { .. }
        )
    }

    /// Get the expected closer tag name if this is an opener
    #[must_use]
    pub fn expected_closer(&self) -> Option<&str> {
        match &self.shape {
            TagShape::Block { ender, .. } | TagShape::RawBlock { ender } => Some(ender),
            TagShape::Singleton => None,
        }
    }
}

impl<'db> SyntaxTree<'db> {
    /// Create a new empty syntax tree
    pub fn empty(db: &'db dyn crate::db::Db) -> Self {
        let root = SyntaxNode::Root {
            children: Vec::new(),
        };
        let root_id = SyntaxNodeId::new(db, root);
        let line_offsets = LineOffsets::default();

        SyntaxTree::new(db, root_id, line_offsets)
    }

    /// Get all child nodes of the root
    pub fn children(&self, db: &'db dyn crate::db::Db) -> Vec<SyntaxNodeId<'db>> {
        match &self.root(db).resolve(db) {
            SyntaxNode::Root { children } => children.clone(),
            _ => Vec::new(),
        }
    }
}

impl<'db> SyntaxNodeId<'db> {
    /// Resolve this ID to the actual node
    pub fn resolve(&self, db: &'db dyn crate::db::Db) -> SyntaxNode<'db> {
        self.node(db)
    }

    /// Check if this node is a specific tag type
    pub fn is_tag(&self, db: &'db dyn crate::db::Db, tag_name: &str) -> bool {
        match &self.resolve(db) {
            SyntaxNode::Tag(tag_node) => tag_node.name.text(db) == tag_name,
            _ => false,
        }
    }

    /// Get the span of this node
    pub fn span(&self, db: &'db dyn crate::db::Db) -> Option<Span> {
        match &self.resolve(db) {
            SyntaxNode::Tag(tag_node) => Some(tag_node.span),
            SyntaxNode::Text(text_node) => Some(text_node.span),
            SyntaxNode::Variable(var_node) => Some(var_node.span),
            SyntaxNode::Comment(comment_node) => Some(comment_node.span),
            SyntaxNode::Error { span, .. } => Some(*span),
            SyntaxNode::Root { .. } => None,
        }
    }

    /// Get all children of this node (for hierarchical nodes)
    pub fn children(&self, db: &'db dyn crate::db::Db) -> Vec<SyntaxNodeId<'db>> {
        match &self.resolve(db) {
            SyntaxNode::Root { children } => children.clone(),
            SyntaxNode::Tag(tag_node) => tag_node.children.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templatetags::EndTag;
    use crate::templatetags::IntermediateTag;


    #[test]
    fn test_tag_shape_from_spec() {
        // Test singleton tag (no end_tag)
        let singleton_spec = TagSpec {
            name: Some("load".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(TagShape::from_spec(&singleton_spec), TagShape::Singleton);

        // Test block tag with branches
        let block_spec = TagSpec {
            name: Some("if".to_string()),
            end_tag: Some(EndTag {
                name: "endif".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: Some(vec![
                IntermediateTag {
                    name: "elif".to_string(),
                },
                IntermediateTag {
                    name: "else".to_string(),
                },
            ]),
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&block_spec),
            TagShape::Block {
                ender: "endif".to_string(),
                has_branches: true,
            }
        );

        // Test block tag without branches
        let simple_block_spec = TagSpec {
            name: Some("block".to_string()),
            end_tag: Some(EndTag {
                name: "endblock".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&simple_block_spec),
            TagShape::Block {
                ender: "endblock".to_string(),
                has_branches: false,
            }
        );

        // Test raw block tag (comment)
        let raw_spec = TagSpec {
            name: Some("comment".to_string()),
            end_tag: Some(EndTag {
                name: "endcomment".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&raw_spec),
            TagShape::RawBlock {
                ender: "endcomment".to_string(),
            }
        );

        // Test verbatim raw block
        let verbatim_spec = TagSpec {
            name: Some("verbatim".to_string()),
            end_tag: Some(EndTag {
                name: "endverbatim".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&verbatim_spec),
            TagShape::RawBlock {
                ender: "endverbatim".to_string(),
            }
        );

        // Test spaceless raw block
        let spaceless_spec = TagSpec {
            name: Some("spaceless".to_string()),
            end_tag: Some(EndTag {
                name: "endspaceless".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&spaceless_spec),
            TagShape::RawBlock {
                ender: "endspaceless".to_string(),
            }
        );
    }

    #[test]
    fn test_parsed_args_operations() {
        let mut args = ParsedArgs::new();

        // Test adding positional args
        args.add_positional(ParsedArg::Literal("test".to_string()));
        assert_eq!(args.positional.len(), 1);
        assert_eq!(args.positional_count(), 1);

        // Test adding named args
        args.add_named("key".to_string(), ParsedArg::String("value".to_string()));
        assert_eq!(args.named.len(), 1);
        assert_eq!(args.named_count(), 1);
        assert!(args.get_named("key").is_some());

        // Test total count
        assert_eq!(args.total_count(), 2);
        assert!(!args.is_empty());

        // Test getters
        assert!(args.get_positional(0).is_some());
        assert!(args.get_positional(1).is_none());
        assert!(args.get_named("key").is_some());
        assert!(args.get_named("nonexistent").is_none());

        // Test named keys
        let keys = args.named_keys();
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&&"key".to_string()));
    }

    #[test]
    fn test_parsed_args_empty() {
        let args = ParsedArgs::new();
        assert!(args.is_empty());
        assert_eq!(args.total_count(), 0);
        assert_eq!(args.positional_count(), 0);
        assert_eq!(args.named_count(), 0);
    }

    #[test]
    fn test_parsed_arg_type_checking() {
        let literal_arg = ParsedArg::Literal("test".to_string());
        assert!(literal_arg.is_literal());
        assert!(!literal_arg.is_variable());
        assert!(!literal_arg.is_string());
        assert!(!literal_arg.is_expression());
        assert!(!literal_arg.is_assignment());

        let string_arg = ParsedArg::String("hello".to_string());
        assert!(string_arg.is_string());
        assert!(!string_arg.is_literal());

        let expr_arg = ParsedArg::Expression("user.name".to_string());
        assert!(expr_arg.is_expression());
        assert!(!expr_arg.is_string());

        let assign_arg = ParsedArg::Assignment {
            name: "key".to_string(),
            value: "value".to_string(),
        };
        assert!(assign_arg.is_assignment());
        assert!(!assign_arg.is_literal());

        // Test assignment getter
        if let Some((name, value)) = assign_arg.as_assignment() {
            assert_eq!(name, "key");
            assert_eq!(value, "value");
        } else {
            panic!("Assignment should return Some");
        }
    }

    #[test]
    fn test_tag_shape_derivation_comprehensive() {
        // Test all possible combinations to ensure TagShape::from_spec works correctly

        // Test unknown tag name (should still work with end_tag)
        let unknown_with_end = TagSpec {
            name: Some("unknown".to_string()),
            end_tag: Some(EndTag {
                name: "endunknown".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: None,
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&unknown_with_end),
            TagShape::Block {
                ender: "endunknown".to_string(),
                has_branches: false,
            }
        );

        // Test with empty intermediate tags (should still be has_branches = true)
        let empty_intermediates = TagSpec {
            name: Some("test".to_string()),
            end_tag: Some(EndTag {
                name: "endtest".to_string(),
                optional: false,
                args: Vec::new(),
            }),
            intermediate_tags: Some(vec![]),
            args: Vec::new(),
        };
        assert_eq!(
            TagShape::from_spec(&empty_intermediates),
            TagShape::Block {
                ender: "endtest".to_string(),
                has_branches: true,
            }
        );
    }
}
