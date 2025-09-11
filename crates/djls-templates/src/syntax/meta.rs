use crate::db::Db as TemplateDb;
use crate::syntax::args::ParsedArg;
use crate::syntax::args::ParsedArgs;
use crate::syntax::tree::VariableName;
use crate::templatetags::ArgType;
use crate::templatetags::SimpleArgType;
use crate::templatetags::TagSpec;
use crate::templatetags::TagSpecs;
use crate::templatetags::TagType;

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct TagMeta<'db> {
    pub tag_type: TagType,            // From existing TagType::for_name()
    pub shape: TagShape,              // Derived from TagSpec
    pub spec_id: Option<String>,      // Reference to TagSpec name
    pub branch_kind: Option<String>,  // "if"/"elif"/"else" for conditionals
    pub parsed_args: ParsedArgs<'db>, // Parsed according to TagSpec.args
    pub unclosed: bool,               // True if this block was left unclosed
}

impl<'db> TagMeta<'db> {
    /// Create `TagMeta` from tag name and `TagSpecs`
    pub fn from_tag(
        db: &'db dyn TemplateDb,
        name: &str,
        bits: &[String],
        tag_specs: &TagSpecs,
    ) -> Self {
        let tag_type = TagType::for_name(name, tag_specs);

        let (shape, spec_id, branch_kind) = if let Some(spec) = tag_specs.get(name) {
            let shape = TagShape::from_spec(spec);
            let spec_id = Some(name.to_string());

            // Determine branch kind by checking if this tag is an intermediate tag
            let branch_kind = if tag_specs.is_intermediate(name) {
                Some(name.to_string())
            } else {
                None
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
            unclosed: false, // Default to closed, TreeBuilder will mark unclosed blocks
        }
    }

    /// Parse tag arguments according to `TagSpec` definition
    fn parse_arguments(
        db: &'db dyn TemplateDb,
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
                    Some(ArgType::Simple(simple_type)) => match simple_type {
                        SimpleArgType::Literal => ParsedArg::Literal(bit.clone()),
                        SimpleArgType::Variable => {
                            ParsedArg::Variable(VariableName::new(db, bit.clone()))
                        }
                        SimpleArgType::String => ParsedArg::String(bit.clone()),
                        SimpleArgType::Expression | SimpleArgType::VarArgs => {
                            ParsedArg::Expression(bit.clone())
                        }
                        SimpleArgType::Assignment => ParsedArg::Assignment {
                            name: bit.clone(),
                            value: String::new(),
                        },
                    },
                    Some(ArgType::Choice { choice }) => {
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
                // Check if it's a raw block using TagSpec field
                let is_raw = spec.raw_content;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templatetags::EndTag;
    use crate::templatetags::IntermediateTag;

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_tag_shape_from_spec() {
        // Test singleton tag (no end_tag)
        let singleton_spec = TagSpec {
            name: Some("load".to_string()),
            end_tag: None,
            intermediate_tags: None,
            args: Vec::new(),
            raw_content: false,
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
                    args: Vec::new(),
                },
                IntermediateTag {
                    name: "else".to_string(),
                    args: Vec::new(),
                },
            ]),
            args: Vec::new(),
            raw_content: false,
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
            raw_content: false,
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
            raw_content: true,
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
            raw_content: true,
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
            raw_content: true,
        };
        assert_eq!(
            TagShape::from_spec(&spaceless_spec),
            TagShape::RawBlock {
                ender: "endspaceless".to_string(),
            }
        );
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
            raw_content: false,
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
            raw_content: false,
        };
        assert_eq!(
            TagShape::from_spec(&empty_intermediates),
            TagShape::Block {
                ender: "endtest".to_string(),
                has_branches: true,
            }
        );
    }

    #[test]
    fn test_hierarchical_tag_meta() {
        // Test that TagMeta correctly identifies block shapes that can have children
        let block_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Opener,
            shape: TagShape::Block {
                ender: "endfor".to_string(),
                has_branches: false,
            },
            spec_id: Some("for".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        assert!(block_meta.can_have_children());
        assert_eq!(block_meta.expected_closer(), Some("endfor"));

        let singleton_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Standalone,
            shape: TagShape::Singleton,
            spec_id: Some("load".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        assert!(!singleton_meta.can_have_children());
        assert_eq!(singleton_meta.expected_closer(), None);
    }

    #[test]
    fn test_tag_shape_variants() {
        // Test all TagShape variants for child capability
        let block_shape = TagShape::Block {
            ender: "endif".to_string(),
            has_branches: true,
        };

        let raw_shape = TagShape::RawBlock {
            ender: "endcomment".to_string(),
        };

        let singleton_shape = TagShape::Singleton;

        // Create TagMeta instances to test can_have_children
        let block_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Opener,
            shape: block_shape,
            spec_id: Some("if".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        let raw_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Opener,
            shape: raw_shape,
            spec_id: Some("comment".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        let singleton_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Standalone,
            shape: singleton_shape,
            spec_id: Some("load".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        assert!(block_meta.can_have_children());
        assert!(raw_meta.can_have_children());
        assert!(!singleton_meta.can_have_children());
    }

    #[test]
    fn test_hierarchical_structure_concept() {
        // Test the conceptual design of hierarchical structures
        // This tests the data structures without database dependency

        // Simulate an if/elif/else block structure
        let if_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Opener,
            shape: TagShape::Block {
                ender: "endif".to_string(),
                has_branches: true,
            },
            spec_id: Some("if".to_string()),
            branch_kind: None,
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        let elif_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Intermediate,
            shape: TagShape::Singleton, // Intermediate tags are themselves singleton
            spec_id: Some("elif".to_string()),
            branch_kind: Some("elif".to_string()),
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        let else_meta = TagMeta {
            tag_type: crate::templatetags::TagType::Intermediate,
            shape: TagShape::Singleton,
            spec_id: Some("else".to_string()),
            branch_kind: Some("else".to_string()),
            parsed_args: ParsedArgs::new(),
            unclosed: false,
        };

        // Verify the structure properties
        assert!(if_meta.can_have_children());
        assert!(!elif_meta.can_have_children()); // Intermediate tags don't have children themselves
        assert!(!else_meta.can_have_children());

        assert_eq!(if_meta.branch_kind, None);
        assert_eq!(elif_meta.branch_kind, Some("elif".to_string()));
        assert_eq!(else_meta.branch_kind, Some("else".to_string()));
    }
}
