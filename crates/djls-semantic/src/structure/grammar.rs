use std::collections::BTreeMap;

use djls_project::Project;
use djls_project::TemplateLibraryKey;
use djls_project::template_libraries;
use djls_templates::TagBit;
use rustc_hash::FxHashMap;

use crate::db::Db;
use crate::tags::TagSpecs;
use crate::tags::library_tag_specs;

/// Identity of an opening Tag Definition contributing semantic grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrammarOpeningDefinition {
    library: TemplateLibraryKey,
    name: String,
}

impl GrammarOpeningDefinition {
    #[must_use]
    pub fn library(&self) -> &TemplateLibraryKey {
        &self.library
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Project vocabulary for orphan closer/intermediate candidate discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticGrammarVocabulary {
    closers: BTreeMap<String, Vec<GrammarOpeningDefinition>>,
    intermediates: BTreeMap<String, Vec<GrammarOpeningDefinition>>,
    open: bool,
}

impl SemanticGrammarVocabulary {
    #[must_use]
    pub fn closer_candidates(&self, name: &str) -> &[GrammarOpeningDefinition] {
        self.closers.get(name).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn intermediate_candidates(&self, name: &str) -> &[GrammarOpeningDefinition] {
        self.intermediates.get(name).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Build the cheap spelling-to-opening-identity vocabulary for a Project.
#[salsa::tracked(returns(ref))]
pub fn semantic_grammar_vocabulary(db: &dyn Db, project: Project) -> SemanticGrammarVocabulary {
    let libraries = template_libraries(db, project);
    let mut vocabulary = SemanticGrammarVocabulary {
        open: libraries.definition_names_are_open(),
        ..SemanticGrammarVocabulary::default()
    };
    for library in libraries.resolved_libraries() {
        let specs = library_tag_specs(db, project, library.key(db));
        for (name, spec) in specs.iter() {
            if library
                .symbol(djls_project::TemplateSymbolKind::Tag, name)
                .is_none()
                && !library.symbol_inventory_is_open()
            {
                continue;
            }
            let Some(end_tag) = &spec.end_tag else {
                continue;
            };
            let definition = GrammarOpeningDefinition {
                library: library.key(db),
                name: name.clone(),
            };
            let closer_candidates = vocabulary
                .closers
                .entry(end_tag.name.as_ref().to_string())
                .or_default();
            if !closer_candidates.contains(&definition) {
                closer_candidates.push(definition.clone());
            }
            if !spec.opaque {
                for intermediate in spec.intermediate_tags.iter() {
                    let candidates = vocabulary
                        .intermediates
                        .entry(intermediate.name.as_ref().to_string())
                        .or_default();
                    if !candidates.contains(&definition) {
                        candidates.push(definition.clone());
                    }
                }
            }
        }
    }
    vocabulary
}

#[salsa::tracked(returns(ref))]
pub(crate) fn compute_preliminary_tag_index_for_file(
    db: &dyn Db,
    file: djls_source::File,
    scope_file: djls_source::File,
) -> ScopedTagIndex {
    let empty = crate::scoping::LoadedLibraries::default();
    let specs = match db.project() {
        Some(project) => crate::tags::effective_tag_specs_for_load_state_in_project_scope(
            db,
            project,
            scope_file,
            &empty.available_at(0),
        ),
        None => crate::tags::effective_tag_specs_for_load_state(db, file, &empty.available_at(0)),
    };
    ScopedTagIndex::single(TagIndex::from_tag_specs(specs))
}

pub(crate) fn scoped_tag_index_for_known_loads(
    db: &dyn Db,
    file: djls_source::File,
    scope_file: djls_source::File,
    loaded: &crate::scoping::LoadedLibraries,
) -> ScopedTagIndex {
    let Some(project) = db.project() else {
        return scoped_tag_index_for_loads(db, file, loaded);
    };
    let initial = TagIndex::from_tag_specs(
        crate::tags::effective_tag_specs_for_load_state_in_project_scope(
            db,
            project,
            scope_file,
            &crate::scoping::LoadedLibraries::default().available_at(0),
        ),
    );
    let boundaries = loaded
        .statements()
        .iter()
        .map(|statement| {
            let position = statement.span().end();
            (
                position,
                TagIndex::from_tag_specs(
                    crate::tags::effective_tag_specs_for_load_state_in_project_scope(
                        db,
                        project,
                        scope_file,
                        &loaded.available_at(position),
                    ),
                ),
            )
        })
        .collect();
    ScopedTagIndex {
        initial,
        boundaries,
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn compute_tag_index_for_file(
    db: &dyn Db,
    file: djls_source::File,
    nodelist: djls_templates::NodeList<'_>,
) -> ScopedTagIndex {
    scoped_tag_index_for_loads(
        db,
        file,
        crate::scoping::compute_loaded_libraries_for_file(db, file, nodelist),
    )
}

fn scoped_tag_index_for_loads(
    db: &dyn Db,
    file: djls_source::File,
    loaded: &crate::scoping::LoadedLibraries,
) -> ScopedTagIndex {
    let initial = TagIndex::from_tag_specs(crate::tags::tag_specs_for_file(db, file).clone());
    let boundaries = loaded
        .statements()
        .iter()
        .map(|statement| {
            let position = statement.span().end();
            (
                position,
                TagIndex::from_tag_specs(crate::tags::effective_tag_specs_for_load_state(
                    db,
                    file,
                    &loaded.available_at(position),
                )),
            )
        })
        .collect();
    ScopedTagIndex {
        initial,
        boundaries,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedTagIndex {
    initial: TagIndex,
    boundaries: Vec<(u32, TagIndex)>,
}

impl ScopedTagIndex {
    fn single(index: TagIndex) -> Self {
        Self {
            initial: index,
            boundaries: Vec::new(),
        }
    }

    pub(crate) fn at(&self, position: u32) -> &TagIndex {
        let index = self
            .boundaries
            .partition_point(|(boundary, _)| *boundary <= position);
        if index == 0 {
            &self.initial
        } else {
            &self.boundaries[index - 1].1
        }
    }
}

/// Index for tag grammar lookups.
#[derive(Clone, Debug, PartialEq)]
pub struct TagIndex {
    specs: TagSpecs,
    openers: FxHashMap<String, OpenerMeta>,
    closers: FxHashMap<String, Vec<String>>,
    intermediates: FxHashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenerMeta {
    required: bool,
    opaque: bool,
    closer: String,
}

impl TagIndex {
    #[must_use]
    pub fn classify(&self, tag_name: &str) -> TagClass<'_> {
        if self.openers.contains_key(tag_name) {
            TagClass::Opener
        } else if self.specs.contains_key(tag_name) {
            TagClass::Standalone
        } else if let Some(possible_openers) = self.closers.get(tag_name) {
            TagClass::Closer {
                possible_openers: possible_openers.as_slice(),
            }
        } else if let Some(possible_openers) = self.intermediates.get(tag_name) {
            TagClass::Intermediate {
                possible_openers: possible_openers.as_slice(),
            }
        } else {
            TagClass::Unknown
        }
    }

    pub(crate) fn intermediate_names(&self, opener_name: &str) -> Vec<String> {
        self.intermediates
            .iter()
            .filter(|(_, openers)| openers.iter().any(|opener| opener == opener_name))
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) fn is_end_required(&self, opener_name: &str) -> bool {
        matches!(
            self.openers.get(opener_name),
            Some(OpenerMeta { required: true, .. })
        )
    }

    #[must_use]
    pub fn is_opaque(&self, opener_name: &str) -> bool {
        matches!(
            self.openers.get(opener_name),
            Some(OpenerMeta { opaque: true, .. })
        )
    }

    #[must_use]
    pub fn closer_name(&self, opener_name: &str) -> Option<&str> {
        self.openers
            .get(opener_name)
            .map(|OpenerMeta { closer, .. }| closer.as_str())
    }

    pub(crate) fn spec(&self, tag_name: &str) -> Option<&crate::TagSpec> {
        self.specs.get(tag_name)
    }

    pub(crate) fn validate_close(
        &self,
        opener_name: &str,
        opener_bits: &[TagBit],
        closer_bits: &[TagBit],
    ) -> CloseValidation {
        if !self.openers.contains_key(opener_name) {
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
                got_span: closer_arg.span,
            };
        }

        CloseValidation::Valid
    }

    /// Build a `TagIndex` from an explicit `TagSpecs` value.
    #[must_use]
    fn from_tag_specs(specs: impl std::borrow::Borrow<TagSpecs>) -> Self {
        let specs = specs.borrow();
        let mut openers: FxHashMap<String, OpenerMeta> = FxHashMap::default();
        let mut closers: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let mut intermediates: FxHashMap<String, Vec<String>> = FxHashMap::default();

        for (name, spec) in specs {
            if let Some(end_tag) = &spec.end_tag {
                let closer = end_tag.name.as_ref().to_owned();
                let meta = OpenerMeta {
                    required: end_tag.required,
                    opaque: spec.opaque,
                    closer: closer.clone(),
                };

                openers.insert(name.clone(), meta);
                closers
                    .entry(closer)
                    .and_modify(|possible_openers| possible_openers.push(name.clone()))
                    .or_insert_with(|| vec![name.clone()]);

                if !spec.opaque {
                    for inter in spec.intermediate_tags.iter() {
                        intermediates
                            .entry(inter.name.as_ref().to_owned())
                            .and_modify(|possible_openers| possible_openers.push(name.clone()))
                            .or_insert_with(|| vec![name.clone()]);
                    }
                }
            }
        }

        for possible_openers in closers.values_mut().chain(intermediates.values_mut()) {
            possible_openers.sort();
            possible_openers.dedup();
        }

        Self {
            specs: specs.clone(),
            openers,
            closers,
            intermediates,
        }
    }
}

/// Classification of a tag based on its role.
///
/// Borrows data from the [`TagIndex`]'s Salsa-tracked storage, avoiding
/// clones of opener names and possible-opener lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagClass<'a> {
    /// This tag opens a block
    Opener,
    /// This tag is an effective standalone definition at this position.
    Standalone,
    /// This tag closes one or more blocks
    Closer { possible_openers: &'a [String] },
    /// This tag is an intermediate (elif, else, etc.)
    Intermediate { possible_openers: &'a [String] },
    /// Unknown tag - treat as leaf
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) enum CloseValidation {
    Valid,
    NotABlock,
    ArgumentMismatch {
        expected: String,
        got: String,
        got_span: djls_source::Span,
    },
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use rustc_hash::FxHashMap;

    use super::*;
    use crate::tags::EndTag;
    use crate::tags::IntermediateTag;
    use crate::tags::TagSpec;
    use crate::tags::TagSpecs;

    fn create_test_specs() -> TagSpecs {
        let mut specs = FxHashMap::default();

        let block = |end_tag: &'static str, intermediates: Vec<&'static str>| {
            let intermediate_tags: Cow<'static, [IntermediateTag]> = if intermediates.is_empty() {
                Cow::Borrowed(&[])
            } else {
                Cow::Owned(
                    intermediates
                        .into_iter()
                        .map(|name| IntermediateTag { name: name.into() })
                        .collect(),
                )
            };

            TagSpec::new(
                "django.template.defaulttags".into(),
                Some(EndTag {
                    name: end_tag.into(),
                    required: true,
                }),
                intermediate_tags,
                false,
            )
        };

        specs.insert(
            "csrf_token".to_string(),
            TagSpec::new(
                "django.template.defaulttags".into(),
                None,
                Cow::Borrowed(&[]),
                false,
            ),
        );
        specs.insert("if".to_string(), block("endif", vec!["elif", "else"]));
        specs.insert("for".to_string(), block("endfor", vec!["empty", "else"]));
        specs.insert("block".to_string(), block("endblock", vec![]));

        TagSpecs::new(specs)
    }

    #[test]
    fn classifies_opening_tags() {
        let specs = create_test_specs();
        let index = TagIndex::from_tag_specs(&specs);

        assert_eq!(index.classify("if"), TagClass::Opener);
        assert_eq!(index.classify("for"), TagClass::Opener);
        assert_eq!(index.classify("block"), TagClass::Opener);
    }

    #[test]
    fn classifies_closing_tags_with_their_openers() {
        let specs = create_test_specs();
        let index = TagIndex::from_tag_specs(&specs);

        match index.classify("endif") {
            TagClass::Closer { possible_openers } => assert_eq!(possible_openers, ["if"]),
            tag_class => panic!("expected endif to classify as closer, got {tag_class:?}"),
        }
        match index.classify("endfor") {
            TagClass::Closer { possible_openers } => assert_eq!(possible_openers, ["for"]),
            tag_class => panic!("expected endfor to classify as closer, got {tag_class:?}"),
        }
        match index.classify("endblock") {
            TagClass::Closer { possible_openers } => assert_eq!(possible_openers, ["block"]),
            tag_class => panic!("expected endblock to classify as closer, got {tag_class:?}"),
        }
        assert_eq!(index.classify("endnonexistent"), TagClass::Unknown);
    }

    #[test]
    fn classifies_intermediate_tags_with_possible_openers() {
        let specs = create_test_specs();
        let index = TagIndex::from_tag_specs(&specs);

        match index.classify("elif") {
            TagClass::Intermediate { possible_openers } => assert_eq!(possible_openers, ["if"]),
            tag_class => panic!("expected elif to classify as intermediate, got {tag_class:?}"),
        }

        match index.classify("else") {
            TagClass::Intermediate { possible_openers } => {
                let mut possible_openers = possible_openers.to_vec();
                possible_openers.sort();
                assert_eq!(possible_openers, ["for", "if"]);
            }
            tag_class => panic!("expected else to classify as intermediate, got {tag_class:?}"),
        }

        match index.classify("empty") {
            TagClass::Intermediate { possible_openers } => assert_eq!(possible_openers, ["for"]),
            tag_class => panic!("expected empty to classify as intermediate, got {tag_class:?}"),
        }
    }

    #[test]
    fn distinguishes_standalone_and_unknown_tags() {
        let specs = create_test_specs();
        let index = TagIndex::from_tag_specs(&specs);

        assert_eq!(index.classify("csrf_token"), TagClass::Standalone);
        assert_eq!(index.classify("nonexistent"), TagClass::Unknown);
    }

    #[test]
    fn tracks_required_end_tags() {
        let specs = create_test_specs();
        let index = TagIndex::from_tag_specs(&specs);

        assert!(index.is_end_required("if"));
        assert!(index.is_end_required("for"));
        assert!(index.is_end_required("block"));
        assert!(!index.is_end_required("csrf_token"));
        assert!(!index.is_end_required("nonexistent"));
    }

    #[test]
    fn tracks_opaque_openers() {
        let mut specs = FxHashMap::default();
        specs.insert(
            "opaque_if".to_string(),
            TagSpec::new(
                "test".into(),
                Some(EndTag {
                    name: "endopaque_if".into(),
                    required: true,
                }),
                vec![IntermediateTag {
                    name: "opaque_else".into(),
                }]
                .into(),
                true,
            ),
        );

        let index = TagIndex::from_tag_specs(TagSpecs::new(specs));

        assert_eq!(index.classify("opaque_if"), TagClass::Opener);
        assert_eq!(index.classify("opaque_else"), TagClass::Unknown);
        assert!(index.is_opaque("opaque_if"));
    }

    #[test]
    fn opaque_openers_do_not_contribute_shared_intermediates() {
        let mut specs = FxHashMap::default();
        specs.insert(
            "opaque_if".to_string(),
            TagSpec::new(
                "test".into(),
                Some(EndTag {
                    name: "endopaque_if".into(),
                    required: true,
                }),
                vec![IntermediateTag {
                    name: "shared_else".into(),
                }]
                .into(),
                true,
            ),
        );
        specs.insert(
            "plain_if".to_string(),
            TagSpec::new(
                "test".into(),
                Some(EndTag {
                    name: "endplain_if".into(),
                    required: true,
                }),
                vec![IntermediateTag {
                    name: "shared_else".into(),
                }]
                .into(),
                false,
            ),
        );

        let index = TagIndex::from_tag_specs(TagSpecs::new(specs));

        match index.classify("shared_else") {
            TagClass::Intermediate { possible_openers } => {
                assert_eq!(possible_openers, ["plain_if"]);
            }
            tag_class => {
                panic!("expected shared_else to classify as intermediate, got {tag_class:?}")
            }
        }
    }
}
