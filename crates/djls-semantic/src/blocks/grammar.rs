use rustc_hash::FxHashMap;

use crate::templatetags::TagSpecs;
use djls_templates::nodelist::TagBit;

/// Index for O(1) tag grammar lookups
#[derive(Clone, Debug)]
pub struct TagIndex {
    /// Opener tags and their end tag metadata
    openers: FxHashMap<String, EndMeta>,
    /// Map from closer tag name to opener tag name
    closers: FxHashMap<String, String>,
    /// Map from intermediate tag name to list of possible opener tags
    intermediate_to_openers: FxHashMap<String, Vec<String>>,
}

/// Metadata about an end tag
#[derive(Clone, Debug)]
struct EndMeta {
    optional: bool,
    match_args: Vec<MatchArgSpec>,
}

/// Specification for matching arguments between opener and closer
#[derive(Clone, Debug)]
struct MatchArgSpec {
    name: String,
    required: bool,
    position: usize,
}

impl TagIndex {
    /// Classify a tag by name
    pub fn classify(&self, tag_name: &str) -> TagClass {
        if self.openers.contains_key(tag_name) {
            return TagClass::Opener;
        }
        if let Some(opener) = self.closers.get(tag_name) {
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

    /// Check if an opener's end tag is optional
    pub fn is_end_optional(&self, opener_name: &str) -> bool {
        self.openers
            .get(opener_name)
            .is_some_and(|meta| meta.optional)
    }

    /// Validate a close tag against its opener
    pub fn validate_close<'db>(
        &self,
        opener_name: &str,
        opener_bits: &[TagBit<'db>],
        closer_bits: &[TagBit<'db>],
        db: &'db dyn crate::db::Db,
    ) -> CloseValidation {
        let Some(meta) = self.openers.get(opener_name) else {
            return CloseValidation::NotABlock;
        };

        // No args to match? Simple close
        if meta.match_args.is_empty() {
            return CloseValidation::Valid;
        }

        // Validate each arg that should match
        for match_arg in &meta.match_args {
            let opener_val = extract_arg_value(opener_bits, match_arg.position, db);
            let closer_val = extract_arg_value(closer_bits, match_arg.position, db);

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
                _ => {}
            }
        }
        CloseValidation::Valid
    }

    /// Check if an intermediate tag is valid in the current context
    #[allow(dead_code)] // TODO: is this still needed?
    pub fn is_valid_intermediate(&self, inter_name: &str, opener_name: &str) -> bool {
        self.intermediate_to_openers
            .get(inter_name)
            .is_some_and(|openers| openers.iter().any(|o| o == opener_name))
    }
}

impl From<&TagSpecs> for TagIndex {
    fn from(specs: &TagSpecs) -> Self {
        let mut openers = FxHashMap::default();
        let mut closers = FxHashMap::default();
        let mut intermediate_to_openers: FxHashMap<String, Vec<String>> = FxHashMap::default();

        for (name, spec) in specs {
            if let Some(end_tag) = &spec.end_tag {
                // Build EndMeta for this opener
                let match_args = end_tag
                    .args
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| MatchArgSpec {
                        name: arg.name().as_ref().to_owned(),
                        required: arg.is_required(),
                        position: i,
                    })
                    .collect();

                let meta = EndMeta {
                    optional: end_tag.optional,
                    match_args,
                };

                // Map opener -> meta
                openers.insert(name.clone(), meta);

                // Map closer -> opener
                closers.insert(end_tag.name.as_ref().to_owned(), name.clone());

                // Map intermediates -> opener
                for inter in spec.intermediate_tags.iter() {
                    intermediate_to_openers
                        .entry(inter.name.as_ref().to_owned())
                        .or_default()
                        .push(name.clone());
                }
            }
        }

        TagIndex {
            openers,
            closers,
            intermediate_to_openers,
        }
    }
}

/// Classification of a tag based on its role
#[derive(Clone, Debug)]
pub enum TagClass {
    /// This tag opens a block
    Opener,
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

/// Extract argument value from tag bits by position
fn extract_arg_value<'db>(
    bits: &[TagBit<'db>],
    position: usize,
    db: &'db dyn crate::db::Db,
) -> Option<String> {
    if position < bits.len() {
        Some(bits[position].text(db).to_string())
    } else {
        None
    }
}

