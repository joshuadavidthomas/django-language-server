use std::collections::BTreeMap;

use djls_project::EffectiveDefinitionLibrary;
use djls_project::Project;
use djls_project::TemplateEnvironment;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolKind;
use djls_project::template_environment;
use djls_project::template_libraries;
use djls_source::File;
use djls_source::Span;
use djls_templates::Node;
use djls_templates::NodeList;
use djls_templates::TagBit;

use crate::db::Db;
use crate::scoping::LoadedLibraries;
use crate::tags::TagSpec;
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

/// Project vocabulary used only to prime orphan closer/intermediate candidates.
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
    let environment = TemplateEnvironment::from_project_inventory(libraries);
    let mut vocabulary = SemanticGrammarVocabulary {
        open: environment.definition_names_are_open(),
        ..SemanticGrammarVocabulary::default()
    };
    for library in environment.resolved_libraries() {
        let specs = library_tag_specs(db, project, library.key(db));
        for (name, spec) in specs.iter() {
            if library.symbol(TemplateSymbolKind::Tag, name).is_none()
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
            push_candidate(
                vocabulary
                    .closers
                    .entry(end_tag.name.as_ref().to_string())
                    .or_default(),
                definition.clone(),
            );
            if !spec.opaque {
                for intermediate in spec.intermediate_tags.iter() {
                    push_candidate(
                        vocabulary
                            .intermediates
                            .entry(intermediate.name.as_ref().to_string())
                            .or_default(),
                        definition.clone(),
                    );
                }
            }
        }
    }
    vocabulary
}

fn push_candidate(
    candidates: &mut Vec<GrammarOpeningDefinition>,
    candidate: GrammarOpeningDefinition,
) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

/// The structural contract captured when an opening occurrence is classified.
///
/// Frames retain this value for their entire lifetime. A later load can therefore
/// change later occurrences without rewriting a Branch that is already open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpeningContract {
    pub(crate) closer: String,
    pub(crate) intermediates: Vec<String>,
    pub(crate) end_required: bool,
    pub(crate) opaque: bool,
}

impl OpeningContract {
    fn from_spec(spec: &TagSpec) -> Option<Self> {
        let end = spec.end_tag.as_ref()?;
        Some(Self {
            closer: end.name.as_ref().to_string(),
            intermediates: if spec.opaque {
                Vec::new()
            } else {
                spec.intermediate_tags
                    .iter()
                    .map(|tag| tag.name.as_ref().to_string())
                    .collect()
            },
            end_required: end.required,
            opaque: spec.opaque,
        })
    }

    pub(crate) fn validate_close(
        opener_bits: &[TagBit],
        closer_bits: &[TagBit],
    ) -> CloseValidation {
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TagClassification {
    Opener(OpeningContract),
    Standalone,
    Closer {
        possible_openers: Vec<String>,
    },
    Intermediate {
        possible_openers: Vec<String>,
    },
    /// The Project grammar vocabulary is open or feasible backends disagree.
    /// Treating this as a definite orphan would be unsound.
    Inconclusive,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TagGrammarFact {
    pub(crate) spec: Option<TagSpec>,
    pub(crate) classification: TagClassification,
}

/// Per-pass grammar containing only source occurrences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GrammarOccurrenceKey(u32);

impl GrammarOccurrenceKey {
    fn from_name_span(span: Span) -> Self {
        Self(span.start())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SparseTagGrammar {
    occurrences: BTreeMap<GrammarOccurrenceKey, TagGrammarFact>,
}

impl SparseTagGrammar {
    pub(crate) fn for_pass(
        db: &dyn Db,
        source_file: File,
        scope_file: File,
        nodelist: NodeList<'_>,
        loaded: &LoadedLibraries,
    ) -> Self {
        let mut occurrences = BTreeMap::new();
        let mut load_cursor = loaded.cursor();
        for node in nodelist.nodelist(db) {
            let Node::Tag {
                name,
                name_span,
                span,
                ..
            } = node
            else {
                continue;
            };
            let load_state = load_cursor.advance_to(span.start());
            let spec = match db.project() {
                Some(project) => crate::tags::effective_tag_spec_in_project_for_scope(
                    db,
                    project,
                    scope_file,
                    name,
                    &load_state,
                ),
                None => crate::tags::effective_tag_spec(db, source_file, name, &load_state),
            };
            let classification = spec.as_ref().map_or_else(
                || classify_orphan(db, source_file, scope_file, name, &load_state),
                |spec| {
                    OpeningContract::from_spec(spec)
                        .map_or(TagClassification::Standalone, TagClassification::Opener)
                },
            );
            occurrences.insert(
                GrammarOccurrenceKey::from_name_span(*name_span),
                TagGrammarFact {
                    spec,
                    classification,
                },
            );
        }
        Self { occurrences }
    }

    #[must_use]
    pub(crate) fn for_name_span(&self, name_span: Span) -> Option<&TagGrammarFact> {
        self.occurrences
            .get(&GrammarOccurrenceKey::from_name_span(name_span))
    }
}

fn classify_orphan(
    db: &dyn Db,
    source_file: File,
    scope_file: File,
    spelling: &str,
    load_state: &crate::scoping::LoadState<'_>,
) -> TagClassification {
    let Some(project) = db.project() else {
        return classify_projectless_orphan(db, source_file, spelling);
    };
    let vocabulary = semantic_grammar_vocabulary(db, project);
    let environment = template_environment(db, project, scope_file);

    let (closers, closer_uncertain) = resolve_orphan_candidates(
        db,
        project,
        environment,
        vocabulary.closer_candidates(spelling),
        load_state,
        |spec| {
            spec.end_tag
                .as_ref()
                .is_some_and(|end| end.name == spelling)
        },
    );
    if !closers.is_empty() {
        return TagClassification::Closer {
            possible_openers: closers,
        };
    }

    let (intermediates, intermediate_uncertain) = resolve_orphan_candidates(
        db,
        project,
        environment,
        vocabulary.intermediate_candidates(spelling),
        load_state,
        |spec| {
            !spec.opaque
                && spec
                    .intermediate_tags
                    .iter()
                    .any(|intermediate| intermediate.name == spelling)
        },
    );
    if !intermediates.is_empty() {
        return TagClassification::Intermediate {
            possible_openers: intermediates,
        };
    }

    if vocabulary.is_open() || closer_uncertain || intermediate_uncertain {
        TagClassification::Inconclusive
    } else {
        TagClassification::Unknown
    }
}

fn resolve_orphan_candidates(
    db: &dyn Db,
    project: Project,
    environment: djls_project::TemplateEnvironment<'_>,
    candidates: &[GrammarOpeningDefinition],
    load_state: &crate::scoping::LoadState<'_>,
    matches_spelling: impl Fn(&TagSpec) -> bool,
) -> (Vec<String>, bool) {
    let mut openers = Vec::new();
    let mut uncertain = false;
    for candidate in candidates {
        let loaded = load_state.libraries_loading_symbol(candidate.name());
        let definitions = environment.effective_definition_libraries(
            candidate.name(),
            TemplateSymbolKind::Tag,
            &loaded,
        );
        let matching = definitions
            .iter()
            .filter(|definition| {
                matches!(
                    definition,
                    EffectiveDefinitionLibrary::Known(Some(library))
                        if library.key(db) == *candidate.library()
                )
            })
            .count();
        let unknown = definitions.iter().any(|definition| {
            matches!(
                definition,
                EffectiveDefinitionLibrary::Unknown | EffectiveDefinitionLibrary::Unobserved(_)
            )
        });
        if unknown || (matching > 0 && matching != definitions.len()) {
            uncertain = true;
            continue;
        }
        if matching == definitions.len()
            && matching > 0
            && library_tag_specs(db, project, *candidate.library())
                .get(candidate.name())
                .is_some_and(&matches_spelling)
            && !openers.iter().any(|name| name == candidate.name())
        {
            openers.push(candidate.name().to_string());
        }
    }
    openers.sort();
    (openers, uncertain)
}

fn classify_projectless_orphan(db: &dyn Db, _file: File, spelling: &str) -> TagClassification {
    let mut closers = Vec::new();
    let mut intermediates = Vec::new();
    for (name, spec) in db.projectless_tag_specs() {
        let Some(contract) = OpeningContract::from_spec(spec) else {
            continue;
        };
        if contract.closer == spelling {
            closers.push(name.clone());
        }
        if contract.intermediates.iter().any(|item| item == spelling) {
            intermediates.push(name.clone());
        }
    }
    closers.sort();
    closers.dedup();
    intermediates.sort();
    intermediates.dedup();
    if !closers.is_empty() {
        TagClassification::Closer {
            possible_openers: closers,
        }
    } else if !intermediates.is_empty() {
        TagClassification::Intermediate {
            possible_openers: intermediates,
        }
    } else {
        TagClassification::Unknown
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CloseValidation {
    Valid,
    ArgumentMismatch {
        expected: String,
        got: String,
        got_span: Span,
    },
}
