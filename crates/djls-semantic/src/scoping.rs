pub(crate) mod loads;
pub(crate) mod symbols;

use std::collections::BTreeMap;

use djls_project::EffectiveDefinitionLibrary;
use djls_project::FilterArity;
use djls_project::LibraryName;
use djls_project::MissingLibraryLookup;
use djls_project::SymbolDefinition;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolAvailability;
use djls_project::TemplateSymbolCandidate;
use djls_project::TemplateSymbolKind;
use djls_source::File;
use djls_templates::NodeList;
use salsa::Accumulator;

use crate::db::Db;
pub(crate) use crate::scoping::loads::LoadKind;
pub(crate) use crate::scoping::loads::LoadState;
pub(crate) use crate::scoping::loads::LoadStatement;
pub(crate) use crate::scoping::loads::LoadedLibraries;
use crate::scoping::symbols::SymbolAvailability;
use crate::scoping::symbols::resolve_occurrence_availability;
use crate::structure::ActiveTemplateNode;
use crate::structure::ActiveTemplateTag;
use crate::structure::CapturedClosingTag;
use crate::structure::StructuralOccurrenceMeaning;
use crate::structure::TagClassification;
use crate::structure::active_template_nodes;
use crate::structure::grammar::SparseTagGrammar;
use crate::tags::TagRole;
use crate::tags::TagSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TagOccurrenceKey(u32);

impl TagOccurrenceKey {
    fn from_name_span(span: djls_source::Span) -> Self {
        Self(span.start())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FilterOccurrenceKey(u32);

impl FilterOccurrenceKey {
    fn from_filter(filter: &djls_templates::Filter) -> Self {
        Self(filter.span.start())
    }
}

/// Deduplicates contextual resolution without leaking cache identity into load scoping.
/// A visible statement count fully identifies the semantic load prefix within one Template.
#[derive(Debug)]
struct ContextualFactCache<T> {
    by_load_prefix: BTreeMap<usize, BTreeMap<String, T>>,
}

impl<T> Default for ContextualFactCache<T> {
    fn default() -> Self {
        Self {
            by_load_prefix: BTreeMap::new(),
        }
    }
}

impl<T: Clone> ContextualFactCache<T> {
    fn resolve(
        &mut self,
        load_state: LoadState<'_>,
        symbol_name: &str,
        resolve: impl FnOnce() -> T,
    ) -> T {
        let facts_by_name = self
            .by_load_prefix
            .entry(load_state.visible_statement_count())
            .or_default();
        if let Some(fact) = facts_by_name.get(symbol_name) {
            return fact.clone();
        }

        let fact = resolve();
        facts_by_name.insert(symbol_name.to_string(), fact.clone());
        fact
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextualTagFact {
    availability: SymbolAvailability,
    unknown_load_can_shadow: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct ContextualFilterFact {
    availability: SymbolAvailability,
    arity: Option<FilterArity>,
    unknown_load_can_shadow: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoaderArgumentFact {
    pub(crate) argument: crate::scoping::loads::LoadArgument,
    pub(crate) availability: MissingLibraryLookup,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedTagFact {
    pub(crate) spec: Option<TagSpec>,
    pub(crate) availability: SymbolAvailability,
    pub(crate) structure_accepts_spelling: bool,
    pub(crate) unknown_load_can_shadow: bool,
    pub(crate) loader_arguments: Vec<LoaderArgumentFact>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ScopedTagFacts(BTreeMap<TagOccurrenceKey, ScopedTagFact>);

impl ScopedTagFacts {
    #[must_use]
    pub(crate) fn for_tag(&self, tag: ActiveTemplateTag<'_>) -> Option<&ScopedTagFact> {
        self.0.get(&TagOccurrenceKey::from_name_span(tag.name_span))
    }

    #[must_use]
    pub(crate) fn for_name_span(&self, name_span: djls_source::Span) -> Option<&ScopedTagFact> {
        self.0.get(&TagOccurrenceKey::from_name_span(name_span))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedFilterFact {
    pub(crate) availability: SymbolAvailability,
    pub(crate) arity: Option<FilterArity>,
    pub(crate) unknown_load_can_shadow: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ScopedFilterFacts(BTreeMap<FilterOccurrenceKey, ScopedFilterFact>);

impl ScopedFilterFacts {
    #[must_use]
    pub(crate) fn for_filter(&self, filter: &djls_templates::Filter) -> Option<&ScopedFilterFact> {
        self.0.get(&FilterOccurrenceKey::from_filter(filter))
    }
}

/// One correlated, converged semantic product for a source under an effective scope.
#[salsa::tracked]
pub(crate) struct TemplateAnalysisProjection<'db> {
    #[returns(ref)]
    pub(crate) loaded_libraries: LoadedLibraries,
    #[tracked]
    #[returns(ref)]
    pub(crate) scoped_tag_facts: ScopedTagFacts,
    #[tracked]
    #[returns(ref)]
    pub(crate) scoped_filter_facts: ScopedFilterFacts,
    #[tracked]
    #[returns(ref)]
    pub(crate) captured_closers: Vec<CapturedClosingTag>,
    pub(crate) tree: crate::structure::TemplateTree<'db>,
}

#[salsa::tracked]
#[allow(clippy::too_many_lines)]
pub(crate) fn template_analysis_projection_for_file_in_scope<'db>(
    db: &'db dyn Db,
    _source_file: File,
    nodelist: NodeList<'db>,
    scope_file: File,
) -> TemplateAnalysisProjection<'db> {
    // Structural/load feedback can change only at parsed tag occurrences. Keep
    // the convergence budget tied to that semantic input rather than raw text,
    // which would bypass NodeList backdating on presentation-only edits.
    let fixed_point_limit = nodelist
        .nodelist(db)
        .iter()
        .filter(|node| matches!(node, djls_templates::Node::Tag { .. }))
        .count()
        + 1;

    let project = db.project();
    let environment = crate::db::template_environment_for_file(db, scope_file);
    let mut loaded = LoadedLibraries::default();
    for _ in 0..fixed_point_limit {
        let grammar = project.map_or_else(
            || SparseTagGrammar::projectless(db, nodelist),
            |project| SparseTagGrammar::project_pass(db, project, nodelist, &loaded, environment),
        );
        // Fixed-point passes are plain temporary values. No tracked Tree identity
        // or structural diagnostic is produced until this pass converges.
        let tree_data =
            crate::structure::TemplateTreeBuilder::new(db, &grammar).model_data(db, nodelist);
        let mut active_nodes = active_template_nodes(&tree_data.regions, tree_data.root);
        active_nodes.extend(
            tree_data
                .captured_closers
                .iter()
                .map(|closer| ActiveTemplateNode::Tag(closer.as_active())),
        );
        active_nodes.sort_by_key(|node| match node {
            ActiveTemplateNode::Tag(tag) => tag.full_span.start(),
            ActiveTemplateNode::Variable(variable) => variable.span.start(),
        });
        let mut statements = Vec::new();
        for node in &active_nodes {
            let ActiveTemplateNode::Tag(tag) = node else {
                continue;
            };
            let role = occurrence_spec(&grammar, *tag).and_then(TagSpec::role);
            if role == Some(TagRole::TemplateLibraryLoader)
                && let Some(statement) = LoadStatement::from_loader_bits(tag.bits, tag.span)
            {
                statements.push(statement);
            }
        }
        let next = LoadedLibraries::new(statements);
        if next != loaded {
            loaded = next;
            continue;
        }

        let mut tag_facts = BTreeMap::new();
        let mut filter_facts = BTreeMap::new();
        let mut tag_context_cache = ContextualFactCache::default();
        let mut filter_context_cache = ContextualFactCache::default();
        let mut load_cursor = loaded.cursor();
        for node in &active_nodes {
            match node {
                ActiveTemplateNode::Tag(tag) => {
                    let Some(grammar_fact) = grammar.for_name_span(tag.name_span) else {
                        continue;
                    };
                    let spec = occurrence_spec(&grammar, *tag);
                    let load_state = load_cursor.advance_to(tag.span.start());
                    let contextual_fact =
                        tag_context_cache.resolve(load_state, tag.tag, || ContextualTagFact {
                            availability: if project.is_none() {
                                if grammar_fact.spec.is_some() {
                                    SymbolAvailability::Available
                                } else {
                                    SymbolAvailability::Unknown
                                }
                            } else {
                                resolve_occurrence_availability(
                                    environment,
                                    &load_state,
                                    tag.tag,
                                    TemplateSymbolKind::Tag,
                                )
                            },
                            unknown_load_can_shadow: load_state.unknown_load_can_shadow_symbol(
                                tag.tag,
                                TemplateSymbolKind::Tag,
                                environment,
                            ),
                        });
                    let loader_arguments =
                        if spec.and_then(TagSpec::role) == Some(TagRole::TemplateLibraryLoader) {
                            LoadKind::from_loader_bits(tag.bits).map_or_else(Vec::new, |kind| {
                                kind.into_library_arguments()
                                    .into_iter()
                                    .filter_map(|argument| {
                                        let name = LibraryName::parse(argument.as_str()).ok()?;
                                        Some(LoaderArgumentFact {
                                            availability: environment.missing_library(&name),
                                            argument,
                                        })
                                    })
                                    .collect()
                            })
                        } else {
                            Vec::new()
                        };
                    tag_facts.insert(
                        TagOccurrenceKey::from_name_span(tag.name_span),
                        ScopedTagFact {
                            spec: spec.cloned(),
                            availability: contextual_fact.availability,
                            structure_accepts_spelling: matches!(
                                tag.structural_meaning,
                                StructuralOccurrenceMeaning::CapturedIntermediate
                                    | StructuralOccurrenceMeaning::CapturedCloser
                            ) || matches!(
                                grammar_fact.classification,
                                TagClassification::Inconclusive
                            ),
                            unknown_load_can_shadow: contextual_fact.unknown_load_can_shadow,
                            loader_arguments,
                        },
                    );
                }
                ActiveTemplateNode::Variable(variable) => {
                    let load_state = load_cursor.advance_to(variable.span.start());
                    for filter in variable.filters {
                        let contextual_fact =
                            filter_context_cache.resolve(load_state, &filter.name, || {
                                let (availability, arity) = if project.is_none() {
                                    let arity = db
                                        .projectless_filter_arity_specs()
                                        .get(&filter.name)
                                        .cloned();
                                    let availability = if arity.is_some() {
                                        SymbolAvailability::Available
                                    } else {
                                        SymbolAvailability::Unknown
                                    };
                                    (availability, arity)
                                } else {
                                    (
                                        resolve_occurrence_availability(
                                            environment,
                                            &load_state,
                                            &filter.name,
                                            TemplateSymbolKind::Filter,
                                        ),
                                        crate::filters::effective_filter_arity_in_environment(
                                            db,
                                            environment,
                                            &filter.name,
                                            &load_state,
                                        ),
                                    )
                                };
                                ContextualFilterFact {
                                    availability,
                                    arity,
                                    unknown_load_can_shadow: load_state
                                        .unknown_load_can_shadow_symbol(
                                            &filter.name,
                                            TemplateSymbolKind::Filter,
                                            environment,
                                        ),
                                }
                            });
                        filter_facts.insert(
                            FilterOccurrenceKey::from_filter(filter),
                            ScopedFilterFact {
                                availability: contextual_fact.availability,
                                arity: contextual_fact.arity,
                                unknown_load_can_shadow: contextual_fact.unknown_load_can_shadow,
                            },
                        );
                    }
                }
            }
        }

        for error in &tree_data.diagnostics {
            crate::ValidationErrorAccumulator(error.clone()).accumulate(db);
        }
        let captured_closers = tree_data.captured_closers.clone();
        let tree = tree_data.into_tree(db);
        return TemplateAnalysisProjection::new(
            db,
            loaded,
            ScopedTagFacts(tag_facts),
            ScopedFilterFacts(filter_facts),
            captured_closers,
            tree,
        );
    }
    panic!("template load discovery did not converge within the number of template tags")
}

fn occurrence_spec<'a>(
    grammar: &'a SparseTagGrammar,
    tag: ActiveTemplateTag<'_>,
) -> Option<&'a TagSpec> {
    match tag.structural_meaning {
        StructuralOccurrenceMeaning::Definition => grammar
            .for_name_span(tag.name_span)
            .and_then(|fact| fact.spec.as_ref()),
        StructuralOccurrenceMeaning::CapturedIntermediate
        | StructuralOccurrenceMeaning::CapturedCloser => None,
    }
}

#[salsa::tracked]
pub(crate) fn template_analysis_projection_for_file<'db>(
    db: &'db dyn Db,
    file: File,
    nodelist: NodeList<'db>,
) -> TemplateAnalysisProjection<'db> {
    template_analysis_projection_for_file_in_scope(db, file, nodelist, file)
}

/// Return the effective definition of one symbol at a source position.
///
/// Django applies builtins and then loaded libraries in source order, with later definitions
/// shadowing earlier ones. The candidate is omitted when feasible backends disagree.
#[must_use]
pub fn effective_symbol_candidate_at(
    db: &dyn Db,
    file: File,
    nodelist: NodeList<'_>,
    position: u32,
    name: &str,
    kind: TemplateSymbolKind,
) -> Option<TemplateSymbolCandidate> {
    let environment = crate::db::template_environment_for_file(db, file);
    let projection = template_analysis_projection_for_file(db, file, nodelist);
    let load_state = projection.loaded_libraries(db).available_at(position);
    let loaded_names = load_state.libraries_loading_symbol(name);
    let definitions = environment.effective_definition_libraries(name, kind, &loaded_names);
    let candidates = definitions
        .into_iter()
        .map(|definition| {
            let EffectiveDefinitionLibrary::Known(Some(library)) = definition else {
                return None;
            };
            let symbol = library.symbol(kind, name)?.clone();
            let availability = library.load_name().map_or_else(
                || TemplateSymbolAvailability::Builtin {
                    module: library.module_name().clone(),
                },
                |load_name| TemplateSymbolAvailability::RequiresLoad {
                    load_name: load_name.clone(),
                },
            );
            Some((
                library.key(),
                TemplateSymbolCandidate {
                    symbol,
                    availability,
                },
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let first = candidates.first()?;
    if !candidates
        .iter()
        .all(|candidate| effective_definitions_agree(first, candidate))
    {
        return None;
    }

    let symbol = candidates
        .iter()
        .map(|(_, candidate)| &candidate.symbol)
        .max_by_key(|symbol| {
            symbol
                .doc()
                .filter(|doc| !doc.trim().is_empty())
                .map(str::trim)
        })?
        .clone();
    let mut availability = first.1.availability.clone();
    for (_, candidate) in &candidates[1..] {
        if availability == candidate.availability {
            continue;
        }
        match (&availability, &candidate.availability) {
            (
                TemplateSymbolAvailability::Builtin { .. },
                TemplateSymbolAvailability::RequiresLoad { .. },
            ) => availability = candidate.availability.clone(),
            (
                TemplateSymbolAvailability::RequiresLoad { .. },
                TemplateSymbolAvailability::Builtin { .. },
            ) => {}
            (
                TemplateSymbolAvailability::RequiresLoad { .. },
                TemplateSymbolAvailability::RequiresLoad { .. },
            )
            | (
                TemplateSymbolAvailability::Builtin { .. },
                TemplateSymbolAvailability::Builtin { .. },
            ) => return None,
        }
    }

    Some(TemplateSymbolCandidate {
        symbol,
        availability,
    })
}

fn effective_definitions_agree(
    left: &(TemplateLibraryKey, TemplateSymbolCandidate),
    right: &(TemplateLibraryKey, TemplateSymbolCandidate),
) -> bool {
    left.1.symbol.has_same_definition(&right.1.symbol)
        || (left.0 == right.0
            && matches!(left.1.symbol.definition, SymbolDefinition::Unknown)
            && matches!(right.1.symbol.definition, SymbolDefinition::Unknown))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use djls_source::Span;

    use super::ContextualFactCache;
    use super::LoadKind;
    use super::LoadStatement;
    use super::LoadedLibraries;
    use crate::scoping::loads::LoadArgument;

    #[test]
    fn contextual_fact_cache_resolves_once_per_visible_prefix_and_symbol() {
        let loaded = LoadedLibraries::new(vec![LoadStatement::new(
            Span::new(10, 10),
            LoadKind::FullLoad {
                libraries: vec![LoadArgument::from("extras")],
            },
        )]);
        let resolutions = Cell::new(0);
        let mut cache = ContextualFactCache::default();
        let mut resolve = || {
            resolutions.set(resolutions.get() + 1);
            resolutions.get()
        };

        assert_eq!(
            cache.resolve(loaded.available_at(0), "shared", &mut resolve),
            1
        );
        assert_eq!(
            cache.resolve(loaded.available_at(5), "shared", &mut resolve),
            1
        );
        assert_eq!(
            cache.resolve(loaded.available_at(5), "other", &mut resolve),
            2
        );
        assert_eq!(
            cache.resolve(loaded.available_at(30), "shared", &mut resolve),
            3
        );
        assert_eq!(
            cache.resolve(loaded.available_at(40), "shared", &mut resolve),
            3
        );
        assert_eq!(resolutions.get(), 3);
    }
}
