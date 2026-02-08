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
    required: bool,
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

    pub fn is_end_required(self, db: &'db dyn crate::Db, opener_name: &str) -> bool {
        self.openers(db)
            .get(opener_name)
            .is_some_and(|meta| meta.required)
    }

    pub fn validate_close(
        self,
        db: &'db dyn crate::Db,
        opener_name: &str,
        opener_bits: &[String],
        closer_bits: &[String],
    ) -> CloseValidation {
        if !self.openers(db).contains_key(opener_name) {
            return CloseValidation::NotABlock;
        }

        // If the closer supplies a name argument, it must match the opener's.
        // e.g. `{% endblock content %}` must match `{% block content %}`
        if let Some(closer_arg) = closer_bits.first() {
            if let Some(opener_arg) = opener_bits.first() {
                if closer_arg != opener_arg {
                    return CloseValidation::ArgumentMismatch {
                        expected: opener_arg.clone(),
                        got: closer_arg.clone(),
                    };
                }
            }
        }

        CloseValidation::Valid
    }

    #[must_use]
    pub fn from_specs(db: &'db dyn crate::Db) -> Self {
        Self::from_tag_specs(db, &db.tag_specs())
    }

    /// Build a `TagIndex` from an explicit `TagSpecs` value.
    ///
    /// This is used by tracked queries that compute `TagSpecs` first and then
    /// need to build the index without going through `db.tag_specs()`.
    #[must_use]
    pub fn from_tag_specs(db: &'db dyn crate::Db, specs: &crate::TagSpecs) -> Self {
        let mut openers = FxHashMap::default();
        let mut closers = FxHashMap::default();
        let mut intermediate_to_openers: FxHashMap<String, Vec<String>> = FxHashMap::default();

        for (name, spec) in specs {
            if let Some(end_tag) = &spec.end_tag {
                let meta = EndMeta {
                    required: end_tag.required,
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
    ArgumentMismatch { expected: String, got: String },
}
