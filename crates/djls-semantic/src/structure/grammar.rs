use djls_templates::TagBit;
use rustc_hash::FxHashMap;

use crate::db::Db;
use crate::tags::TagSpecs;

/// Role a tag plays in Django's block structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TagGrammarRole {
    Opener(EndMeta),
    Closer { opener: String },
    Intermediate { possible_openers: Vec<String> },
}

/// Compute the tag grammar index from tag specifications.
#[salsa::tracked(returns(ref))]
pub fn compute_tag_index(db: &dyn Db) -> TagIndex {
    TagIndex::from_tag_specs(db.tag_specs())
}

/// Index for tag grammar lookups.
///
/// Uses a single unified map from tag name to [`TagGrammarRole`], so every
/// lookup (`classify`, `validate_close`, `is_end_required`) is a single
/// hash probe instead of checking up to three separate maps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagIndex {
    roles: FxHashMap<String, TagGrammarRole>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EndMeta {
    required: bool,
}

impl TagIndex {
    #[must_use]
    pub fn classify(&self, tag_name: &str) -> TagClass<'_> {
        match self.roles.get(tag_name) {
            Some(TagGrammarRole::Opener(_)) => TagClass::Opener,
            Some(TagGrammarRole::Closer { opener }) => TagClass::Closer {
                opener_name: opener.as_str(),
            },
            Some(TagGrammarRole::Intermediate { possible_openers }) => TagClass::Intermediate {
                possible_openers: possible_openers.as_slice(),
            },
            None => TagClass::Unknown,
        }
    }

    pub(crate) fn is_end_required(&self, opener_name: &str) -> bool {
        matches!(
            self.roles.get(opener_name),
            Some(TagGrammarRole::Opener(EndMeta { required: true }))
        )
    }

    pub(crate) fn validate_close(
        &self,
        opener_name: &str,
        opener_bits: &[TagBit],
        closer_bits: &[TagBit],
    ) -> CloseValidation {
        if !matches!(
            self.roles.get(opener_name),
            Some(TagGrammarRole::Opener(_))
        ) {
            return CloseValidation::NotABlock;
        }

        // If the closer supplies a name argument, it must match the opener's.
        // e.g. `{% endblock content %}` must match `{% block content %}`
        if let Some(closer_arg) = closer_bits.first()
            && let Some(opener_arg) = opener_bits.first()
            && closer_arg.as_str() != opener_arg.as_str()
        {
            return CloseValidation::ArgumentMismatch {
                expected: opener_arg.as_str().to_string(),
                got: closer_arg.as_str().to_string(),
            };
        }

        CloseValidation::Valid
    }

    /// Build a `TagIndex` from an explicit `TagSpecs` value.
    #[must_use]
    fn from_tag_specs(specs: &TagSpecs) -> Self {
        let mut roles: FxHashMap<String, TagGrammarRole> = FxHashMap::default();

        for (name, spec) in specs {
            if let Some(end_tag) = &spec.end_tag {
                let meta = EndMeta {
                    required: end_tag.required,
                };

                roles.insert(name.clone(), TagGrammarRole::Opener(meta));
                roles.insert(
                    end_tag.name.as_ref().to_owned(),
                    TagGrammarRole::Closer {
                        opener: name.clone(),
                    },
                );

                for inter in spec.intermediate_tags.iter() {
                    roles
                        .entry(inter.name.as_ref().to_owned())
                        .and_modify(|role| {
                            if let TagGrammarRole::Intermediate { possible_openers } = role {
                                possible_openers.push(name.clone());
                            }
                        })
                        .or_insert_with(|| TagGrammarRole::Intermediate {
                            possible_openers: vec![name.clone()],
                        });
                }
            }
        }

        Self { roles }
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
pub(crate) enum CloseValidation {
    Valid,
    NotABlock,
    ArgumentMismatch { expected: String, got: String },
}
