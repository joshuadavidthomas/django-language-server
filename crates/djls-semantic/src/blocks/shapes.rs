use rustc_hash::FxHashMap;

use crate::templatetags::IntermediateTag;
use crate::templatetags::TagArg;
use crate::templatetags::TagSpec;
use crate::templatetags::TagSpecs;
use crate::EndTag;
use djls_templates::nodelist::TagBit;

/// Collection of tag shapes with pre-computed indices for O(1) lookups
#[derive(Clone, Debug)]
pub struct TagShapes {
    /// Primary shape storage
    shapes: FxHashMap<String, TagShape>,
    /// Pre-computed closer -> opener mapping
    closer_to_opener: FxHashMap<String, String>,
    /// Pre-computed intermediate -> [openers] mapping
    intermediate_to_openers: FxHashMap<String, Vec<String>>,
}

impl TagShapes {
    /// Get a shape by opener name
    pub fn get(&self, opener_name: &str) -> Option<&TagShape> {
        self.shapes.get(opener_name)
    }

    /// What kind of tag is this? O(1) lookup
    pub fn classify(&self, tag_name: &str) -> TagClass {
        if let Some(shape) = self.shapes.get(tag_name) {
            return TagClass::Opener {
                shape: shape.clone(),
            };
        }
        if let Some(opener) = self.closer_to_opener.get(tag_name) {
            return TagClass::Closer {
                opener_name: opener.clone(),
            };
        }
        if let Some(openers) = self.intermediate_to_openers.get(tag_name) {
            return TagClass::Intermediate {
                possible_openers: openers.clone(),
            };
        }
        TagClass::Unknown
    }

    /// Validate a close tag against its opener
    pub fn validate_close<'db>(
        &self,
        opener_name: &str,
        opener_bits: &[TagBit<'db>],
        closer_bits: &[TagBit<'db>],
        db: &'db dyn crate::db::Db,
    ) -> CloseValidation {
        let Some(shape) = self.shapes.get(opener_name) else {
            return CloseValidation::NotABlock;
        };

        match shape {
            TagShape::Block { end, .. } => {
                // No args to match? Simple close
                if end.match_args.is_empty() {
                    return CloseValidation::Valid;
                }

                // Validate each arg that should match
                for match_arg in &end.match_args {
                    let opener_val = extract_arg_value(opener_bits, match_arg, db);
                    let closer_val = extract_arg_value(closer_bits, match_arg, db);

                    match (opener_val, closer_val, match_arg.required) {
                        (Some(o), Some(c), _) if o != c => {
                            return CloseValidation::ArgumentMismatch {
                                arg: match_arg.name.clone(),
                                expected: o,
                                got: c,
                            };
                        }
                        (Some(o), None, true) => {
                            return CloseValidation::MissingRequiredArg {
                                arg: match_arg.name.clone(),
                                expected: o,
                            };
                        }
                        (None, Some(c), _) if match_arg.required => {
                            return CloseValidation::UnexpectedArg {
                                arg: match_arg.name.clone(),
                                got: c,
                            };
                        }
                        _ => continue,
                    }
                }
                CloseValidation::Valid
            }
            TagShape::Leaf { .. } => CloseValidation::NotABlock,
        }
    }

    /// Can this intermediate appear in the current context?
    pub fn is_valid_intermediate(&self, inter_name: &str, opener_name: &str) -> bool {
        self.intermediate_to_openers
            .get(inter_name)
            .is_some_and(|openers| openers.contains(&opener_name.to_string()))
    }
}

impl From<&TagSpecs> for TagShapes {
    fn from(specs: &TagSpecs) -> Self {
        let mut shapes = FxHashMap::default();
        let mut closer_to_opener = FxHashMap::default();
        let mut intermediate_to_openers: FxHashMap<String, Vec<String>> = FxHashMap::default();

        for (name, spec) in specs {
            let shape = TagShape::from((name.as_str(), spec));

            // Build reverse indices
            match &shape {
                TagShape::Block {
                    end, intermediates, ..
                } => {
                    // Map closer -> opener
                    closer_to_opener.insert(end.name.to_string(), name.to_string());

                    // Map each intermediate -> [openers that allow it]
                    for inter in intermediates {
                        intermediate_to_openers
                            .entry(inter.name.clone())
                            .or_default()
                            .push(name.to_string());
                    }
                }
                TagShape::Leaf { .. } => {}
            }

            shapes.insert(name.to_string(), shape);
        }

        TagShapes {
            shapes,
            closer_to_opener,
            intermediate_to_openers,
        }
    }
}

/// Shape of a template tag
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagShape {
    /// A standalone tag with no body
    Leaf { name: String },
    /// A block tag that contains a body
    Block {
        name: String,
        end: EndShape,
        intermediates: Vec<IntermediateShape>,
    },
}

impl From<(&str, &TagSpec)> for TagShape {
    fn from((name, spec): (&str, &TagSpec)) -> Self {
        match &spec.end_tag {
            None => TagShape::Leaf {
                name: name.to_string(),
            },
            Some(end) => TagShape::Block {
                name: name.to_string(),
                end: end.into(),
                intermediates: spec
                    .intermediate_tags
                    .iter()
                    .map(IntermediateShape::from)
                    .collect(),
            },
        }
    }
}

/// Shape of an end tag
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndShape {
    pub name: String,
    pub optional: bool,
    pub match_args: Vec<MatchArg>,
}

impl From<&EndTag> for EndShape {
    fn from(end: &EndTag) -> Self {
        let match_args = end
            .args
            .iter()
            .enumerate()
            .map(|(i, arg)| MatchArg {
                name: arg.name().as_ref().to_owned(),
                arg_type: ArgType::from(arg),
                required: arg.is_required(),
                opener_position: Some(i),
            })
            .collect();

        EndShape {
            name: end.name.as_ref().to_owned(),
            optional: end.optional,
            match_args,
        }
    }
}

/// An argument that needs to match between opener and closer
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArg {
    pub name: String,
    pub arg_type: ArgType,
    pub required: bool,
    pub opener_position: Option<usize>,
}

/// Type of an argument
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgType {
    Variable,
    String,
    Literal,
    Expression,
    Assignment,
    VarArgs,
    Choice(Vec<String>),
}

impl From<&TagArg> for ArgType {
    fn from(arg: &TagArg) -> Self {
        match arg {
            TagArg::Var { .. } => ArgType::Variable,
            TagArg::String { .. } => ArgType::String,
            TagArg::Literal { .. } => ArgType::Literal,
            TagArg::Expr { .. } => ArgType::Expression,
            TagArg::Assignment { .. } => ArgType::Assignment,
            TagArg::VarArgs { .. } => ArgType::VarArgs,
            TagArg::Choice { choices, .. } => {
                ArgType::Choice(choices.iter().map(|s| s.as_ref().to_owned()).collect())
            }
        }
    }
}

/// Shape of an intermediate tag
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntermediateShape {
    name: String,
    args: Vec<ArgShape>,
}

impl IntermediateShape {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl From<&IntermediateTag> for IntermediateShape {
    fn from(tag: &IntermediateTag) -> Self {
        IntermediateShape {
            name: tag.name.as_ref().to_owned(),
            args: tag.args.iter().map(ArgShape::from).collect(),
        }
    }
}

/// Shape of a tag argument
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgShape {
    name: String,
    required: bool,
}

impl From<&TagArg> for ArgShape {
    fn from(arg: &TagArg) -> Self {
        ArgShape {
            name: arg.name().as_ref().to_owned(),
            required: arg.is_required(),
        }
    }
}

/// Classification of a tag based on its role
#[derive(Clone, Debug)]
pub enum TagClass {
    /// This tag opens a block
    Opener { shape: TagShape },
    /// This tag closes a block
    Closer { opener_name: String },
    /// This tag is an intermediate (elif, else, etc.)
    Intermediate { possible_openers: Vec<String> },
    /// Unknown tag - treat as leaf
    Unknown,
}

/// Result of validating a close tag
#[derive(Clone, Debug)]
pub enum CloseValidation {
    /// Close is valid
    Valid,
    /// Not a block tag
    NotABlock,
    /// Argument value mismatch
    ArgumentMismatch {
        arg: String,
        expected: String,
        got: String,
    },
    /// Missing required argument
    MissingRequiredArg { arg: String, expected: String },
    /// Unexpected argument provided
    UnexpectedArg { arg: String, got: String },
}

/// Extract argument value from tag bits
fn extract_arg_value<'db>(
    bits: &[TagBit<'db>],
    match_arg: &MatchArg,
    db: &'db dyn crate::db::Db,
) -> Option<String> {
    // For now, use position-based matching if available
    // In the future, we might want to parse the bits more intelligently
    // to match by name rather than position
    match match_arg.opener_position {
        Some(pos) if pos < bits.len() => Some(bits[pos].text(db).to_string()),
        _ => None,
    }
}
