use rustc_hash::FxHashMap;

/// Index for tag grammar lookups
#[salsa::tracked(debug)]
pub struct TagIndex<'db> {
    /// Opener tags and their end tag metadata
    #[tracked]
    #[returns(ref)]
    openers: FxHashMap<String, EndMeta>,
    /// Map from closer tag name to opener tag name
    #[tracked]
    #[returns(ref)]
    closers: FxHashMap<String, String>,
    /// Map from intermediate tag name to list of possible opener tags
    #[tracked]
    #[returns(ref)]
    intermediate_to_openers: FxHashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndMeta {
    optional: bool,
    match_args: Vec<MatchArgSpec>,
}

/// Specification for matching arguments between opener and closer
#[derive(Clone, Debug, PartialEq, Eq)]
struct MatchArgSpec {
    name: String,
    required: bool,
    position: usize,
}

impl<'db> TagIndex<'db> {
    pub fn classify(self, db: &'db dyn crate::Db, tag_name: &str) -> TagClass {
        if self.openers(db).contains_key(tag_name) {
            return TagClass::Opener;
        }
        if let Some(opener) = self.closers(db).get(tag_name) {
            return TagClass::Closer {
                opener_name: opener.clone(),
            };
        }
        if let Some(openers) = self.intermediate_to_openers(db).get(tag_name) {
            return TagClass::Intermediate {
                possible_openers: openers.clone(),
            };
        }
        TagClass::Unknown
    }

    pub fn is_end_optional(self, db: &'db dyn crate::Db, opener_name: &str) -> bool {
        self.openers(db)
            .get(opener_name)
            .is_some_and(|meta| meta.optional)
    }

    pub fn validate_close(
        self,
        db: &'db dyn crate::Db,
        opener_name: &str,
        opener_bits: &[String],
        closer_bits: &[String],
    ) -> CloseValidation {
        let Some(meta) = self.openers(db).get(opener_name) else {
            return CloseValidation::NotABlock;
        };

        // No args to match? Simple close
        if meta.match_args.is_empty() {
            return CloseValidation::Valid;
        }

        for match_arg in &meta.match_args {
            let opener_val = extract_arg_value(opener_bits, match_arg.position);
            let closer_val = extract_arg_value(closer_bits, match_arg.position);

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

    #[allow(dead_code)] // TODO: is this still needed?
    pub fn is_valid_intermediate(
        self,
        db: &'db dyn crate::Db,
        inter_name: &str,
        opener_name: &str,
    ) -> bool {
        self.intermediate_to_openers(db)
            .get(inter_name)
            .is_some_and(|openers| openers.iter().any(|o| o == opener_name))
    }
    #[must_use]
    pub fn from_specs(db: &'db dyn crate::Db) -> Self {
        let mut openers = FxHashMap::default();
        let mut closers = FxHashMap::default();
        let mut intermediate_to_openers: FxHashMap<String, Vec<String>> = FxHashMap::default();

        for (name, spec) in db.tag_specs() {
            if let Some(end_tag) = &spec.end_tag {
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

                // opener -> meta
                openers.insert(name.clone(), meta);
                // closer -> opener
                closers.insert(end_tag.name.as_ref().to_owned(), name.clone());
                // intermediates -> opener
                for inter in spec.intermediate_tags.iter() {
                    intermediate_to_openers
                        .entry(inter.name.as_ref().to_owned())
                        .or_default()
                        .push(name.clone());
                }
            }
        }

        TagIndex::new(db, openers, closers, intermediate_to_openers)
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

#[derive(Clone, Debug)]
pub enum CloseValidation {
    Valid,
    NotABlock,
    ArgumentMismatch {
        arg: String,
        expected: String,
        got: String,
    },
    MissingRequiredArg {
        arg: String,
        expected: String,
    },
    UnexpectedArg {
        arg: String,
        got: String,
    },
}

fn extract_arg_value(bits: &[String], position: usize) -> Option<String> {
    if position < bits.len() {
        Some(bits[position].clone())
    } else {
        None
    }
}
