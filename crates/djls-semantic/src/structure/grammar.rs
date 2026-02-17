use rustc_hash::FxHashMap;

/// Role a tag plays in Django's block structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagRole {
    Opener(EndMeta),
    Closer { opener: String },
    Intermediate { possible_openers: Vec<String> },
}

/// Index for tag grammar lookups.
///
/// Uses a single unified map from tag name to [`TagRole`], so every
/// lookup (`classify`, `validate_close`, `is_end_required`) is a single
/// hash probe instead of checking up to three separate maps.
#[salsa::tracked(debug)]
pub struct TagIndex<'db> {
    #[tracked]
    #[returns(ref)]
    roles: FxHashMap<String, TagRole>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndMeta {
    required: bool,
}

impl<'db> TagIndex<'db> {
    pub fn classify(self, db: &'db dyn crate::Db, tag_name: &str) -> TagClass<'db> {
        match self.roles(db).get(tag_name) {
            Some(TagRole::Opener(_)) => TagClass::Opener,
            Some(TagRole::Closer { opener }) => TagClass::Closer {
                opener_name: opener,
            },
            Some(TagRole::Intermediate { possible_openers }) => {
                TagClass::Intermediate { possible_openers }
            }
            None => TagClass::Unknown,
        }
    }

    pub fn is_end_required(self, db: &'db dyn crate::Db, opener_name: &str) -> bool {
        matches!(
            self.roles(db).get(opener_name),
            Some(TagRole::Opener(EndMeta { required: true }))
        )
    }

    pub fn validate_close(
        self,
        db: &'db dyn crate::Db,
        opener_name: &str,
        opener_bits: &[String],
        closer_bits: &[String],
    ) -> CloseValidation {
        if !matches!(self.roles(db).get(opener_name), Some(TagRole::Opener(_))) {
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
        Self::from_tag_specs(db, db.tag_specs())
    }

    /// Build a `TagIndex` from an explicit `TagSpecs` value.
    ///
    /// This is used by tracked queries that compute `TagSpecs` first and then
    /// need to build the index without going through `db.tag_specs()`.
    #[must_use]
    pub fn from_tag_specs(db: &'db dyn crate::Db, specs: &crate::TagSpecs) -> Self {
        let mut roles: FxHashMap<String, TagRole> = FxHashMap::default();

        for (name, spec) in specs {
            if let Some(end_tag) = &spec.end_tag {
                let meta = EndMeta {
                    required: end_tag.required,
                };

                roles.insert(name.clone(), TagRole::Opener(meta));
                roles.insert(
                    end_tag.name.as_ref().to_owned(),
                    TagRole::Closer {
                        opener: name.clone(),
                    },
                );

                for inter in spec.intermediate_tags.iter() {
                    roles
                        .entry(inter.name.as_ref().to_owned())
                        .and_modify(|role| {
                            if let TagRole::Intermediate { possible_openers } = role {
                                possible_openers.push(name.clone());
                            }
                        })
                        .or_insert_with(|| TagRole::Intermediate {
                            possible_openers: vec![name.clone()],
                        });
                }
            }
        }

        TagIndex::new(db, roles)
    }
}

/// Classification of a tag based on its role.
///
/// Borrows data from the [`TagIndex`]'s Salsa-tracked storage, avoiding
/// clones of opener names and possible-opener lists.
#[derive(Clone, Debug)]
pub enum TagClass<'a> {
    /// This tag opens a block
    Opener,
    /// This tag closes a block
    Closer { opener_name: &'a str },
    /// This tag is an intermediate (elif, else, etc.)
    Intermediate { possible_openers: &'a [String] },
    /// Unknown tag - treat as leaf
    Unknown,
}

#[derive(Clone, Debug)]
pub enum CloseValidation {
    Valid,
    NotABlock,
    ArgumentMismatch { expected: String, got: String },
}
